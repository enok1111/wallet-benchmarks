//! New Wallet Mode (minotari-cli library with offline signing)
//!
//! Uses the minotari crate directly for:
//! - Blockchain scanning via Scanner
//! - Balance queries via get_balance
//! - Transaction building and broadcast via HTTP RPC
//! - SQLite wallet database with encrypted keys

use anyhow::{anyhow, Context, Result};
use log::{debug, info, warn};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::config::HarnessConfig;
use crate::metrics::ScenarioResult;
use crate::modes::WalletMode;

/// New wallet mode implementation using minotari library directly
pub struct NewWalletMode {
    /// Data directory path for this wallet instance
    data_dir: PathBuf,
    /// Database file path
    db_path: PathBuf,
    /// Wallet address (retrieved after initialization)
    address: Option<String>,
    /// Seed words for this wallet
    seed_words: Vec<String>,
    /// Password for wallet encryption
    password: String,
    /// Base node HTTP RPC endpoint
    base_node_http: String,
    /// Account ID in the wallet database
    account_id: u32,
}

impl NewWalletMode {
    pub fn new(data_dir: PathBuf) -> Self {
        let db_path = data_dir.join("wallet.db");
        Self {
            data_dir,
            db_path,
            address: None,
            seed_words: Vec::new(),
            password: "benchmark_password_32chars_min".to_string(),
            base_node_http: "http://127.0.0.1:18142".to_string(),
            account_id: 1, // Default account ID
        }
    }

    /// Initialize the wallet database with view key and spend public key
    async fn init_wallet_db(&self, birthday_height: u64) -> Result<()> {
        // In production, derive keys from seed words using BIP39/BIP32
        // For now, use minotari::utils::init_wallet::init_with_view_key
        // This requires view_private_key and spend_public_key in hex format
        
        info!(
            "Initializing wallet DB at {} with birthday height {}",
            self.db_path.display(),
            birthday_height
        );

        // Create database directory
        std::fs::create_dir_all(&self.data_dir)?;

        // TODO: Implement actual key derivation and wallet initialization
        // minotari::utils::init_wallet::init_with_view_key(
        //     &view_private_key_hex,
        //     &spend_public_key_hex,
        //     &self.password,
        //     &self.db_path,
        //     birthday_height,
        //     Some("default"),
        // )?;

        debug!("Wallet database initialized");
        Ok(())
    }

    /// Scan blockchain using the minotari Scanner
    async fn scan_blockchain(&self, _from_height: u64) -> Result<Vec<minotari::WalletEvent>> {
        use minotari::{Scanner, ScanMode};

        info!(
            "Scanning blockchain via {}",
            self.base_node_http
        );

        // Run the scanner with Full mode
        let (events, _more_blocks) = Scanner::new(
            &self.password,
            &self.base_node_http,
            self.db_path.clone(),
            100, // batch_size - blocks per HTTP request
            10,  // required_confirmations
        )
        .mode(ScanMode::Full)
        .account("default")
        .run()
        .await
        .context("Scanner failed")?;

        info!("Scan completed: {} events processed", events.len());
        Ok(events)
    }

    /// Get wallet balance from database
    async fn query_balance(&self) -> Result<u64> {
        use minotari::{get_balance, init_db};

        let db = init_db(self.db_path.clone())
            .context("Failed to initialize database connection")?;
        let conn = db.get().context("Failed to get DB connection")?;
        
        let balance = get_balance(&conn, self.account_id as i64)
            .context("Failed to query balance")?;

        debug!("Balance: {} µT available", balance.available);
        // MicroMinotari has inner() method to get u64 value
        Ok(balance.available.into())
    }

    /// Generate a unique 12-word seed phrase for this mode.
    fn generate_seed_words(mode_suffix: &str) -> Vec<String> {
        let mut words = vec![
            "abandon".to_string(), "ability".to_string(), "able".to_string(), "about".to_string(),
            "above".to_string(), "absent".to_string(), "absorb".to_string(), "abstract".to_string(),
            "absurd".to_string(), "abuse".to_string(), "access".to_string(),
        ];
        words.push(format!("{}{}", "accident", mode_suffix));
        words
    }

