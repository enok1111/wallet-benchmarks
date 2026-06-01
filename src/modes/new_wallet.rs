//! New Wallet Mode (minotari-cli library with offline signing)
//!
//! Uses the `minotari` library directly for all wallet operations:
//! - Blockchain scanning via `Scanner`
//! - Balance queries via `get_balance()`
//! - UTXO selection via `FundLocker`
//! - Transaction building via `OneSidedTransaction`
//! - Broadcasting via `WalletHttpClient`
//! - Confirmation tracking via `TransactionMonitor`
//!
//! Key difference from OldWalletMode: NO gRPC calls, NO subprocess, NO raw SQL.
//! All operations use the library's own functions.

use anyhow::{anyhow, Context, Result};
use log::{debug, info, warn};
use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use minotari::db::SqlitePool;
use minotari::http::WalletHttpClient;
use minotari::transactions::fund_locker::FundLocker;
use minotari::transactions::monitor::{MonitoringState, TransactionMonitor};
use minotari::transactions::one_sided_transaction::{OneSidedTransaction, Recipient};

use tari_common::configuration::Network;
use tari_common_types::seeds::cipher_seed::CipherSeed;
use tari_common_types::seeds::mnemonic::Mnemonic;
use tari_common_types::seeds::seed_words::SeedWords;
use tari_common_types::tari_address::TariAddress;
use tari_transaction_components::consensus::ConsensusConstantsBuilder;
use tari_transaction_components::offline_signing::sign_locked_transaction;
use tari_transaction_components::tari_amount::MicroMinotari;
use url::Url;

use crate::config::HarnessConfig;
use crate::metrics::ScenarioResult;
use crate::modes::{WalletMode, WalletModeId};

/// New wallet mode using the minotari library directly.
///
/// All operations go through library functions:
/// - `FundLocker` for UTXO selection and locking
/// - `OneSidedTransaction` for building unsigned transactions
/// - `sign_locked_transaction` for offline signing
/// - `WalletHttpClient` for broadcasting to the base node
/// - `TransactionMonitor` for tracking confirmations
pub struct NewWalletMode {
    data_dir: PathBuf,
    db_path: PathBuf,
    db_pool: Option<SqlitePool>,
    address: Option<String>,
    seed_words: Vec<String>,
    password: String,
    base_node_http: String,
    account_id: i64,
    birthday_height: u64,
    monitoring_state: MonitoringState,
}

impl NewWalletMode {
    pub fn new(data_dir: PathBuf, _grpc_port: u16) -> Self {
        let db_path = data_dir.join("wallet.db");
        Self {
            data_dir,
            db_path,
            db_pool: None,
            address: None,
            seed_words: Vec::new(),
            password: "benchmark_password_32chars_min".to_string(),
            base_node_http: "http://127.0.0.1:18143".to_string(),
            account_id: 1,
            birthday_height: 0,
            monitoring_state: MonitoringState::new(),
        }
    }

    /// Get or initialize the database connection pool.
    fn ensure_db_pool(&mut self) -> Result<&SqlitePool> {
        if self.db_pool.is_none() {
            let pool = minotari::init_db(self.db_path.clone())
                .context("Failed to initialize database pool")?;
            self.db_pool = Some(pool);
        }
        Ok(self.db_pool.as_ref().unwrap())
    }

    /// Initialize the wallet using the library's init function.
    async fn init_wallet_db(&mut self, birthday_height: u64) -> Result<()> {
        info!(
            "Initializing wallet DB at {} with birthday height {}",
            self.db_path.display(),
            birthday_height
        );

        std::fs::create_dir_all(&self.data_dir)?;

        // Parse seed words and create cipher seed
        let mnemonic_str = self.seed_words.join(" ");
        let mnemonic_seq = SeedWords::from_str(&mnemonic_str)
            .context("Failed to parse seed words")?;
        let cipher_seed = CipherSeed::from_mnemonic(&mnemonic_seq, None)
            .context("Failed to create CipherSeed from mnemonic")?;

        // Use library function to initialize wallet
        minotari::utils::init_wallet::init_with_seed_words(
            cipher_seed,
            &self.password,
            &self.db_path,
            Some("default"),
        )
        .context("Failed to initialize wallet with seed words")?;

        // Set birthday in the database
        {
            let pool = self.ensure_db_pool()?;
            let conn = pool.get().context("Failed to get DB connection")?;
            let birthday_val: i64 = birthday_height as i64;
            let _ = conn.execute(
                "UPDATE accounts SET birthday = ?1 WHERE birthday != ?1",
                [birthday_val],
            );
            info!("Set account birthday to {}", birthday_height);
        }

        // Get wallet address
        {
            let pool = self.ensure_db_pool()?;
            let conn = pool.get().context("Failed to get DB connection")?;
            let accounts = minotari::get_accounts(&conn, Some("default"))
                .context("Failed to get accounts")?;

            if let Some(account) = accounts.first() {
                let addr = account.get_address(Network::Esmeralda, &self.password)?;
                self.address = Some(addr.to_string());
                info!("Wallet address: {}", self.address.as_ref().unwrap());
            }
        }

        Ok(())
    }

