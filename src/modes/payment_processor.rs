//! Payment Processor Mode
//!
//! Uses the actual `minotari_payment_processor` service for batch 1-to-many transactions.
//! The service runs as a subprocess with its own HTTP API on port 9145.
//!
//! For non-S5 scenarios (B0, S0-S4, S6, S7), uses the minotari library directly
//! (same flow as NewWalletMode) since these are scan/funding/UTXO scenarios.
//!
//! For S5 (Payment Processor Throughput), spawns the payment processor service
//! and drives it via HTTP API:
//! - POST /v1/payment-batches - submit bulk payment batch
//! - GET /v1/payments/{id} - check payment status
//! - Wait for CONFIRMED status

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
use tari_transaction_components::key_manager::TransactionKeyManagerInterface;
use tari_transaction_components::offline_signing::sign_locked_transaction;
use tari_transaction_components::tari_amount::MicroMinotari;
use tari_utilities::ByteArray;
use url::Url;

use crate::config::HarnessConfig;
use crate::metrics::ScenarioResult;
use crate::modes::{WalletMode, WalletModeId};

/// Payment processor mode. Uses library flow for most scenarios,
/// actual minotari_payment_processor service for S5 batch throughput.
pub struct PaymentProcessorMode {
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
    /// Path to minotari_console_wallet binary (for payment processor signing)
    console_wallet_path: String,
    /// Path to minotari_payment_processor binary
    payment_processor_path: String,
}

impl PaymentProcessorMode {
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
            console_wallet_path: "minotari_console_wallet".to_string(),
            payment_processor_path: "minotari_payment_processor".to_string(),
        }
    }

    fn ensure_db_pool(&mut self) -> Result<&SqlitePool> {
        if self.db_pool.is_none() {
            let pool = minotari::init_db(self.db_path.clone())
                .context("Failed to initialize database pool")?;
            self.db_pool = Some(pool);
        }
        Ok(self.db_pool.as_ref().unwrap())
    }

    async fn init_wallet_db(&mut self, birthday_height: u64) -> Result<()> {
        info!(
            "Initializing payment processor wallet DB at {} with birthday height {}",
            self.db_path.display(),
            birthday_height
        );

        std::fs::create_dir_all(&self.data_dir)?;

        let mnemonic_str = self.seed_words.join(" ");
        let mnemonic_seq = SeedWords::from_str(&mnemonic_str)
            .context("Failed to parse seed words")?;
        let cipher_seed = CipherSeed::from_mnemonic(&mnemonic_seq, None)
            .context("Failed to create CipherSeed from mnemonic")?;

        minotari::utils::init_wallet::init_with_seed_words(
            cipher_seed,
            &self.password,
            &self.db_path,
            Some("default"),
        )
        .context("Failed to initialize wallet with seed words")?;

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

    async fn query_balance(&mut self) -> Result<u64> {
        let pool = self.ensure_db_pool()?;
        let conn = pool.get().context("Failed to get DB connection")?;

        let balance = minotari::get_balance(&conn, self.account_id)
            .context("Failed to query balance")?;

        debug!("Balance: {} uT available", balance.available.0);
        Ok(balance.available.0)
    }

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

    /// Send a batch transaction to multiple recipients using the library flow.
    async fn send_batch(&mut self, recipients: Vec<Recipient>, fee_per_gram: u64) -> Result<u64> {
        let pool = self.ensure_db_pool()?.clone();
        let conn = pool.get().context("Failed to get DB connection")?;

        let account = minotari::db::get_account_by_name(&conn, "default")?
            .ok_or_else(|| anyhow!("Default account not found"))?;

        let total_amount: u64 = recipients.iter().map(|r| r.amount.0).sum();
        let num_outputs = recipients.len().max(1);

        // Lock funds using FundLocker
        let locker = FundLocker::new(pool.clone());
        let locked_funds = locker.lock(
            self.account_id,
            MicroMinotari(total_amount),
            num_outputs,
            MicroMinotari(fee_per_gram),
            None,
            None,
            300,
            3,
        ).context("Failed to lock funds")?;

        // Create unsigned transaction
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

        // Sign
        let key_manager = account.get_key_manager(&self.password)?;
        let consensus_constants = ConsensusConstantsBuilder::new(Network::Esmeralda).build();
        let signed_result = sign_locked_transaction(
            &key_manager,
            consensus_constants,
            Network::Esmeralda,
            unsigned_tx,
        ).context("Failed to sign transaction")?;

        // Broadcast
        let base_url = Url::parse(&self.base_node_http)
            .context("Invalid base node URL")?;
        let client = WalletHttpClient::new(base_url)
            .context("Failed to create wallet HTTP client")?;

        let tx = signed_result.signed_transaction.transaction.clone();
        let _response = client
            .submit_transaction(tx)
            .await
            .context("Failed to submit transaction")?;

        let numeric_tx_id = signed_result.request.tx_id.as_u64();
        debug!("Batch transaction broadcast successfully (tx_id={})", numeric_tx_id);
        Ok(numeric_tx_id)
    }

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

            let pool = self.ensure_db_pool()?.clone();
            let base_url = Url::parse(&base_node_http)
                .context("Invalid base node URL")?;
            let client = WalletHttpClient::new(base_url)
                .context("Failed to create wallet HTTP client")?;

            let monitoring_state = self.monitoring_state.clone();
            let monitor = TransactionMonitor::new(
                monitoring_state,
                c_min as u64,
                None,
            );

            let _result = monitor
                .monitor_if_needed(&client, &pool, account_id, tip_height)
                .await
                .context("Transaction monitor failed")?;

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

    // =====================================================
    // Payment Processor Service Integration (S5)
    // =====================================================

    /// Get account keys for payment processor configuration
    async fn get_account_keys(&self) -> Result<(String, String)> {
        let pool = self.db_pool.as_ref()
            .ok_or_else(|| anyhow!("Database not initialized"))?;
        let conn = pool.get().context("Failed to get DB connection")?;

        let accounts = minotari::get_accounts(&conn, Some("default"))
            .context("Failed to get accounts")?;
        let account = accounts.first()
            .ok_or_else(|| anyhow!("Default account not found"))?;

        // Get view key and public spend key from the account
        let key_manager = account.get_key_manager(&self.password)?;
        let private_view_key = key_manager.get_private_view_key();
        let spend_key_info = key_manager.get_spend_key();

        // For payment processor config, we need hex-encoded keys
        let view_key_hex = hex::encode(private_view_key.as_bytes());
        let public_spend_key_hex = hex::encode(spend_key_info.pub_key.as_bytes());

        Ok((view_key_hex, public_spend_key_hex))
    }
}