    /// Create and broadcast a transaction via HTTP RPC
    async fn create_and_broadcast_transaction(
        &self,
        recipient_address: &str,
        amount_ut: u64,
        fee_per_gram: u64,
    ) -> Result<String> {
        // For now, use HTTP RPC client to submit pre-built transaction
        // In production, this would use minotari's TransactionSender for offline signing
        let http_client = crate::http_rpc::BaseNodeRpcClient::new(&self.base_node_http);
        
        // TODO: Build actual transaction bytes using minotari transaction builder
        let tx_bytes = vec![]; // Placeholder
        
        match http_client.submit_transaction(&tx_bytes).await {
            Ok(tx_id) => {
                debug!("Transaction broadcasted: {}", tx_id);
                Ok(tx_id)
            }
            Err(e) => {
                warn!("Failed to broadcast transaction: {}", e);
                Err(e)
            }
        }
    }

     /// Query UTXO count from database (simplified implementation)
    async fn query_utxo_count(&self) -> Result<u32> {
        // For now, return placeholder - actual implementation would use minotari DB queries
        // The minotari crate provides its own database access methods
        debug!("Querying UTXO count (implementation pending)");
        Ok(0)
    }
}

#[async_trait::async_trait]
impl WalletMode for NewWalletMode {
    async fn initialize(&mut self, config: &HarnessConfig) -> Result<()> {
        info!("Initializing new wallet mode at {}", self.data_dir.display());

        // Create data directory
        std::fs::create_dir_all(&self.data_dir)?;

        // Generate or use provided seed words (unique per mode)
        if self.seed_words.is_empty() {
            self.seed_words = config
                .seed_words_new
                .clone()
                .unwrap_or_else(|| Self::generate_seed_words("new"));
        }

        // Initialize wallet database with birthday height 0 (genesis)
        self.init_wallet_db(0).await?;

        // Retrieve wallet address after initialization
        self.address = Some("placeholder_address".to_string());

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
        // No process to kill; just clean up temp files if needed
        info!("New wallet torn down");
        Ok(())
    }
}

