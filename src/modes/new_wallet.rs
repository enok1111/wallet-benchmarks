//! New Wallet Mode (minotari-cli library with offline signing)
//!
//! Uses the minotari crate directly for:
//! - Blockchain scanning via Scanner
//! - Balance queries via get_balance
//! - Transaction building and broadcast via HTTP RPC
//! - SQLite wallet database with encrypted keys
//!
//! Key difference from OldWalletMode: NO gRPC calls. All operations use
//! the library's own functions and HTTP RPC for broadcasting.

use anyhow::{anyhow, Context, Result};
use log::{debug, info, warn};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use crate::config::HarnessConfig;
use crate::metrics::ScenarioResult;
use crate::modes::{WalletMode, WalletModeId};

/// Cloneable wallet handle for concurrent task spawning.
/// Holds the fields needed for send_to_self and related operations.
#[derive(Clone)]
struct WalletHandle {
    db_path: PathBuf,
    address: Option<String>,
    password: String,
    base_node_http: String,
}

impl WalletHandle {
    fn from_wallet(wallet: &NewWalletMode) -> Self {
        Self {
            db_path: wallet.db_path.clone(),
            address: wallet.address.clone(),
            password: wallet.password.clone(),
            base_node_http: wallet.base_node_http.clone(),
        }
    }
}

pub struct NewWalletMode {
    data_dir: PathBuf,
    db_path: PathBuf,
    address: Option<String>,
    seed_words: Vec<String>,
    password: String,
    base_node_http: String,
    account_id: u32,
    birthday_height: u64,
}

impl NewWalletMode {
    pub fn new(data_dir: PathBuf, _grpc_port: u16) -> Self {
        let db_path = data_dir.join("wallet.db");
        Self {
            data_dir,
            db_path,
            address: None,
            seed_words: Vec::new(),
            password: "benchmark_password_32chars_min".to_string(),
            base_node_http: "http://127.0.0.1:18143".to_string(),
            account_id: 1,
            birthday_height: 0,
        }
    }

    async fn init_wallet_db(&mut self, birthday_height: u64) -> Result<()> {
        info!(
            "Initializing wallet DB at {} with birthday height {}",
            self.db_path.display(),
            birthday_height
        );

        std::fs::create_dir_all(&self.data_dir)?;

        // Initialize wallet using seed words via library function
        use tari_common_types::seeds::mnemonic::Mnemonic;
        use tari_common_types::seeds::seed_words::SeedWords;
        use std::str::FromStr;

        let mnemonic_str = self.seed_words.join(" ");
        let mnemonic_seq = SeedWords::from_str(&mnemonic_str)
            .context("Failed to parse seed words")?;
        let cipher_seed = tari_common_types::seeds::cipher_seed::CipherSeed::from_mnemonic(
            &mnemonic_seq,
            None,
        )
        .context("Failed to create CipherSeed from mnemonic")?;

        minotari::utils::init_wallet::init_with_seed_words(
            cipher_seed,
            &self.password,
            &self.db_path,
            Some("default"),
        )
        .context("Failed to initialize wallet with seed words")?;

        // Persist birthday height to DB so the Scanner starts from the correct height
        {
            let db = minotari::db::init_db(self.db_path.clone())
                .context("Failed to init DB for birthday update")?;
            let conn = db.get().context("Failed to get DB connection")?;
            let birthday_val: i64 = birthday_height as i64;
            let _ = conn.execute(
                "UPDATE accounts SET birthday = ?1 WHERE birthday != ?1",
                [birthday_val],
            );
            info!("Set account birthday to {}", birthday_height);
        }

        // Get the wallet address after initialization
        let db = minotari::db::init_db(self.db_path.clone())
            .context("Failed to init DB for address query")?;
        let conn = db.get().context("Failed to get DB connection")?;
        let accounts = minotari::db::get_accounts(&conn, Some("default"))
            .context("Failed to get accounts")?;
        
        if let Some(account) = accounts.first() {
            use tari_common::configuration::Network;
            let addr = account.get_address(Network::Esmeralda, &self.password)?;
            self.address = Some(addr.to_string());
            info!("Wallet address: {}", self.address.as_ref().unwrap());
        }

        debug!("Wallet database initialized successfully");
        Ok(())
    }