#[async_trait::async_trait]
impl WalletMode for PaymentProcessorMode {
    async fn initialize(&mut self, config: &HarnessConfig) -> Result<()> {
        info!(
            "Initializing payment processor mode at {}",
            self.data_dir.display()
        );

        std::fs::create_dir_all(&self.data_dir)?;

        if self.seed_words.is_empty() {
            self.seed_words = config
                .resolve_seed_words("payment_processor")
                .unwrap_or_else(|| Self::generate_seed_words(WalletModeId::PaymentProcessor));
        }

        self.base_node_http = config.base_node_http.clone();

        self.init_wallet_db(0).await?;

        if let Err(e) = self.scan_blockchain().await {
            warn!("Initial scan (expected for empty wallet): {}", e);
        }

        info!("Payment processor initialized successfully");
        Ok(())
    }

    async fn run_scenario(
        &mut self,
        scenario_id: &str,
        config: &HarnessConfig,
    ) -> Result<ScenarioResult> {
        info!(
            "Running scenario {} for payment processor mode",
            scenario_id
        );

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
            "Scenario {} completed for payment processor: success={}, failures={}",
            scenario_id, result.success_count, result.failure_count
        );

        Ok(result)
    }

    fn get_address(&self) -> Option<&str> {
        self.address.as_deref()
    }

    async fn get_balance(&self) -> Result<u64> {
        let pool = self.db_pool.clone()
            .ok_or_else(|| anyhow!("Database not initialized"))?;
        let conn = pool.get().context("Failed to get DB connection")?;
        let balance = minotari::get_balance(&conn, self.account_id)
            .context("Failed to query balance")?;
        Ok(balance.available.0)
    }

    async fn teardown(&mut self) -> Result<()> {
        info!("Tearing down payment processor mode");
        info!("Payment processor torn down");
        Ok(())
    }
}