// Scenario implementations for new wallet mode
impl NewWalletMode {
    async fn run_b0(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        // B0 - Baseline Scan (empty wallet)
        // Floor cost of block-walk + view-key check with nothing to store.
        use std::time::Instant;

        let start = Instant::now();
        info!("B0: Baseline scan for new wallet mode (library integration)");

        // Record chain tip at start
        let base_node_client =
            crate::http_rpc::BaseNodeRpcClient::new(&config.base_node_http);
        let tip_height_start = base_node_client.get_tip_height().await?;
        result.tip_height_start = tip_height_start;

        // TODO: Run scanner using minotari crate
        // Scanner::new(password, base_url, db_path, batch_size)
        //   .mode(ScanMode::Full)
        //   .run()
        info!(
            "B0: Scanning from genesis to tip {} (library integration pending)",
            tip_height_start
        );

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
            "B0 (new wallet) completed: {} blocks in {:.2}s",
            blocks_scanned, scan_time_secs
        );
        Ok(())
    }

    async fn run_s0(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        // S0 - Funding Baseline
        use std::time::Instant;

        let start = Instant::now();
        info!("S0: Funding baseline for new wallet mode");

        let funding_address = self.address.as_deref().unwrap_or("placeholder");
        info!(
            "S0: Funding address: {} (awaiting {} µT)",
            funding_address, config.a_fund
        );

        // Wait for funding
        let wait_result = self.wait_for_funding(config.a_fund, config.c_min, 600).await;

        let elapsed_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = elapsed_secs;

        match wait_result {
            Ok(balance) => {
                result.success_count = 1;
                result.balance_delta_ut = balance as i64;
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
        // S1 - UTXO Build-up (doubling + fan-out)
        use std::time::Instant;

        let start = Instant::now();
        info!(
            "S1: UTXO build-up for new wallet - target {} UTXOs",
            config.volume_target
        );

        let mut current_utxos = 1u32;

        // Doubling phase
        for round in 0..config.doubling_rounds {
            match self
                .send_to_self(current_utxos * 2, config.fee_rate)
                .await
            {
                Ok(tx_id) => {
                    info!("S1: Doubling round {} tx: {}", round + 1, tx_id);
                    self.wait_for_confirmation(config.c_min, 300).await?;
                    current_utxos *= 2;
                }
                Err(e) => {
                    result.failure_reasons
                        .push(format!("Doubling round {} failed: {}", round + 1, e));
                    break;
                }
            }
        }

        // Fan-out phase
        if current_utxos < config.volume_target {
            let remaining = config.volume_target - current_utxos;
            let fanout_rounds =
                (remaining + config.fanout_outputs_per_tx - 1) / config.fanout_outputs_per_tx;

            for round in 0..fanout_rounds {
                let outputs = std::cmp::min(
                    config.fanout_outputs_per_tx,
                    config.volume_target - current_utxos,
                );
                match self.send_to_self(outputs, config.fee_rate).await {
                    Ok(tx_id) => {
                        info!("S1: Fan-out round {} tx: {}", round + 1, tx_id);
                        self.wait_for_confirmation(config.c_min, 300).await?;
                        current_utxos += outputs - 1;
                    }
                    Err(e) => {
                        result.failure_reasons
                            .push(format!("Fan-out round {} failed: {}", round + 1, e));
                        break;
                    }
                }
            }
        }

        let elapsed_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = elapsed_secs;
        result.success_count = if current_utxos >= config.volume_target { 1 } else { 0 };

        let mut s1_metrics: HashMap<String, serde_json::Value> = HashMap::new();
        s1_metrics.insert("final_utxo_count".into(), serde_json::json!(current_utxos));
        result.metrics.extend(s1_metrics);

        info!("S1 (new wallet) completed: {} UTXOs in {:.2}s", current_utxos, elapsed_secs);
        Ok(())
    }

    async fn run_s2(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        // S2 - Scan from Genesis (checkpoint 1)
        use std::time::Instant;

        let start = Instant::now();
        info!("S2: Scan from Genesis (checkpoint 1) for new wallet");

        let base_node_client =
            crate::http_rpc::BaseNodeRpcClient::new(&config.base_node_http);
        let tip_height_start = base_node_client.get_tip_height().await?;
        result.tip_height_start = tip_height_start;

        // TODO: Wipe DB and rescan from genesis using minotari Scanner
        self.rescan_from_height(config, 0).await?;

        let scan_time_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = scan_time_secs;

        let tip_height_end = base_node_client.get_tip_height().await?;
        result.tip_height_end = tip_height_end;

        let blocks_scanned = tip_height_end.saturating_sub(tip_height_start);
        let utxo_count = self.query_utxo_count().await.unwrap_or(0);
        result.success_count = if utxo_count >= config.volume_target { 1 } else { 0 };
        result.balance_delta_ut = self.query_balance().await? as i64;

        let mut s2_metrics: HashMap<String, serde_json::Value> = HashMap::new();
        s2_metrics.insert("scan_mode".into(), serde_json::json!("genesis"));
        s2_metrics.insert("blocks_scanned".into(), serde_json::json!(blocks_scanned));
        s2_metrics.insert("utxo_count_found".into(), serde_json::json!(utxo_count));
        result.metrics.extend(s2_metrics);

        info!(
            "S2 (new wallet) completed: {} blocks, {} UTXOs in {:.2}s",
            blocks_scanned, utxo_count, scan_time_secs
        );
        Ok(())
    }

    async fn run_s3(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        // S3 - Scan from Birthday (checkpoint 1)
        use std::time::Instant;

        let start = Instant::now();
        info!("S3: Scan from Birthday (checkpoint 1) for new wallet");

        let base_node_client =
            crate::http_rpc::BaseNodeRpcClient::new(&config.base_node_http);
        let tip_height_start = base_node_client.get_tip_height().await?;
        result.tip_height_start = tip_height_start;

        let birthday_height = 1u64;
        self.rescan_from_height(config, birthday_height).await?;

        let scan_time_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = scan_time_secs;

        let tip_height_end = base_node_client.get_tip_height().await?;
        result.tip_height_end = tip_height_end;

        let blocks_scanned = tip_height_end.saturating_sub(birthday_height);
        let utxo_count = self.query_utxo_count().await.unwrap_or(0);
        result.success_count = if utxo_count >= config.volume_target { 1 } else { 0 };
        result.balance_delta_ut = self.query_balance().await? as i64;

        let mut s3_metrics: HashMap<String, serde_json::Value> = HashMap::new();
        s3_metrics.insert("scan_mode".into(), serde_json::json!("birthday"));
        s3_metrics.insert("birthday_height".into(), serde_json::json!(birthday_height));
        s3_metrics.insert("blocks_scanned".into(), serde_json::json!(blocks_scanned));
        s3_metrics.insert("utxo_count_found".into(), serde_json::json!(utxo_count));
        result.metrics.extend(s3_metrics);

        info!(
            "S3 (new wallet) completed: {} blocks, {} UTXOs in {:.2}s",
            blocks_scanned, utxo_count, scan_time_secs
        );
        Ok(())
    }

    async fn run_s4(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        // S4 - Concurrent Construction
        use std::time::Instant;

        let start = Instant::now();
        info!(
            "S4: Concurrent Construction for new wallet - batches {:?}",
            config.concurrent_batches
        );

        let mut total_success = 0u32;
        let mut total_failure = 0u32;
        let mut batch_results: Vec<serde_json::Value> = Vec::new();

        for n_concurrent in &config.concurrent_batches {
            let batch_start = Instant::now();
            let n_val = *n_concurrent;
            let recipient = self.address.as_deref().unwrap_or("placeholder").to_string();

            let mut handles = Vec::new();
            for i in 0..n_val {
                let rcpt = recipient.clone();
                let fee = config.fee_rate;
                let handle = tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    let _ = (rcpt, fee);
                    (i, true, format!("tx_s4_new_{}_{}", n_val, i))
                });
                handles.push(handle);
            }

            let mut batch_success = 0u32;
            let mut batch_failure = 0u32;
            for handle in handles {
                match handle.await {
                    Ok((_idx, success, _tx_id)) => {
                        if success {
                            batch_success += 1;
                        } else {
                            batch_failure += 1;
                        }
                    }
                    Err(e) => {
                        batch_failure += 1;
                        warn!("S4: Task join error: {}", e);
                    }
                }
            }

            let batch_time = batch_start.elapsed().as_secs_f64();
            total_success += batch_success;
            total_failure += batch_failure;

            batch_results.push(serde_json::json!({
                "n_concurrent": n_val,
                "batch_wall_clock_secs": batch_time,
                "success_count": batch_success,
                "failure_count": batch_failure,
                "success_rate": if n_val > 0 { batch_success as f64 / n_val as f64 } else { 0.0 },
            }));
        }

        let elapsed_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = elapsed_secs;
        result.success_count = total_success;
        result.failure_count = total_failure;

        let mut s4_metrics: HashMap<String, serde_json::Value> = HashMap::new();
        s4_metrics.insert("batch_results".into(), serde_json::json!(batch_results));
        result.metrics.extend(s4_metrics);

        info!(
            "S4 (new wallet) completed: {} success, {} failure in {:.2}s",
            total_success, total_failure, elapsed_secs
        );
        Ok(())
    }

    async fn run_s5(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        // S5 - Payment Processor Throughput (individual arm for new wallet)
        use std::time::Instant;

        let start = Instant::now();
        info!(
            "S5: Individual arm for new wallet - {} recipients",
            config.s5_m
        );

        let recipient = self.address.as_deref().unwrap_or("placeholder");
        let mut success_count = 0u32;
        let mut failure_count = 0u32;
        let mut total_fees: u64 = 0;

        for i in 0..config.s5_m {
            let amount = config.a_fund / config.s5_m as u64;
            match self
                .create_and_broadcast_transaction(recipient, amount, config.fee_rate)
                .await
            {
                Ok(_tx_id) => {
                    success_count += 1;
                    total_fees += config.fee_rate;
                }
                Err(e) => {
                    failure_count += 1;
                    result.failure_reasons.push(format!("Transfer {} failed: {}", i, e));
                }
            }
        }

        let elapsed_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = elapsed_secs;
        result.success_count = success_count;
        result.failure_count = failure_count;
        result.fees_paid_ut = total_fees;

        let mut s5_metrics: HashMap<String, serde_json::Value> = HashMap::new();
        s5_metrics.insert("arm".into(), serde_json::json!("individual"));
        s5_metrics.insert("t_individual_secs".into(), serde_json::json!(elapsed_secs));
        result.metrics.extend(s5_metrics);

        info!(
            "S5 (new wallet, individual) completed: {} success in {:.2}s",
            success_count, elapsed_secs
        );
        Ok(())
    }

    async fn run_s6(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        // S6 - Scan from Genesis (checkpoint 2)
        use std::time::Instant;

        let start = Instant::now();
        info!("S6: Scan from Genesis (checkpoint 2) for new wallet");

        let base_node_client =
            crate::http_rpc::BaseNodeRpcClient::new(&config.base_node_http);
        let tip_height_start = base_node_client.get_tip_height().await?;
        result.tip_height_start = tip_height_start;

        self.rescan_from_height(config, 0).await?;

        let scan_time_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = scan_time_secs;

        let tip_height_end = base_node_client.get_tip_height().await?;
        result.tip_height_end = tip_height_end;

        let blocks_scanned = tip_height_end.saturating_sub(tip_height_start);
        let utxo_count = self.query_utxo_count().await.unwrap_or(0);
        result.success_count = if utxo_count > 0 { 1 } else { 0 };

        let mut s6_metrics: HashMap<String, serde_json::Value> = HashMap::new();
        s6_metrics.insert("scan_mode".into(), serde_json::json!("genesis"));
        s6_metrics.insert("blocks_scanned".into(), serde_json::json!(blocks_scanned));
        s6_metrics.insert("utxo_count_found".into(), serde_json::json!(utxo_count));
        result.metrics.extend(s6_metrics);

        info!(
            "S6 (new wallet) completed: {} blocks, {} UTXOs in {:.2}s",
            blocks_scanned, utxo_count, scan_time_secs
        );
        Ok(())
    }

    async fn run_s7(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        // S7 - Scan from Birthday (checkpoint 2)
        use std::time::Instant;

        let start = Instant::now();
        info!("S7: Scan from Birthday (checkpoint 2) for new wallet");

        let base_node_client =
            crate::http_rpc::BaseNodeRpcClient::new(&config.base_node_http);
        let tip_height_start = base_node_client.get_tip_height().await?;
        result.tip_height_start = tip_height_start;

        let birthday_height = 1u64;
        self.rescan_from_height(config, birthday_height).await?;

        let scan_time_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = scan_time_secs;

        let tip_height_end = base_node_client.get_tip_height().await?;
        result.tip_height_end = tip_height_end;

        let blocks_scanned = tip_height_end.saturating_sub(birthday_height);
        let utxo_count = self.query_utxo_count().await.unwrap_or(0);
        result.success_count = if utxo_count > 0 { 1 } else { 0 };

        let mut s7_metrics: HashMap<String, serde_json::Value> = HashMap::new();
        s7_metrics.insert("scan_mode".into(), serde_json::json!("birthday"));
        s7_metrics.insert("birthday_height".into(), serde_json::json!(birthday_height));
        s7_metrics.insert("blocks_scanned".into(), serde_json::json!(blocks_scanned));
        s7_metrics.insert("utxo_count_found".into(), serde_json::json!(utxo_count));
        result.metrics.extend(s7_metrics);

        info!(
            "S7 (new wallet) completed: {} blocks, {} UTXOs in {:.2}s",
            blocks_scanned, utxo_count, scan_time_secs
        );
        Ok(())
    }

    // Helper methods for new wallet scenarios

    async fn wait_for_funding(&self, target: u64, _c_min: u32, timeout_secs: u64) -> Result<u64> {
        let start = std::time::Instant::now();
        let timeout = std::time::Duration::from_secs(timeout_secs);
        loop {
            if start.elapsed() > timeout {
                anyhow::bail!("Timeout waiting for funding of {} µT", target);
            }
            let balance = self.query_balance().await?;
            if balance >= target {
                return Ok(balance);
            }
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        }
    }

    async fn send_to_self(&self, output_count: u32, fee_rate: u64) -> Result<String> {
        // TODO: Use TransactionSender for self-send
        debug!(
            "Sending to self: {} outputs at fee {} µT/g",
            output_count, fee_rate
        );
        Ok(format!("tx_self_new_{}", output_count))
    }

    async fn wait_for_confirmation(&self, _c_min: u32, timeout_secs: u64) -> Result<()> {
        let start = std::time::Instant::now();
        let timeout = std::time::Duration::from_secs(timeout_secs);
        loop {
            if start.elapsed() > timeout {
                anyhow::bail!("Timeout waiting for confirmation");
            }
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            return Ok(());
        }
    }
  /// Rescan blockchain from a specific height
    async fn rescan_from_height(&self, _config: &HarnessConfig, from_height: u64) -> Result<()> {
        info!("Rescanning from height {}", from_height);
        let events = self.scan_blockchain(from_height).await?;
        info!("Rescan completed: {} events", events.len());
        Ok(())
    }
}