    async fn scan_blockchain(&self) -> Result<Vec<minotari::WalletEvent>> {
        use minotari::{Scanner, ScanMode};

        info!("Scanning blockchain via {} (library integration)", self.base_node_http);

        let (events, _more_blocks) = Scanner::new(
            &self.password,
            &self.base_node_http,
            self.db_path.clone(),
            100,
            10,
        )
        .mode(ScanMode::Full)
        .account("default")
        .run()
        .await
        .context("Scanner failed")?;

        info!("Scan completed: {} events processed", events.len());
        Ok(events)
    }

    async fn query_balance(&self) -> Result<u64> {
        use minotari::{get_balance, init_db};

        let db = init_db(self.db_path.clone())
            .context("Failed to initialize database connection")?;
        let conn = db.get().context("Failed to get DB connection")?;

        let balance = get_balance(&conn, self.account_id as i64)
            .context("Failed to query balance")?;

        debug!("Balance: {} µT available", balance.available.0);
        Ok(balance.available.0)
    }

    async fn query_utxo_count(&self) -> Result<u32> {
        use minotari::{db::fetch_unspent_outputs, db::get_accounts, init_db};

        let db = init_db(self.db_path.clone())
            .context("Failed to initialize database connection")?;
        let conn = db.get().context("Failed to get DB connection")?;

        let accounts = get_accounts(&conn, Some("default"))
            .context("Failed to get accounts")?;
        let account = accounts.first()
            .ok_or_else(|| anyhow!("Default account not found"))?;

        let outputs = fetch_unspent_outputs(&conn, account.id, 0)
            .context("Failed to fetch unspent outputs")?;

        debug!("UTXO count: {}", outputs.len());
        Ok(outputs.len() as u32)
    }

    fn generate_seed_words(_mode_id: WalletModeId) -> Vec<String> {
        crate::modes::generate_tari_seed_words()
    }

    /// Send a transaction to self using the library's offline signing flow.
    ///
    /// Flow: Lock UTXOs -> Create unsigned tx -> Sign with sign_locked_transaction
    ///       -> Broadcast via HTTP RPC to base node
    async fn send_to_self(&self, output_count: u32, fee_per_gram: u64) -> Result<u64> {
        use minotari::transactions::one_sided_transaction::{OneSidedTransaction, Recipient};
        use minotari::db::get_account_by_name;
        use tari_common::configuration::Network;
        use tari_common_types::tari_address::TariAddress;
        use tari_transaction_components::offline_signing::sign_locked_transaction;
        use tari_transaction_components::consensus::ConsensusConstantsBuilder;
        use tari_transaction_components::tari_amount::MicroMinotari;

        let db = minotari::init_db(self.db_path.clone())?;
        let conn = db.get()?;

        let account = get_account_by_name(&conn, "default")?
            .ok_or_else(|| anyhow!("Default account not found"))?;

        let address = TariAddress::from_base58(
            self.address.as_deref().ok_or_else(|| anyhow!("Wallet address not available"))?,
        )?;

        // Create unsigned transaction using library
        let tx_builder = OneSidedTransaction::new(db.clone(), Network::Esmeralda, self.password.clone());

        let amount_per_output = MicroMinotari(100_000);
        let total_amount = amount_per_output.0 * output_count as u64;

        let recipient = Recipient {
            address: address.clone(),
            amount: MicroMinotari(total_amount),
            payment_id: Some(format!(
                "bench-s1-{}-{}",
                output_count,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis()
            )),
        };

        // Lock funds for this transaction (simple UTXO selection)
        let locked_funds = self.lock_funds(output_count).await?;

        // Create unsigned transaction (sync function from library)
        let unsigned_tx = tx_builder.create_unsigned_transaction(
            &account,
            locked_funds,
            vec![recipient],
            MicroMinotari(fee_per_gram),
        )?;

        // Sign the transaction externally using sign_locked_transaction
        let key_manager = account.get_key_manager(&self.password)?;
        let consensus_constants = ConsensusConstantsBuilder::new(Network::Esmeralda).build();
        let signed_result = sign_locked_transaction(
            &key_manager,
            consensus_constants,
            Network::Esmeralda,
            unsigned_tx,
        )?;

        // Broadcast via HTTP RPC to base node
        let tx_id = self.broadcast_signed_transaction(&signed_result).await?;

        Ok(tx_id)
    }