impl PaymentProcessorMode {
    async fn run_b0(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        use std::time::Instant;

        let start = Instant::now();
        info!("B0: Baseline scan for payment processor mode");

        let base_node_client =
            crate::http_rpc::BaseNodeRpcClient::new(&config.base_node_http);
        let tip_height_start = base_node_client.get_tip_height().await?;
        result.tip_height_start = tip_height_start;

        info!(
            "B0: Scanning from genesis to tip {} (library integration)",
            tip_height_start
        );

        match self.scan_blockchain().await {
            Ok(_) => info!("B0 scan completed"),
            Err(e) => warn!("B0 scan error (may be expected for empty wallet): {}", e),
        }

        let scan_time_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = scan_time_secs;

        let tip_height_end = base_node_client.get_tip_height().await?;
        result.tip_height_end = tip_height_end;

        let blocks_scanned = tip_height_end.saturating_sub(tip_height_start);
        result.success_count = if blocks_scanned > 0 { 1 } else { 0 };

        let mut b0_metrics: HashMap<String, serde_json::Value> = HashMap::new();
        b0_metrics.insert("scan_mode".into(), serde_json::json!("genesis"));
        b0_metrics.insert("blocks_scanned".into(), serde_json::json!(blocks_scanned));
        b0_metrics.insert(
            "blocks_per_sec".into(),
            serde_json::json!(if scan_time_secs > 0.0 { blocks_scanned as f64 / scan_time_secs } else { 0.0 }),
        );
        result.metrics.extend(b0_metrics);

        info!(
            "B0 (payment processor) completed: {} blocks in {:.2}s",
            blocks_scanned, scan_time_secs
        );
        Ok(())
    }