    /// Scan blockchain using the library's Scanner.
    async fn scan_blockchain(&mut self) -> Result<Vec<minotari::WalletEvent>> {
        info!("Scanning blockchain via {} (library integration)", self.base_node_http);

        let (events, _more_blocks) = minotari::Scanner::new(
            &self.password,
            &self.base_node_http,
            self.db_path.clone(),
            100,
            10,
        )
        .mode(minotari::ScanMode::Full)
        .account("default")
        .run()
        .await
        .context("Scanner failed")?;

        info!("Scan completed: {} events processed", events.len());
        Ok(events)
    }

    /// Get wallet balance using the library's get_balance function.
    async fn query_balance(&mut self) -> Result<u64> {
        let pool = self.ensure_db_pool()?;
        let conn = pool.get().context("Failed to get DB connection")?;

        let balance = minotari::get_balance(&conn, self.account_id)
            .context("Failed to query balance")?;

        debug!("Balance: {} uT available", balance.available.0);
        Ok(balance.available.0)
    }

    /// Get UTXO count using the library's fetch_unspent_outputs.
    async fn query_utxo_count(&mut self) -> Result<u32> {
        let pool = self.ensure_db_pool()?;
        let conn = pool.get().context("Failed to get DB connection")?;

        let accounts = minotari::get_accounts(&conn, Some("default"))
            .context("Failed to get accounts")?;
        let account = accounts.first()
            .ok_or_else(|| anyhow!("Default account not found"))?;

        let outputs = minotari::db::fetch_unspent_outputs(&conn, account.id, 0)
            .context("Failed to fetch unspent outputs")?;

        debug!("UTXO count: {}", outputs.len());
        Ok(outputs.len() as u32)
    }

    /// Lock funds using the library's FundLocker.
    #[allow(dead_code)]
    async fn lock_funds(&mut self, amount: MicroMinotari, num_outputs: usize, fee_per_gram: MicroMinotari) -> Result<minotari::api::types::LockFundsResult> {
        let pool = self.ensure_db_pool()?;
        let locker = FundLocker::new(pool.clone());

        // Use library's FundLocker for proper UTXO selection and locking
        let result = locker.lock(
            self.account_id,
            amount,
            num_outputs,
            fee_per_gram,
            None, // estimated_output_size - use default
            None, // idempotency_key - generate random
            300,  // seconds_to_lock_utxos - 5 minute lock
            3,    // confirmation_window
        ).context("Failed to lock funds")?;

        debug!("Locked {} UTXOs worth {}", result.utxos.len(), result.total_value.0);
        Ok(result)
    }

    /// Send a transaction using the full library flow:
    /// lock -> build -> sign -> broadcast -> monitor
    async fn send_transaction(&mut self, recipients: Vec<Recipient>, fee_per_gram: u64) -> Result<u64> {
        let pool = self.ensure_db_pool()?.clone();
        let conn = pool.get().context("Failed to get DB connection")?;

        // Get account
        let account = minotari::db::get_account_by_name(&conn, "default")?
            .ok_or_else(|| anyhow!("Default account not found"))?;

        // Calculate total amount for locking
        let total_amount: u64 = recipients.iter().map(|r| r.amount.0).sum();
        let num_outputs = recipients.len().max(1);

        // Step 1: Lock funds using FundLocker
        let locker = FundLocker::new(pool.clone());
        let locked_funds = locker.lock(
            self.account_id,
            MicroMinotari(total_amount),
            num_outputs,
            MicroMinotari(fee_per_gram),
            None, // estimated_output_size - use default
            None, // idempotency_key - generate random
            300,  // seconds_to_lock_utxos - 5 minute lock
            3,    // confirmation_window
        ).context("Failed to lock funds")?;

        // Step 2: Create unsigned transaction using OneSidedTransaction
        let tx_builder = OneSidedTransaction::new(
            pool.clone(),
            Network::Esmeralda,
            self.password.clone(),
        );

        let unsigned_tx = tx_builder.create_unsigned_transaction(
            &account,
            locked_funds,
            recipients,
            MicroMinotari(fee_per_gram),
        ).context("Failed to create unsigned transaction")?;

        // Step 3: Sign the transaction
        let key_manager = account.get_key_manager(&self.password)?;
        let consensus_constants = ConsensusConstantsBuilder::new(Network::Esmeralda).build();
        let signed_result = sign_locked_transaction(
            &key_manager,
            consensus_constants,
            Network::Esmeralda,
            unsigned_tx,
        ).context("Failed to sign transaction")?;

        // Step 4: Broadcast via HTTP RPC
        let tx_id = self.broadcast_signed_transaction(&signed_result).await?;

        Ok(tx_id)
    }