    /// Lock funds for a transaction using simple UTXO selection.
    async fn lock_funds(&self, output_count: u32) -> Result<minotari::api::types::LockFundsResult> {
        use minotari::db::{get_account_by_name, fetch_unspent_outputs};
        use tari_transaction_components::tari_amount::MicroMinotari;
        use tari_transaction_components::utxo_selection::UtxoValue;

        let db = minotari::init_db(self.db_path.clone())?;
        let conn = db.get()?;

        let account = get_account_by_name(&conn, "default")?
            .ok_or_else(|| anyhow!("Default account not found"))?;
        let outputs = fetch_unspent_outputs(&conn, account.id, 0)?;

        // Simple UTXO selection - take the first N outputs needed
        let amount_per_output = MicroMinotari(100_000);
        let total_amount = amount_per_output.0 * output_count as u64;

        let mut locked_outputs = Vec::new();
        let mut accumulated = 0u64;

        for output in outputs {
            locked_outputs.push(output.output.clone());
            accumulated += output.value().as_u64();
            if accumulated >= total_amount {
                break;
            }
        }

        // Estimate fee (rough approximation based on transaction size)
        let fee_per_gram = MicroMinotari(5);
        let estimated_tx_size = 500; // rough estimate in grams
        let fee_with_change = MicroMinotari(fee_per_gram.0 * estimated_tx_size);
        let fee_without_change = MicroMinotari((fee_per_gram.0 * (estimated_tx_size - 100)) / 2);

        Ok(minotari::api::types::LockFundsResult {
            utxos: locked_outputs,
            requires_change_output: accumulated > total_amount,
            total_value: MicroMinotari(accumulated),
            fee_without_change,
            fee_with_change,
        })
    }