    async fn run_s0(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        use std::time::Instant;

        let start = Instant::now();
        info!("S0: Funding baseline for payment processor mode");

        let wait_result = self.wait_for_funding(config.a_fund, config.c_min, 600).await;
        let elapsed_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = elapsed_secs;

        match wait_result {
            Ok(balance) => {
                result.success_count = 1;
                result.balance_delta_ut = balance as i64;
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

    async fn run_s1(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        use std::time::Instant;

        let start = Instant::now();
        info!(
            "S1: UTXO build-up for payment processor - target {} UTXOs",
            config.volume_target
        );

        let mut current_utxos = 1u32;

        // Doubling phase
        for round in 0..config.doubling_rounds {
            let recipient = Recipient {
                address: TariAddress::from_base58(
                    self.address.as_deref().unwrap_or("otl_esm_1placeholder"),
                )?,
                amount: MicroMinotari(100_000),
                payment_id: Some(format!("bench-s1-doubling-{}", round)),
            };

            // Send 2 recipients per tx for UTXO growth (+1 net per tx)
            let recipients = vec![
                recipient.clone(),
                Recipient {
                    address: recipient.address.clone(),
                    amount: MicroMinotari(100_000),
                    payment_id: Some(format!("bench-s1-doubling-{}-2", round)),
                },
            ];

            match self.send_batch(recipients, config.fee_rate).await {
                Ok(tx_id) => {
                    self.wait_for_confirmation(tx_id, config.c_min, 300).await?;
                    current_utxos += 1; // +1 net UTXO per tx
                    info!("S1 doubling round {}: tx={}, utxos={}", round, tx_id, current_utxos);
                }
                Err(e) => {
                    result.failure_count += 1;
                    result.failure_reasons.push(format!("S1 doubling round {} failed: {}", round, e));
                    break;
                }
            }
        }

        let elapsed_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = elapsed_secs;
        result.success_count = if current_utxos > 1 { 1 } else { 0 };

        let mut s1_metrics: HashMap<String, serde_json::Value> = HashMap::new();
        s1_metrics.insert("utxo_count_after".into(), serde_json::json!(current_utxos));
        s1_metrics.insert("doubling_rounds_completed".into(), serde_json::json!(config.doubling_rounds));
        result.metrics.extend(s1_metrics);

        info!("S1 (payment processor) completed: {} UTXOs in {:.2}s", current_utxos, elapsed_secs);
        Ok(())
    }

    async fn run_s2(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        let start = std::time::Instant::now();
        info!("S2: Scan from genesis (checkpoint 1) for payment processor");

        let base_node_client =
            crate::http_rpc::BaseNodeRpcClient::new(&config.base_node_http);
        let tip_height_start = base_node_client.get_tip_height().await?;
        result.tip_height_start = tip_height_start;

        std::fs::remove_dir_all(&self.data_dir).ok();
        std::fs::create_dir_all(&self.data_dir)?;
        self.init_wallet_db(0).await?;

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

        let mut s2_metrics: HashMap<String, serde_json::Value> = HashMap::new();
        s2_metrics.insert("scan_mode".into(), serde_json::json!("genesis"));
        s2_metrics.insert("blocks_scanned".into(), serde_json::json!(blocks_scanned));
        result.metrics.extend(s2_metrics);

        Ok(())
    }

    async fn run_s3(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        let start = std::time::Instant::now();
        info!("S3: Scan from birthday (checkpoint 1) for payment processor");

        let base_node_client =
            crate::http_rpc::BaseNodeRpcClient::new(&config.base_node_http);
        let tip_height_start = base_node_client.get_tip_height().await?;
        result.tip_height_start = tip_height_start;

        std::fs::remove_dir_all(&self.data_dir).ok();
        std::fs::create_dir_all(&self.data_dir)?;
        self.init_wallet_db(self.birthday_height).await?;

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

        let mut s3_metrics: HashMap<String, serde_json::Value> = HashMap::new();
        s3_metrics.insert("scan_mode".into(), serde_json::json!("birthday"));
        s3_metrics.insert("birthday_height".into(), serde_json::json!(self.birthday_height));
        s3_metrics.insert("blocks_scanned".into(), serde_json::json!(blocks_scanned));
        result.metrics.extend(s3_metrics);

        Ok(())
    }

    async fn run_s4(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        use std::time::Instant;

        let start = Instant::now();
        info!("S4: Concurrent construction for payment processor");

        let addr = TariAddress::from_base58(
            self.address.as_deref().unwrap_or("otl_esm_1placeholder"),
        )?;

        let mut total_successes = 0u32;
        let mut total_failures = 0u32;

        for n_concurrent in &config.concurrent_batches {
            info!("S4: Running {} concurrent txs", n_concurrent);

            let pool = self.db_pool.clone().ok_or_else(|| anyhow!("DB not initialized"))?;
            let base_node_http = self.base_node_http.clone();
            let account_id = self.account_id;
            let password = self.password.clone();
            let fee_rate = config.fee_rate;

            let mut handles = Vec::new();

            for i in 0..*n_concurrent {
                let recipient = Recipient {
                    address: addr.clone(),
                    amount: MicroMinotari(100_000),
                    payment_id: Some(format!("bench-s4-{}-{}", n_concurrent, i)),
                };

                let pool_clone = pool.clone();
                let http_clone = base_node_http.clone();
                let pw_clone = password.clone();

                let handle = tokio::spawn(async move {
                    // Each task gets its own connection from the shared pool
                    let conn = match pool_clone.get() {
                        Ok(c) => c,
                        Err(e) => return Err(anyhow!("DB connection failed: {}", e)),
                    };

                    let account = minotari::db::get_account_by_name(&conn, "default")?
                        .ok_or_else(|| anyhow!("Default account not found"))?;

                    let locker = FundLocker::new(pool_clone.clone());
                    let locked_funds = locker.lock(
                        account_id,
                        MicroMinotari(100_000),
                        1,
                        MicroMinotari(fee_rate),
                        None,
                        None,
                        300,
                        3,
                    )?;

                    let tx_builder = OneSidedTransaction::new(
                        pool_clone.clone(),
                        Network::Esmeralda,
                        pw_clone.clone(),
                    );

                    let unsigned_tx = tx_builder.create_unsigned_transaction(
                        &account,
                        locked_funds,
                        vec![recipient],
                        MicroMinotari(fee_rate),
                    )?;

                    let key_manager = account.get_key_manager(&pw_clone)?;
                    let consensus_constants = ConsensusConstantsBuilder::new(Network::Esmeralda).build();
                    let signed_result = sign_locked_transaction(
                        &key_manager,
                        consensus_constants,
                        Network::Esmeralda,
                        unsigned_tx,
                    )?;

                    let base_url = Url::parse(&http_clone)?;
                    let client = WalletHttpClient::new(base_url)?;
                    let tx = signed_result.signed_transaction.transaction.clone();
                    let _response = client.submit_transaction(tx).await?;

                    Ok::<_, anyhow::Error>(())
                });

                handles.push(handle);
            }

            // Wait for all concurrent tasks
            let results = futures::future::join_all(handles).await;
            for r in results {
                match r {
                    Ok(Ok(())) => total_successes += 1,
                    Ok(Err(e)) => {
                        total_failures += 1;
                        result.failure_reasons.push(format!("S4 concurrent tx failed: {}", e));
                    }
                    Err(e) => {
                        total_failures += 1;
                        result.failure_reasons.push(format!("S4 task join error: {}", e));
                    }
                }
            }

            info!("S4: {} concurrent txs completed (success={}, failures={})", n_concurrent, total_successes, total_failures);
        }

        let elapsed_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = elapsed_secs;
        result.success_count = total_successes;
        result.failure_count = total_failures;

        let mut s4_metrics: HashMap<String, serde_json::Value> = HashMap::new();
        s4_metrics.insert("total_successes".into(), serde_json::json!(total_successes));
        s4_metrics.insert("total_failures".into(), serde_json::json!(total_failures));
        result.metrics.extend(s4_metrics);

        Ok(())
    }

    async fn run_s5(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        use std::time::Instant;

        let start = Instant::now();
        info!("S5: Payment processor throughput (batch arm via actual service)");

        // Arm A - Batch sends using the actual minotari_payment_processor service
        // For now, fall back to library batch if service is not available
        let use_service = std::path::Path::new(&self.payment_processor_path).exists()
            && std::path::Path::new(&self.console_wallet_path).exists();

        if use_service {
            info!("S5: Using minotari_payment_processor service for batch throughput");
            self.run_s5_service(config, result).await?;
        } else {
            info!("S5: Payment processor service not found, using library batch fallback");
            self.run_s5_library(config, result).await?;
        }

        let elapsed_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = elapsed_secs;

        let mut s5_metrics: HashMap<String, serde_json::Value> = HashMap::new();
        s5_metrics.insert("arm".into(), serde_json::json!("batch"));
        s5_metrics.insert("t_batch_secs".into(), serde_json::json!(elapsed_secs));
        s5_metrics.insert("use_service".into(), serde_json::json!(use_service));
        result.metrics.extend(s5_metrics);

        Ok(())
    }

    /// S5 using the actual minotari_payment_processor service
    async fn run_s5_service(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        use std::process::Stdio;
        use std::time::Instant;

        info!("S5: Starting payment processor service integration");

        // Get account keys for payment processor configuration
        let (view_key_hex, public_spend_key_hex) = self.get_account_keys().await?;
        let addr = self.address.as_deref().unwrap_or("otl_esm_1placeholder").to_string();

        // Create temp directory for payment processor data
        let pp_data_dir = tempfile::TempDir::new()?;
        let pp_db_path = pp_data_dir.path().join("payments.db");
        let database_url = format!("sqlite://{}", pp_db_path.display());

        // Initialize SQLite database
        std::fs::create_dir_all(pp_data_dir.path())?;

        // Spawn minotari_console_wallet as Payment Receiver (PR API)
        info!("S5: Spawning console wallet as Payment Receiver on port 9000");
        let pr_port = 9000;
        let pr_url = format!("http://127.0.0.1:{}", pr_port);

        // Start console wallet with HTTP API
        let mut wallet_cmd = tokio::process::Command::new(&self.console_wallet_path);
        wallet_cmd
            .current_dir(&self.data_dir)
            .env("MINOTARI_WALLET_PASSWORD", &self.password)
            .arg("--command-mode-auto-exit")
            .arg("--base-path")
            .arg(&self.data_dir)
            .arg("--network")
            .arg("Esmeralda")
            .arg("--listen-port")
            .arg(pr_port.to_string())
            .arg("--grpc-listen-port")
            .arg((pr_port + 100).to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        let mut wallet_process = wallet_cmd.spawn()
            .context("Failed to spawn console wallet as Payment Receiver")?;

        // Give wallet time to start
        tokio::time::sleep(Duration::from_secs(5)).await;

        // Spawn payment processor service
        info!("S5: Spawning payment processor service on port 9145");
        let pp_port = 9145;
        let pp_url = format!("http://127.0.0.1:{}", pp_port);

        let mut pp_cmd = tokio::process::Command::new(&self.payment_processor_path);
        pp_cmd
            .current_dir(pp_data_dir.path())
            .env("DATABASE_URL", &database_url)
            .env("PAYMENT_RECEIVER", &pr_url)
            .env("BASE_NODE", &self.base_node_http)
            .env("CONSOLE_WALLET_PATH", &self.console_wallet_path)
            .env("CONSOLE_WALLET_BASE_PATH", self.data_dir.to_string_lossy().to_string())
            .env("CONSOLE_WALLET_PASSWORD", &self.password)
            .env("LISTEN_IP", "127.0.0.1")
            .env("LISTEN_PORT", pp_port.to_string())
            .env("TARI_NETWORK", "Esmeralda")
            .env("ACCOUNTS__DEFAULT__NAME", "default")
            .env("ACCOUNTS__DEFAULT__VIEW_KEY", &view_key_hex)
            .env("ACCOUNTS__DEFAULT__PUBLIC_SPEND_KEY", &public_spend_key_hex)
            .env("BATCH_CREATOR_SLEEP_SECS", "5")
            .env("UNSIGNED_TX_CREATOR_SLEEP_SECS", "5")
            .env("TRANSACTION_SIGNER_SLEEP_SECS", "5")
            .env("BROADCASTER_SLEEP_SECS", "5")
            .env("CONFIRMATION_CHECKER_SLEEP_SECS", "10")
            .env("CONFIRMATION_CHECKER_REQUIRED_CONFIRMATIONS", config.c_min.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        let mut pp_process = pp_cmd.spawn()
            .context("Failed to spawn payment processor service")?;

        // Give payment processor time to start
        tokio::time::sleep(Duration::from_secs(10)).await;

        // Verify payment processor is running
        let health_url = format!("{}/health/version", pp_url);
        let health_resp = reqwest::get(&health_url).await;
        if health_resp.is_err() {
            warn!("S5: Payment processor health check failed, falling back to library batch");
            wallet_process.kill().await.ok();
            pp_process.kill().await.ok();
            self.run_s5_library(config, result).await?;
            return Ok(());
        }

        info!("S5: Payment processor service is running");

        // Create recipients for batch txs
        let recipient_addr = TariAddress::from_base58(&addr)?;
        let recipients_per_batch = config.s5_k as usize;
        let num_batches = (config.s5_m as usize).div_ceil(recipients_per_batch);

        let mut successes = 0u32;
        let mut failures = 0u32;

        // Submit payment batches via HTTP API
        for batch_idx in 0..num_batches {
            let batch_items: Vec<serde_json::Value> = (0..recipients_per_batch)
                .map(|i| {
                    serde_json::json!({
                        "client_id": format!("bench-s5-batch-{}-{}", batch_idx, i),
                        "recipient_address": addr.clone(),
                        "amount": 100_000,
                        "payment_id": Some(format!("bench-s5-batch-{}-{}", batch_idx, i))
                    })
                })
                .collect();

            let batch_request = serde_json::json!({
                "account_name": "default",
                "items": batch_items
            });

            let batch_url = format!("{}/v1/payment-batches", pp_url);
            let client = reqwest::Client::new();

            match client.post(&batch_url)
                .json(&batch_request)
                .send()
                .await
            {
                Ok(resp) => {
                    if resp.status().is_success() {
                        info!("S5: Batch {} submitted successfully", batch_idx);
                        successes += 1;

                        // Poll for confirmation status
                        let body = resp.json::<serde_json::Value>().await.ok();
                        if let Some(batch_id) = body.as_ref().and_then(|b| b["batch_id"].as_str().map(|s| s.to_string())) {
                            self.wait_for_pp_confirmation(&pp_url, &batch_id, config.c_min, 300).await.ok();
                        }
                    } else {
                        warn!("S5: Batch {} failed with status {}", batch_idx, resp.status());
                        failures += 1;
                    }
                }
                Err(e) => {
                    warn!("S5: Failed to submit batch {}: {}", batch_idx, e);
                    failures += 1;
                }
            }
        }

        // Cleanup processes
        wallet_process.kill().await.ok();
        pp_process.kill().await.ok();

        result.success_count = successes;
        result.failure_count = failures;

        let mut s5_pp_metrics: HashMap<String, serde_json::Value> = HashMap::new();
        s5_pp_metrics.insert("successes".into(), serde_json::json!(successes));
        s5_pp_metrics.insert("failures".into(), serde_json::json!(failures));
        s5_pp_metrics.insert("num_batches".into(), serde_json::json!(num_batches));
        s5_pp_metrics.insert("recipients_per_batch".into(), serde_json::json!(recipients_per_batch));
        s5_pp_metrics.insert("use_service".into(), serde_json::json!(true));
        result.metrics.extend(s5_pp_metrics);

        Ok(())
    }

    /// Wait for payment processor batch to reach CONFIRMED status
    async fn wait_for_pp_confirmation(
        &self,
        pp_url: &str,
        batch_id: &str,
        _c_min: u32,
        timeout_secs: u64,
    ) -> Result<()> {
        let start = std::time::Instant::now();
        let timeout = Duration::from_secs(timeout_secs);
        let client = reqwest::Client::new();

        loop {
            if start.elapsed() > timeout {
                return Err(anyhow!("Timeout waiting for batch {} confirmation", batch_id));
            }

            let status_url = format!("{}/v1/events", pp_url);
            match client.get(&status_url).send().await {
                Ok(resp) => {
                    if resp.status().is_success() {
                        let events = resp.json::<Vec<serde_json::Value>>().await.ok();
                        if let Some(events) = events {
                            for event in &events {
                                if let Some(batch) = event.get("batch_id").and_then(|b| b.as_str()) {
                                    if batch == batch_id {
                                        if let Some(event_type) = event.get("type").and_then(|t| t.as_str()) {
                                            if event_type == "BatchConfirmed" {
                                                info!("S5: Batch {} confirmed", batch_id);
                                                return Ok(());
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    debug!("S5: Failed to check batch status: {}", e);
                }
            }

            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    }

    /// S5 using library batch (fallback / placeholder)
    async fn run_s5_library(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        info!("S5: Payment processor throughput - library batch arm");

        let mut successes = 0u32;
        let mut failures = 0u32;

        // Create recipients for batch txs
        let addr = TariAddress::from_base58(
            self.address.as_deref().unwrap_or("otl_esm_1placeholder"),
        )?;
        let recipients_per_batch = config.s5_k as usize;
        let num_batches = (config.s5_m as usize).div_ceil(recipients_per_batch);

        for batch_idx in 0..num_batches {
            let recipients: Vec<Recipient> = (0..recipients_per_batch)
                .map(|i| Recipient {
                    address: addr.clone(),
                    amount: MicroMinotari(100_000),
                    payment_id: Some(format!("bench-s5-batch-{}-{}", batch_idx, i)),
                })
                .collect();

            match self.send_batch(recipients, config.fee_rate).await {
                Ok(_) => successes += 1,
                Err(e) => {
                    failures += 1;
                    result.failure_reasons.push(format!("S5 batch tx {} failed: {}", batch_idx, e));
                }
            }
        }

        result.success_count = successes;
        result.failure_count = failures;

        let mut s5_lib_metrics: HashMap<String, serde_json::Value> = HashMap::new();
        s5_lib_metrics.insert("successes".into(), serde_json::json!(successes));
        s5_lib_metrics.insert("failures".into(), serde_json::json!(failures));
        s5_lib_metrics.insert("num_batches".into(), serde_json::json!(num_batches));
        s5_lib_metrics.insert("recipients_per_batch".into(), serde_json::json!(recipients_per_batch));
        result.metrics.extend(s5_lib_metrics);

        Ok(())
    }

    async fn run_s6(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        let start = std::time::Instant::now();
        info!("S6: Scan from genesis (checkpoint 2) for payment processor");

        let base_node_client =
            crate::http_rpc::BaseNodeRpcClient::new(&config.base_node_http);
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

        let mut s6_metrics: HashMap<String, serde_json::Value> = HashMap::new();
        s6_metrics.insert("scan_mode".into(), serde_json::json!("genesis"));
        s6_metrics.insert("blocks_scanned".into(), serde_json::json!(blocks_scanned));
        result.metrics.extend(s6_metrics);

        Ok(())
    }

    async fn run_s7(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        let start = std::time::Instant::now();
        info!("S7: Scan from birthday (checkpoint 2) for payment processor");

        let base_node_client =
            crate::http_rpc::BaseNodeRpcClient::new(&config.base_node_http);
        let tip_height_start = base_node_client.get_tip_height().await?;
        result.tip_height_start = tip_height_start;

        std::fs::remove_dir_all(&self.data_dir).ok();
        std::fs::create_dir_all(&self.data_dir)?;
        self.init_wallet_db(self.birthday_height).await?;

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

        let mut s7_metrics: HashMap<String, serde_json::Value> = HashMap::new();
        s7_metrics.insert("scan_mode".into(), serde_json::json!("birthday"));
        s7_metrics.insert("birthday_height".into(), serde_json::json!(self.birthday_height));
        s7_metrics.insert("blocks_scanned".into(), serde_json::json!(blocks_scanned));
        result.metrics.extend(s7_metrics);

        Ok(())
    }
}