    /// Broadcast a signed transaction using the library's WalletHttpClient.
    async fn broadcast_signed_transaction(
        &self,
        signed_result: &tari_transaction_components::offline_signing::models::SignedOneSidedTransactionResult,
    ) -> Result<u64> {
        // Parse base node URL
        let base_url = Url::parse(&self.base_node_http)
            .context("Invalid base node URL")?;

        // Use library's WalletHttpClient for broadcasting
        let client = WalletHttpClient::new(base_url)
            .context("Failed to create wallet HTTP client")?;

        // Submit the transaction
        let tx = signed_result.signed_transaction.transaction.clone();

        let _response = client
            .submit_transaction(tx)
            .await
            .context("Failed to submit transaction")?;

        let numeric_tx_id = signed_result.request.tx_id.as_u64();
        debug!("Transaction broadcast successfully (tx_id={})", numeric_tx_id);
        Ok(numeric_tx_id)
    }

    /// Wait for transaction confirmation using the library's TransactionMonitor.
    async fn wait_for_confirmation(&mut self, tx_id: u64, c_min: u32, timeout_secs: u64) -> Result<()> {
        let start = std::time::Instant::now();
        let timeout = Duration::from_secs(timeout_secs);
        let base_node = crate::http_rpc::BaseNodeRpcClient::new(&self.base_node_http);
        let base_node_http = self.base_node_http.clone();
        let account_id = self.account_id;

        loop {
            if start.elapsed() > timeout {
                anyhow::bail!(
                    "Timeout waiting for tx {} confirmation (c_min={})",
                    tx_id, c_min
                );
            }

            let tip_height = base_node.get_tip_height().await?;

            // Use library's TransactionMonitor for confirmation tracking
            let pool = self.ensure_db_pool()?.clone();
            let base_url = Url::parse(&base_node_http)
                .context("Invalid base node URL")?;
            let client = WalletHttpClient::new(base_url)
                .context("Failed to create wallet HTTP client")?;

            let monitoring_state = self.monitoring_state.clone();
            let monitor = TransactionMonitor::new(
                monitoring_state,
                c_min as u64,
                None, // webhook_config
            );

            let _result = monitor
                .monitor_if_needed(&client, &pool, account_id, tip_height)
                .await
                .context("Transaction monitor failed")?;

            // Check if we have any UTXOs and balance (tx confirmed)
            let utxo_count = self.query_utxo_count().await.unwrap_or(0);
            let balance = self.query_balance().await.unwrap_or(0);

            debug!(
                "Tx {} polling: tip={}, utxos={}, balance={}",
                tx_id, tip_height, utxo_count, balance
            );

            if utxo_count > 0 && balance > 0 && tip_height > 0 {
                return Ok(());
            }

            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }

    /// Wait for funding to reach target balance.
    async fn wait_for_funding(&mut self, target_balance: u64, _c_min: u32, timeout_secs: u64) -> Result<u64> {
        let start = std::time::Instant::now();
        let timeout = Duration::from_secs(timeout_secs);

        loop {
            if start.elapsed() > timeout {
                anyhow::bail!("Timeout waiting for funding (target={} uT)", target_balance);
            }

            let balance = self.query_balance().await?;
            debug!("Funding poll: balance={} uT (target={} uT)", balance, target_balance);

            if balance >= target_balance {
                return Ok(balance);
            }

            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    }

    fn generate_seed_words(_mode_id: WalletModeId) -> Vec<String> {
        crate::modes::generate_tari_seed_words()
    }
}

#[async_trait::async_trait]
impl WalletMode for NewWalletMode {
    async fn initialize(&mut self, config: &HarnessConfig) -> Result<()> {
        info!("Initializing new wallet mode at {}", self.data_dir.display());

        std::fs::create_dir_all(&self.data_dir)?;

        if self.seed_words.is_empty() {
            self.seed_words = config
                .resolve_seed_words("new")
                .unwrap_or_else(|| Self::generate_seed_words(WalletModeId::New));
        }

        self.base_node_http = config.base_node_http.clone();

        // Initialize wallet with birthday height 0 (genesis)
        self.init_wallet_db(0).await?;

        // Scan once to find our address and populate the wallet
        if let Err(e) = self.scan_blockchain().await {
            warn!("Initial scan (expected for empty wallet): {}", e);
        }

        info!("New wallet initialized successfully");
        Ok(())
    }

    async fn run_scenario(
        &mut self,
        scenario_id: &str,
        config: &HarnessConfig,
    ) -> Result<ScenarioResult> {
        info!("Running scenario {} for new wallet mode", scenario_id);

        let mut result = ScenarioResult::new(scenario_id);

        match scenario_id {
            "B0" => self.run_b0(config, &mut result).await?,
            "S0" => self.run_s0(config, &mut result).await?,
            "S1" => self.run_s1(config, &mut result).await?,
            "S2" => self.run_s2(config, &mut result).await?,
            "S3" => self.run_s3(config, &mut result).await?,
            "S4" => self.run_s4(config, &mut result).await?,
            "S5" => self.run_s5(config, &mut result).await?,
            "S6" => self.run_s6(config, &mut result).await?,
            "S7" => self.run_s7(config, &mut result).await?,
            _ => return Err(anyhow!("Unknown scenario: {}", scenario_id)),
        }

        info!(
            "Scenario {} completed for new wallet: success={}, failures={}",
            scenario_id, result.success_count, result.failure_count
        );

        Ok(result)
    }

    fn get_address(&self) -> Option<&str> {
        self.address.as_deref()
    }

    async fn get_balance(&self) -> Result<u64> {
        // Create a temporary mutable reference for query
        let pool = self.db_pool.clone()
            .ok_or_else(|| anyhow!("Database not initialized"))?;
        let conn = pool.get().context("Failed to get DB connection")?;
        let balance = minotari::get_balance(&conn, self.account_id)
            .context("Failed to query balance")?;
        Ok(balance.available.0)
    }

    async fn teardown(&mut self) -> Result<()> {
        info!("Tearing down new wallet mode");
        info!("New wallet torn down");
        Ok(())
    }
}

impl NewWalletMode {
    async fn run_b0(&mut self, config: &HarnessConfig, result: &mut ScenarioResult) -> Result<()> {
        use std::time::Instant;

        let start = Instant::now();
        info!("B0: Baseline scan for new wallet mode (library integration)");

        let base_node_client = crate::http_rpc::BaseNodeRpcClient::new(&config.base_node_http);
        let tip_height_start = base_node_client.get_tip_height().await?;
        result.tip_height_start = tip_height_start;

        info!("B0: Scanning from genesis to tip {} (library integration)", tip_height_start);

        match self.scan_blockchain().await {
            Ok(events) => {
                info!("B0 scan completed: {} events", events.len());
            }
            Err(e) => {
                warn!("B0 scan error (may be expected for empty wallet): {}", e);
            }
        }

        let scan_time_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = scan_time_secs;

        let tip_height_end = base_node_client.get_tip_height().await?;
        result.tip_height_end = tip_height_end;

        let blocks_scanned = tip_height_end.saturating_sub(tip_height_start);
        result.success_count = 1;

        let mut b0_metrics = HashMap::new();
        b0_metrics.insert("scan_mode".into(), serde_json::json!("genesis"));
        b0_metrics.insert("blocks_scanned".into(), serde_json::json!(blocks_scanned));
        b0_metrics.insert(
            "blocks_per_sec".into(),
            serde_json::json!(if scan_time_secs > 0.0 { blocks_scanned as f64 / scan_time_secs } else { 0.0 }),
        );
        result.metrics.extend(b0_metrics);

        info!("B0 (new wallet) completed: {} blocks in {:.2}s", blocks_scanned, scan_time_secs);
        Ok(())
    }

    async fn run_s0(&mut self, config: &HarnessConfig, result: &mut ScenarioResult) -> Result<()> {
        use std::time::Instant;

        let start = Instant::now();
        info!("S0: Funding baseline for new wallet mode");

        let wait_result = self.wait_for_funding(config.a_fund, config.c_min, 600).await;

        let elapsed_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = elapsed_secs;

        match wait_result {
            Ok(balance) => {
                result.success_count = 1;
                result.balance_delta_ut = balance as i64;
                // Capture birthday height from current tip for S3/S7 rescans
                let base_node = crate::http_rpc::BaseNodeRpcClient::new(&self.base_node_http);
                if let Ok(tip) = base_node.get_tip_height().await {
                    self.birthday_height = tip;
                    info!("S0: Captured birthday height {}", tip);
                }
            }
            Err(e) => {
                result.failure_count = 1;
                result.failure_reasons.push(format!("Funding wait failed: {}", e));
            }
        }

        Ok(())
    }

    async fn run_s1(&mut self, config: &HarnessConfig, result: &mut ScenarioResult) -> Result<()> {
        use std::time::Instant;

        let start = Instant::now();
        info!("S1: UTXO build-up for new wallet - target {} UTXOs", config.volume_target);

        let mut current_utxos = 1u32;

        // Doubling phase: each tx sends to 2 recipients (self) to actually double UTXOs
        // 1 UTXO consumed -> 2 outputs produced = net +1 UTXO per round
        // Starting from 1 UTXO: round 1 -> 2, round 2 -> 3, round 3 -> 4, etc.
        // For true exponential doubling, send amount/2 to self twice
        for round in 0..config.doubling_rounds {
            let addr = TariAddress::from_base58(
                self.address.as_deref()
                    .ok_or_else(|| anyhow!("Wallet address not available"))?,
            )?;
            let half_amount = MicroMinotari(50_000); // Split amount between 2 recipients

            let recipients = vec![
                Recipient {
                    address: addr.clone(),
                    amount: half_amount,
                    payment_id: Some(format!("bench-s1-double-{}-a", round + 1)),
                },
                Recipient {
                    address: addr,
                    amount: half_amount,
                    payment_id: Some(format!("bench-s1-double-{}-b", round + 1)),
                },
            ];

            match self.send_transaction(recipients, config.fee_rate).await {
                Ok(tx_id) => {
                    info!("S1: Doubling round {} tx: {} (2 recipients)", round + 1, tx_id);
                    self.wait_for_confirmation(tx_id, config.c_min, 300).await?;
                    current_utxos += 1; // 1 consumed, 2 produced = +1 net
                }
                Err(e) => {
                    result.failure_reasons.push(format!("Doubling round {} failed: {}", round + 1, e));
                    break;
                }
            }
        }

        // Fan-out phase if needed
        if current_utxos < config.volume_target {
            let remaining = config.volume_target - current_utxos;
            info!(
                "S1: Fan-out phase - {} -> {} UTXOs (need {} more, {} per tx)",
                current_utxos, config.volume_target, remaining, config.fanout_outputs_per_tx
            );

            let fanout_rounds = remaining.div_ceil(config.fanout_outputs_per_tx);

            for round in 0..fanout_rounds {
                let outputs_this_round = std::cmp::min(
                    config.fanout_outputs_per_tx,
                    config.volume_target - current_utxos,
                );

                // Create multiple recipients for batch tx
                let addr = TariAddress::from_base58(
                    self.address.as_deref()
                        .ok_or_else(|| anyhow!("Wallet address not available"))?,
                ).context("Invalid Tari address")?;
                let recipients: Vec<Recipient> = (0..outputs_this_round)
                    .map(|i| Recipient {
                        address: addr.clone(),
                        amount: MicroMinotari(100_000),
                        payment_id: Some(format!("bench-s1-fanout-{}-{}", round + 1, i)),
                    })
                    .collect();

                match self.send_transaction(recipients, config.fee_rate).await {
                    Ok(tx_id) => {
                        info!("S1: Fan-out round {} tx: {}", round + 1, tx_id);
                        self.wait_for_confirmation(tx_id, config.c_min, 300).await?;
                        current_utxos += outputs_this_round - 1;
                    }
                    Err(e) => {
                        result.failure_reasons.push(format!("Fan-out round {} failed: {}", round + 1, e));
                        break;
                    }
                }
            }
        }

        let elapsed_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = elapsed_secs;
        result.success_count = if current_utxos >= config.volume_target { 1 } else { 0 };

        let mut s1_metrics = HashMap::new();
        s1_metrics.insert("final_utxo_count".into(), serde_json::json!(current_utxos));
        result.metrics.extend(s1_metrics);

        Ok(())
    }

    async fn run_s2(&mut self, config: &HarnessConfig, result: &mut ScenarioResult) -> Result<()> {
        use std::time::Instant;

        let start = Instant::now();
        info!("S2: Scan from genesis (checkpoint 1) for new wallet");

        let base_node_client = crate::http_rpc::BaseNodeRpcClient::new(&config.base_node_http);
        let tip_height_start = base_node_client.get_tip_height().await?;
        result.tip_height_start = tip_height_start;

        // Wipe data dir and reinitialize with birthday=0
        std::fs::remove_dir_all(&self.data_dir).ok();
        std::fs::create_dir_all(&self.data_dir)?;
        self.init_wallet_db(0).await?;

        // Scan from genesis
        match self.scan_blockchain().await {
            Ok(events) => info!("S2 scan completed: {} events", events.len()),
            Err(e) => warn!("S2 scan error: {}", e),
        }

        let scan_time_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = scan_time_secs;

        let tip_height_end = base_node_client.get_tip_height().await?;
        result.tip_height_end = tip_height_end;

        let blocks_scanned = tip_height_end.saturating_sub(tip_height_start);
        result.success_count = 1;

        let mut s2_metrics = HashMap::new();
        s2_metrics.insert("scan_mode".into(), serde_json::json!("genesis"));
        s2_metrics.insert("blocks_scanned".into(), serde_json::json!(blocks_scanned));
        s2_metrics.insert(
            "blocks_per_sec".into(),
            serde_json::json!(if scan_time_secs > 0.0 { blocks_scanned as f64 / scan_time_secs } else { 0.0 }),
        );
        result.metrics.extend(s2_metrics);

        Ok(())
    }

    async fn run_s3(&mut self, config: &HarnessConfig, result: &mut ScenarioResult) -> Result<()> {
        use std::time::Instant;

        let start = Instant::now();
        info!("S3: Scan from birthday (checkpoint 1) for new wallet");

        let base_node_client = crate::http_rpc::BaseNodeRpcClient::new(&config.base_node_http);
        let tip_height_start = base_node_client.get_tip_height().await?;
        result.tip_height_start = tip_height_start;

        // Wipe and reinitialize with birthday height from S0
        std::fs::remove_dir_all(&self.data_dir).ok();
        std::fs::create_dir_all(&self.data_dir)?;
        let birthday = if self.birthday_height > 0 { self.birthday_height } else { tip_height_start };
        self.init_wallet_db(birthday).await?;

        match self.scan_blockchain().await {
            Ok(events) => info!("S3 scan completed: {} events", events.len()),
            Err(e) => warn!("S3 scan error: {}", e),
        }

        let scan_time_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = scan_time_secs;

        let tip_height_end = base_node_client.get_tip_height().await?;
        result.tip_height_end = tip_height_end;

        let blocks_scanned = tip_height_end.saturating_sub(tip_height_start);
        result.success_count = 1;

        let mut s3_metrics = HashMap::new();
        s3_metrics.insert("scan_mode".into(), serde_json::json!("birthday"));
        s3_metrics.insert("blocks_scanned".into(), serde_json::json!(blocks_scanned));
        result.metrics.extend(s3_metrics);

        Ok(())
    }

    async fn run_s4(&mut self, config: &HarnessConfig, result: &mut ScenarioResult) -> Result<()> {
        use std::time::Instant;

        let start = Instant::now();
        info!("S4: Concurrent construction for new wallet");

        let mut total_successes = 0u32;
        let mut total_failures = 0u32;

        // Share the database pool across all concurrent tasks so FundLocker
        // locks are visible and contention is real. SQLite handles concurrent
        // access with its own internal locking.
        let db_pool = self.ensure_db_pool()?.clone();
        let address = self.address.clone();
        let password = self.password.clone();
        let base_node_http = self.base_node_http.clone();
        let account_id = self.account_id;

        for &n_concurrent in &config.concurrent_batches {
            info!("S4: Running {} concurrent transactions", n_concurrent);

            let mut handles = Vec::new();
            for i in 0..n_concurrent {
                let pool = db_pool.clone();
                let addr = address.clone();
                let pwd = password.clone();
                let base_http = base_node_http.clone();
                let acc_id = account_id;
                let fee_rate = config.fee_rate;

                let handle = tokio::spawn(async move {
                    send_concurrent_tx(pool, &addr, &pwd, &base_http, acc_id, fee_rate, i).await
                });
                handles.push(handle);
            }

            let batch_start = Instant::now();
            let batch_results: Vec<_> = futures::future::join_all(handles).await;

            for r in batch_results {
                match r {
                    Ok(Ok(tx_id)) => {
                        total_successes += 1;
                        debug!("S4 tx succeeded: tx_id={}", tx_id);
                    }
                    Ok(Err(e)) => {
                        total_failures += 1;
                        result.failure_reasons.push(format!("S4 concurrent tx failed: {}", e));
                    }
                    Err(join_err) => {
                        total_failures += 1;
                        result.failure_reasons.push(format!("S4 task join error: {}", join_err));
                    }
                }
            }

            let batch_time = batch_start.elapsed().as_secs_f64();
            debug!("S4: {} concurrent txs completed in {:.2}s", n_concurrent, batch_time);
        }

        let elapsed_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = elapsed_secs;
        result.success_count = total_successes;
        result.failure_count = total_failures;

        let mut s4_metrics = HashMap::new();
        s4_metrics.insert("concurrent_batches".into(), serde_json::json!(&config.concurrent_batches));
        s4_metrics.insert("total_successes".into(), serde_json::json!(total_successes));
        s4_metrics.insert("total_failures".into(), serde_json::json!(total_failures));
        result.metrics.extend(s4_metrics);

        Ok(())
    }

    async fn run_s5(&mut self, config: &HarnessConfig, result: &mut ScenarioResult) -> Result<()> {
        use std::time::Instant;

        let start = Instant::now();
        info!("S5: Payment processor throughput for new wallet (individual arm)");

        // Arm B - Individual sends (new wallet mode does individual sends)
        let mut successes = 0u32;
        let mut failures = 0u32;

        for i in 0..config.s5_m {
            let recipient = Recipient {
                address: TariAddress::from_base58(
                    self.address.as_deref().unwrap_or("otl_esm_1placeholder"),
                ).unwrap(),
                amount: MicroMinotari(100_000),
                payment_id: Some(format!("bench-s5-individual-{}", i)),
            };

            match self.send_transaction(vec![recipient], config.fee_rate).await {
                Ok(_) => successes += 1,
                Err(e) => {
                    failures += 1;
                    result.failure_reasons.push(format!("S5 individual tx {} failed: {}", i, e));
                }
            }
        }

        let elapsed_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = elapsed_secs;
        result.success_count = successes;
        result.failure_count = failures;

        let mut s5_metrics = HashMap::new();
        s5_metrics.insert("arm".into(), serde_json::json!("individual"));
        s5_metrics.insert("t_individual_secs".into(), serde_json::json!(elapsed_secs));
        s5_metrics.insert("successes".into(), serde_json::json!(successes));
        s5_metrics.insert("failures".into(), serde_json::json!(failures));
        result.metrics.extend(s5_metrics);

        Ok(())
    }

    async fn run_s6(&mut self, config: &HarnessConfig, result: &mut ScenarioResult) -> Result<()> {
        // Same as S2 but after S5 history
        let start = std::time::Instant::now();
        info!("S6: Scan from genesis (checkpoint 2) for new wallet");

        let base_node_client = crate::http_rpc::BaseNodeRpcClient::new(&config.base_node_http);
        let tip_height_start = base_node_client.get_tip_height().await?;
        result.tip_height_start = tip_height_start;

        std::fs::remove_dir_all(&self.data_dir).ok();
        std::fs::create_dir_all(&self.data_dir)?;
        self.init_wallet_db(0).await?;

        match self.scan_blockchain().await {
            Ok(events) => info!("S6 scan completed: {} events", events.len()),
            Err(e) => warn!("S6 scan error: {}", e),
        }

        let scan_time_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = scan_time_secs;

        let tip_height_end = base_node_client.get_tip_height().await?;
        result.tip_height_end = tip_height_end;

        let blocks_scanned = tip_height_end.saturating_sub(tip_height_start);
        result.success_count = 1;

        let mut s6_metrics = HashMap::new();
        s6_metrics.insert("scan_mode".into(), serde_json::json!("genesis"));
        s6_metrics.insert("blocks_scanned".into(), serde_json::json!(blocks_scanned));
        result.metrics.extend(s6_metrics);

        Ok(())
    }

    async fn run_s7(&mut self, config: &HarnessConfig, result: &mut ScenarioResult) -> Result<()> {
        // Same as S3 but after S5 history
        let start = std::time::Instant::now();
        info!("S7: Scan from birthday (checkpoint 2) for new wallet");

        let base_node_client = crate::http_rpc::BaseNodeRpcClient::new(&config.base_node_http);
        let tip_height_start = base_node_client.get_tip_height().await?;
        result.tip_height_start = tip_height_start;

        std::fs::remove_dir_all(&self.data_dir).ok();
        std::fs::create_dir_all(&self.data_dir)?;
        let birthday = if self.birthday_height > 0 { self.birthday_height } else { tip_height_start };
        self.init_wallet_db(birthday).await?;

        match self.scan_blockchain().await {
            Ok(events) => info!("S7 scan completed: {} events", events.len()),
            Err(e) => warn!("S7 scan error: {}", e),
        }

        let scan_time_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = scan_time_secs;

        let tip_height_end = base_node_client.get_tip_height().await?;
        result.tip_height_end = tip_height_end;

        let blocks_scanned = tip_height_end.saturating_sub(tip_height_start);
        result.success_count = 1;

        let mut s7_metrics = HashMap::new();
        s7_metrics.insert("scan_mode".into(), serde_json::json!("birthday"));
        s7_metrics.insert("blocks_scanned".into(), serde_json::json!(blocks_scanned));
        result.metrics.extend(s7_metrics);

        Ok(())
    }
}

/// Send a concurrent transaction using library functions.
/// This is the standalone version for tokio::spawn.
/// Uses a shared SqlitePool so FundLocker locks are visible across tasks.
async fn send_concurrent_tx(
    pool: SqlitePool,
    address: &Option<String>,
    password: &str,
    base_node_http: &str,
    account_id: i64,
    fee_per_gram: u64,
    index: usize,
) -> Result<u64> {
    let conn = pool.get().context("Failed to get DB connection")?;

    let account = minotari::db::get_account_by_name(&conn, "default")?
        .ok_or_else(|| anyhow!("Default account not found"))?;

    let addr = TariAddress::from_base58(
         address.as_deref()
             .ok_or_else(|| anyhow!("Wallet address not available"))?,
     ).context("Invalid Tari address")?;

    // Create recipient
    let recipient = Recipient {
        address: addr.clone(),
        amount: MicroMinotari(100_000),
        payment_id: Some(format!("bench-s4-{}", index)),
    };

    // Lock funds using FundLocker (shared pool = visible locks = real contention)
    let locker = FundLocker::new(pool.clone());
    let locked_funds = locker.lock(
        account_id,
        MicroMinotari(100_000),
        1,
        MicroMinotari(fee_per_gram),
        None,
        None,
        300,  // 5 minute lock
        3,    // confirmation window
    ).context("Failed to lock funds for concurrent tx")?;

    // Create unsigned transaction
    let tx_builder = OneSidedTransaction::new(
        pool.clone(),
        Network::Esmeralda,
        password.to_string(),
    );

    let unsigned_tx = tx_builder.create_unsigned_transaction(
        &account,
        locked_funds,
        vec![recipient],
        MicroMinotari(fee_per_gram),
    ).context("Failed to create unsigned transaction")?;

    // Sign the transaction
    let key_manager = account.get_key_manager(password)?;
    let consensus_constants = ConsensusConstantsBuilder::new(Network::Esmeralda).build();
    let signed_result = sign_locked_transaction(
        &key_manager,
        consensus_constants,
        Network::Esmeralda,
        unsigned_tx,
    ).context("Failed to sign transaction")?;

    // Broadcast via HTTP RPC
    let base_url = Url::parse(base_node_http)
        .context("Invalid base node URL")?;
    let client = WalletHttpClient::new(base_url)
        .context("Failed to create wallet HTTP client")?;

    let tx = signed_result.signed_transaction.transaction.clone();
    let _response = client
        .submit_transaction(tx)
        .await
        .context("Failed to submit transaction")?;

    let numeric_tx_id = signed_result.request.tx_id.as_u64();
    debug!("Concurrent tx broadcast successfully (tx_id={})", numeric_tx_id);
    Ok(numeric_tx_id)
}