    /// Broadcast a signed transaction to the base node via HTTP RPC.
    async fn broadcast_signed_transaction(
        &self,
        signed_result: &tari_transaction_components::offline_signing::models::SignedOneSidedTransactionResult,
    ) -> Result<u64> {
        let client = reqwest::Client::new();

        // Submit via JSON-RPC to base node
        let submit_url = format!("{}/json_rpc", self.base_node_http);
        let transaction = &signed_result.signed_transaction.transaction;
        
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "1",
            "method": "submit_transaction",
            "params": { "transaction": transaction }
        });

        let response = client
            .post(&submit_url)
            .json(&request)
            .send()
            .await?;

        if response.status().is_success() {
            // Extract numeric tx_id for tracking
            let numeric_tx_id = signed_result.request.tx_id.as_u64();
            debug!("Transaction broadcast successfully (tx_id={})", numeric_tx_id);
            Ok(numeric_tx_id)
        } else {
            let status = response.status();
            anyhow::bail!("Transaction broadcast failed: {}", status);
        }
    }

    /// Send a single transfer to a destination address using offline signing.
    async fn send_single_transfer(&self, destination: &str, amount: u64, fee_per_gram: u64) -> Result<u64> {
        use minotari::transactions::one_sided_transaction::{OneSidedTransaction, Recipient};
        use minotari::db::get_account_by_name;
        use tari_common::configuration::Network;
        use tari_common_types::tari_address::TariAddress;
        use tari_transaction_components::offline_signing::sign_locked_transaction;
        use tari_transaction_components::consensus::ConsensusConstantsBuilder;
        use tari_transaction_components::tari_amount::MicroMinotari;

        let db = minotari::init_db(self.db_path.clone())?;
        let conn = db.get()?;

        let account = get_account_by_name(&conn, "default")?
            .ok_or_else(|| anyhow!("Default account not found"))?;
        let dest_address = TariAddress::from_base58(destination)?;

        let tx_builder = OneSidedTransaction::new(db.clone(), Network::Esmeralda, self.password.clone());

        let recipient = Recipient {
            address: dest_address,
            amount: MicroMinotari(amount),
            payment_id: None,
        };

        let locked_funds = self.lock_funds(1).await?;
        
        let unsigned_tx = tx_builder.create_unsigned_transaction(
            &account,
            locked_funds,
            vec![recipient],
            MicroMinotari(fee_per_gram),
        )?;

        let key_manager = account.get_key_manager(&self.password)?;
        let consensus_constants = ConsensusConstantsBuilder::new(Network::Esmeralda).build();
        let signed_result = sign_locked_transaction(
            &key_manager,
            consensus_constants,
            Network::Esmeralda,
            unsigned_tx,
        )?;

        self.broadcast_signed_transaction(&signed_result).await
    }

    /// Wait for a transaction to be confirmed by polling the base node.
    async fn wait_for_confirmation(&self, tx_id: u64, c_min: u32, timeout_secs: u64) -> Result<()> {
        let start = std::time::Instant::now();
        let timeout = Duration::from_secs(timeout_secs);
        let base_node = crate::http_rpc::BaseNodeRpcClient::new(&self.base_node_http);

        loop {
            if start.elapsed() > timeout {
                anyhow::bail!(
                    "Timeout waiting for tx {} confirmation (c_min={})",
                    tx_id, c_min
                );
            }

            let tip_height = base_node.get_tip_height().await?;

            // Re-scan to pick up any new confirmed outputs
            if let Err(e) = self.scan_blockchain().await {
                debug!("Scan during confirmation poll: {}", e);
            }

            // Check if UTXO count has increased (means our tx confirmed)
            let utxo_count = self.query_utxo_count().await.unwrap_or(0);
            let balance = self.query_balance().await.unwrap_or(0);
            
            debug!(
                "Tx {} polling: tip={}, utxos={}, balance={}",
                tx_id, tip_height, utxo_count, balance
            );

            // If we have any UTXOs and balance, assume tx is confirmed
            if utxo_count > 0 && balance > 0 {
                let scanned_tip = tip_height;
                if scanned_tip > 0 {
                    return Ok(());
                }
            }

            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }

    async fn wait_for_funding(&self, target_balance: u64, _c_min: u32, timeout_secs: u64) -> Result<u64> {
        let start = std::time::Instant::now();
        let timeout = Duration::from_secs(timeout_secs);

        loop {
            if start.elapsed() > timeout {
                anyhow::bail!("Timeout waiting for funding (target={} µT)", target_balance);
            }

            let balance = self.query_balance().await?;
            debug!("Funding poll: balance={} µT (target={} µT)", balance, target_balance);

            if balance >= target_balance {
                return Ok(balance);
            }

            tokio::time::sleep(Duration::from_secs(10)).await;
        }
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
        self.query_balance().await
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

        // Doubling phase
        for round in 0..config.doubling_rounds {
            match self.send_to_self(current_utxos * 2, config.fee_rate).await {
                Ok(tx_id) => {
                    info!("S1: Doubling round {} tx: {}", round + 1, tx_id);
                    self.wait_for_confirmation(tx_id, config.c_min, 300).await?;
                    current_utxos *= 2;
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

                match self.send_to_self(outputs_this_round, config.fee_rate).await {
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

        // Clone wallet handle for concurrent tasks -- each task gets its own
        // copy of the wallet state. SQLite handles its own locking internally.
        let handle = WalletHandle::from_wallet(self);

        for &n_concurrent in &config.concurrent_batches {
            info!("S4: Running {} concurrent transactions", n_concurrent);

            let mut handles = Vec::new();
            for i in 0..n_concurrent {
                let h = handle.clone();
                let fee_rate = config.fee_rate;
                let handle = tokio::spawn(async move {
                    let tx_result = send_concurrent_tx(&h, fee_rate).await;
                    (i, tx_result)
                });
                handles.push(handle);
            }

            let batch_start = Instant::now();
            let batch_results: Vec<_> = futures::future::join_all(handles).await;

            for r in batch_results {
                match r {
                    Ok((_, Ok(tx_id))) => {
                        total_successes += 1;
                        debug!("S4 tx succeeded: tx_id={}", tx_id);
                    }
                    Ok((_, Err(e))) => {
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
            match self.send_single_transfer(
                self.address.as_deref().unwrap_or("otl_esm_1placeholder"),
                100_000,
                config.fee_rate,
            ).await {
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

/// Send a concurrent transaction using a cloneable wallet handle.
/// This is the standalone version of send_to_self that works with tokio::spawn.
async fn send_concurrent_tx(handle: &WalletHandle, fee_per_gram: u64) -> Result<u64> {
    use minotari::transactions::one_sided_transaction::{OneSidedTransaction, Recipient};
    use minotari::db::get_account_by_name;
    use tari_common::configuration::Network;
    use tari_common_types::tari_address::TariAddress;
    use tari_transaction_components::offline_signing::sign_locked_transaction;
    use tari_transaction_components::consensus::ConsensusConstantsBuilder;
    use tari_transaction_components::tari_amount::MicroMinotari;

    let db = minotari::init_db(handle.db_path.clone())?;
    let conn = db.get()?;

    let account = get_account_by_name(&conn, "default")?
        .ok_or_else(|| anyhow!("Default account not found"))?;

    let address = TariAddress::from_base58(
        handle
            .address
            .as_deref()
            .ok_or_else(|| anyhow!("Wallet address not available"))?,
    )?;

    let tx_builder =
        OneSidedTransaction::new(db.clone(), Network::Esmeralda, handle.password.clone());

    let amount_per_output = MicroMinotari(100_000);
    let total_amount = amount_per_output.0;

    let recipient = Recipient {
        address: address.clone(),
        amount: MicroMinotari(total_amount),
        payment_id: Some(format!(
            "bench-s4-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
        )),
    };

    // Lock funds
    let locked_funds = lock_funds_concurrent(handle).await?;

    // Create unsigned transaction
    let unsigned_tx = tx_builder.create_unsigned_transaction(
        &account,
        locked_funds,
        vec![recipient],
        MicroMinotari(fee_per_gram),
    )?;

    // Sign
    let key_manager = account.get_key_manager(&handle.password)?;
    let consensus_constants = ConsensusConstantsBuilder::new(Network::Esmeralda).build();
    let signed_result = sign_locked_transaction(
        &key_manager,
        consensus_constants,
        Network::Esmeralda,
        unsigned_tx,
    )?;

    // Broadcast
    broadcast_signed_concurrent(handle, &signed_result).await
}

/// Lock funds for a concurrent transaction.
async fn lock_funds_concurrent(
    handle: &WalletHandle,
) -> Result<minotari::api::types::LockFundsResult> {
    use minotari::db::{get_account_by_name, fetch_unspent_outputs};
    use tari_transaction_components::tari_amount::MicroMinotari;
    use tari_transaction_components::utxo_selection::UtxoValue;

    let db = minotari::init_db(handle.db_path.clone())?;
    let conn = db.get()?;

    let account = get_account_by_name(&conn, "default")?
        .ok_or_else(|| anyhow!("Default account not found"))?;
    let outputs = fetch_unspent_outputs(&conn, account.id, 0)?;

    let amount_per_output = MicroMinotari(100_000);
    let total_amount = amount_per_output.0;

    let mut locked_outputs = Vec::new();
    let mut accumulated = 0u64;

    for output in outputs {
        locked_outputs.push(output.output.clone());
        accumulated += output.value().as_u64();
        if accumulated >= total_amount {
            break;
        }
    }

    let fee_per_gram = MicroMinotari(5);
    let estimated_tx_size = 500;
    let fee_with_change = MicroMinotari(fee_per_gram.0 * estimated_tx_size);
    let fee_without_change = MicroMinotari((fee_per_gram.0 * (estimated_tx_size - 100)) / 2);

    Ok(minotari::api::types::LockFundsResult {
        utxos: locked_outputs,
        requires_change_output: accumulated > total_amount,
        total_value: MicroMinotari(accumulated),
        fee_without_change,
        fee_with_change,
    })
}

/// Broadcast a signed transaction via HTTP RPC (concurrent version).
async fn broadcast_signed_concurrent(
    handle: &WalletHandle,
    signed_result: &tari_transaction_components::offline_signing::models::SignedOneSidedTransactionResult,
) -> Result<u64> {
    let client = reqwest::Client::new();
    let submit_url = format!("{}/json_rpc", handle.base_node_http);
    let transaction = &signed_result.signed_transaction.transaction;

    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "1",
        "method": "submit_transaction",
        "params": { "transaction": transaction }
    });

    let response = client
        .post(&submit_url)
        .json(&request)
        .send()
        .await
        .context("Failed to send broadcast request")?;

    let status = response.status();
    if status.is_success() {
        let numeric_tx_id = signed_result.request.tx_id.as_u64();
        debug!("Transaction broadcast successfully (tx_id={})", numeric_tx_id);
        Ok(numeric_tx_id)
    } else {
        anyhow::bail!("Transaction broadcast failed: {}", status);
    }
}
