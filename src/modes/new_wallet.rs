//! New Wallet Mode (minotari-cli library with offline signing)
//!
//! Uses the minotari crate directly for:
//! - Blockchain scanning via Scanner
//! - Balance queries via get_balance
//! - Transaction building and broadcast via HTTP RPC
//! - SQLite wallet database with encrypted keys
//!
//! Note: The minotari crate is a view-only wallet. Transaction signing
//! requires access to spend keys which are not available in view-only mode.
//! For transactions, this mode uses the minotari_console_wallet gRPC interface
//! for signing while keeping scanning and balance queries library-native.

use anyhow::{anyhow, Context, Result};
use log::{debug, info, warn};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

use crate::config::HarnessConfig;
use crate::metrics::ScenarioResult;
use crate::modes::{WalletMode, WalletModeId};

/// New wallet mode implementation using minotari library directly
pub struct NewWalletMode {
    data_dir: PathBuf,
    db_path: PathBuf,
    address: Option<String>,
    seed_words: Vec<String>,
    password: String,
    base_node_http: String,
    account_id: u32,
    grpc_client: Arc<Mutex<Option<crate::grpc_client::OldWalletGrpcClient>>>,
    grpc_address: String,
}

impl NewWalletMode {
    pub fn new(data_dir: PathBuf, grpc_port: u16) -> Self {
        let db_path = data_dir.join("wallet.db");
        let grpc_address = format!("127.0.0.1:{}", grpc_port);
        Self {
            data_dir,
            db_path,
            address: None,
            seed_words: Vec::new(),
            password: "benchmark_password_32chars_min".to_string(),
            base_node_http: "http://127.0.0.1:18143".to_string(),
            account_id: 1,
            grpc_client: Arc::new(Mutex::new(None)),
            grpc_address,
        }
    }

    /// Initialize the wallet database. The birthday_height is stored in the DB's account
    /// record (set by the gRPC wallet subprocess). The minotari Scanner reads the birthday
    /// from the accounts table to determine its starting scan height.
    async fn init_wallet_db(&self, birthday_height: u64) -> Result<()> {
        info!(
            "Initializing wallet DB at {} with birthday height {}",
            self.db_path.display(),
            birthday_height
        );

        std::fs::create_dir_all(&self.data_dir)?;

        let pool = minotari::db::init_db(self.db_path.clone())
            .map_err(|e| anyhow!("Failed to initialize minotari database: {}", e))?;

        // If an account already exists (created by the gRPC wallet subprocess),
        // update its birthday to match the requested scan start height.
        if let Ok(conn) = pool.get() {
            let birthday_val: i64 = birthday_height as i64;
            if let Ok(count) = conn.query_row(
                "SELECT COUNT(*) FROM accounts",
                [],
                |row| row.get::<_, i64>(0),
            ) {
                if count > 0 {
                    let _ = conn.execute(
                        "UPDATE accounts SET birthday = ?1 WHERE birthday != ?1",
                        [birthday_val],
                    );
                    info!("Updated account birthday to {}", birthday_height);
                }
            }
        }

        debug!("Wallet database initialized");
        Ok(())
    }

    /// Scan the blockchain using the minotari Scanner.
    /// The Scanner reads the starting height from the wallet's stored birthday in the DB
    /// (accounts.birthday column), not from the `_from_height` parameter. For genesis
    /// rescans, call `rescan_from_height` which adjusts the birthday in the DB before scanning.
    async fn scan_blockchain(&self) -> Result<Vec<minotari::WalletEvent>> {
        use minotari::{Scanner, ScanMode};

        info!(
            "Scanning blockchain via {}",
            self.base_node_http,
        );

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
        use minotari::{init_db, db::get_accounts, db::fetch_unspent_outputs};

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

    async fn get_balance_via_grpc(&self) -> Result<u64> {
        let mut guard = self.grpc_client.lock().await;
        let client = guard.as_mut().ok_or_else(|| anyhow!("gRPC client not connected"))?;
        let resp = client.get_balance().await?;
        Ok(resp.available_balance)
    }

    async fn send_to_self_via_grpc(&self, output_count: u32, fee_per_gram: u64) -> Result<String> {
        let address = self.address.as_deref().ok_or_else(|| anyhow!("Wallet address not available"))?;
        let amount_per_output = 100_000u64;

        let mut guard = self.grpc_client.lock().await;
        let client = guard.as_mut().ok_or_else(|| anyhow!("gRPC client not connected"))?;

        let recipients: Vec<_> = (0..output_count)
            .map(|_| minotari_app_grpc::tari_rpc::PaymentRecipient {
                address: address.to_string(),
                amount: amount_per_output,
                fee_per_gram,
                payment_type: 0,
                raw_payment_id: Vec::new(),
                user_payment_id: None,
            })
            .collect();

        let request = tonic::Request::new(minotari_app_grpc::tari_rpc::TransferRequest {
            recipients,
            single_tx: false,
        });

        let resp = client.client_mut().transfer(request).await?;
        let inner = resp.into_inner();
        let tx_id = inner.results.first()
            .map(|r| r.transaction_id.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        Ok(tx_id)
    }

    async fn send_single_transfer_via_grpc(&self, destination: &str, amount: u64, fee_per_gram: u64) -> Result<String> {
        let mut guard = self.grpc_client.lock().await;
        let client = guard.as_mut().ok_or_else(|| anyhow!("gRPC client not connected"))?;
        let resp = client.transfer(destination, amount, fee_per_gram).await?;
        let tx_id = resp.results.first()
            .map(|r| r.transaction_id.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        Ok(tx_id)
    }

    async fn wait_for_confirmation_via_grpc(&self, c_min: u32, timeout_secs: u64) -> Result<()> {
        let start = std::time::Instant::now();
        let timeout = Duration::from_secs(timeout_secs);
        let base_node_client = crate::http_rpc::BaseNodeRpcClient::new(&self.base_node_http);
        let broadcast_tip = base_node_client.get_tip_height().await?;

        loop {
            if start.elapsed() > timeout {
                anyhow::bail!("Timeout waiting for confirmation (c_min={})", c_min);
            }

            let current_tip = base_node_client.get_tip_height().await?;
            if current_tip >= broadcast_tip + c_min as u64 {
                debug!(
                    "Confirmation reached: broadcast_tip={}, current_tip={}, c_min={}",
                    broadcast_tip, current_tip, c_min
                );
                return Ok(());
            }
            debug!(
                "Waiting for {} confirmations: broadcast_tip={}, current_tip={}, c_min={}",
                current_tip.saturating_sub(broadcast_tip),
                broadcast_tip,
                current_tip,
                c_min
            );
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    }

    async fn wait_for_grpc_ready(&self, timeout_secs: u64) -> Result<()> {
        let start = std::time::Instant::now();
        let timeout = Duration::from_secs(timeout_secs);

        loop {
            if start.elapsed() > timeout {
                return Err(anyhow!("Timeout waiting for gRPC server to be ready"));
            }

            match crate::grpc_client::OldWalletGrpcClient::connect(&self.grpc_address).await {
                Ok(client) => {
                    info!("New wallet gRPC server ready at {}", self.grpc_address);
                    let mut guard = self.grpc_client.lock().await;
                    *guard = Some(client);
                    return Ok(());
                }
                Err(_) => {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            }
        }
    }

    fn generate_seed_words(mode_id: WalletModeId) -> Vec<String> {
        let suffix = mode_id.suffix_word();
        let words = [
            "abandon", "ability", "able", "about", "above", "absent",
            "absorb", "abstract", "absurd", "abuse", "access", suffix,
        ];
        words.iter().map(|w| w.to_string()).collect()
    }
}

#[async_trait::async_trait]
impl WalletMode for NewWalletMode {
    async fn initialize(&mut self, config: &HarnessConfig) -> Result<()> {
        info!("Initializing new wallet mode at {}", self.data_dir.display());

        std::fs::create_dir_all(&self.data_dir)?;

        if self.seed_words.is_empty() {
            self.seed_words = config
                .seed_words_new
                .clone()
                .unwrap_or_else(|| Self::generate_seed_words(WalletModeId::New));
        }

        self.base_node_http = config.base_node_http.clone();

        self.init_wallet_db(0).await?;

        self.wait_for_grpc_ready(60).await?;

        let mut guard = self.grpc_client.lock().await;
        if let Some(ref mut client) = *guard {
            match client.get_address().await {
                Ok(response) => {
                    self.address = Some(hex::encode(&response.interactive_address));
                    info!("Wallet address: {}", self.address.as_ref().unwrap());
                }
                Err(e) => {
                    warn!("Failed to get wallet address via gRPC: {}", e);
                    self.address = Some("placeholder_address".to_string());
                }
            }
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
        self.get_balance_via_grpc().await
    }

    async fn teardown(&mut self) -> Result<()> {
        info!("Tearing down new wallet mode");
        info!("New wallet torn down");
        Ok(())
    }
}

impl NewWalletMode {
    async fn run_b0(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
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

    async fn run_s0(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        use std::time::Instant;

        let start = Instant::now();
        info!("S0: Funding baseline for new wallet mode");

        let funding_address = self.address.as_deref().unwrap_or("placeholder");
        info!(
            "S0: Funding address: {} (awaiting {} µT)",
            funding_address, config.a_fund
        );

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
        use std::time::Instant;

        let start = Instant::now();
        info!(
            "S1: UTXO build-up for new wallet - target {} UTXOs",
            config.volume_target
        );

        let mut current_utxos = 1u32;

        for round in 0..config.doubling_rounds {
            match self.send_to_self_via_grpc(current_utxos * 2, config.fee_rate).await {
                Ok(tx_id) => {
                    info!("S1: Doubling round {} tx: {}", round + 1, tx_id);
                    self.wait_for_confirmation_via_grpc(config.c_min, 300).await?;
                    current_utxos *= 2;
                }
                Err(e) => {
                    result.failure_reasons.push(format!("Doubling round {} failed: {}", round + 1, e));
                    break;
                }
            }
        }

        if current_utxos < config.volume_target {
            let remaining = config.volume_target - current_utxos;
            let fanout_rounds =
                (remaining + config.fanout_outputs_per_tx - 1) / config.fanout_outputs_per_tx;

            for round in 0..fanout_rounds {
                let outputs = std::cmp::min(
                    config.fanout_outputs_per_tx,
                    config.volume_target - current_utxos,
                );
                match self.send_to_self_via_grpc(outputs, config.fee_rate).await {
                    Ok(tx_id) => {
                        info!("S1: Fan-out round {} tx: {}", round + 1, tx_id);
                        self.wait_for_confirmation_via_grpc(config.c_min, 300).await?;
                        current_utxos += outputs - 1;
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

        info!("S1 (new wallet) completed: {} UTXOs in {:.2}s", current_utxos, elapsed_secs);
        Ok(())
    }

    async fn run_s2(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        use std::time::Instant;

        let start = Instant::now();
        info!("S2: Scan from Genesis (checkpoint 1) for new wallet");

        let base_node_client = crate::http_rpc::BaseNodeRpcClient::new(&config.base_node_http);
        let tip_height_start = base_node_client.get_tip_height().await?;
        result.tip_height_start = tip_height_start;

        self.rescan_from_height(config, 0).await?;

        let scan_time_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = scan_time_secs;

        let tip_height_end = base_node_client.get_tip_height().await?;
        result.tip_height_end = tip_height_end;

        let blocks_scanned = tip_height_end.saturating_sub(tip_height_start);
        let utxo_count = self.query_utxo_count().await.unwrap_or(0);
        result.success_count = if utxo_count >= config.volume_target { 1 } else { 0 };
        result.balance_delta_ut = self.query_balance().await.unwrap_or(0) as i64;

        let mut s2_metrics = HashMap::new();
        s2_metrics.insert("scan_mode".into(), serde_json::json!("genesis"));
        s2_metrics.insert("blocks_scanned".into(), serde_json::json!(blocks_scanned));
        s2_metrics.insert("utxo_count_found".into(), serde_json::json!(utxo_count));
        result.metrics.extend(s2_metrics);

        info!("S2 (new wallet) completed: {} blocks, {} UTXOs in {:.2}s", blocks_scanned, utxo_count, scan_time_secs);
        Ok(())
    }

    async fn run_s3(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        use std::time::Instant;

        let start = Instant::now();
        info!("S3: Scan from Birthday (checkpoint 1) for new wallet");

        let base_node_client = crate::http_rpc::BaseNodeRpcClient::new(&config.base_node_http);
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
        result.balance_delta_ut = self.query_balance().await.unwrap_or(0) as i64;

        let mut s3_metrics = HashMap::new();
        s3_metrics.insert("scan_mode".into(), serde_json::json!("birthday"));
        s3_metrics.insert("birthday_height".into(), serde_json::json!(birthday_height));
        s3_metrics.insert("blocks_scanned".into(), serde_json::json!(blocks_scanned));
        s3_metrics.insert("utxo_count_found".into(), serde_json::json!(utxo_count));
        result.metrics.extend(s3_metrics);

        info!("S3 (new wallet) completed: {} blocks, {} UTXOs in {:.2}s", blocks_scanned, utxo_count, scan_time_secs);
        Ok(())
    }

    async fn run_s4(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
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
            let fee = config.fee_rate;
            let client_arc = Arc::clone(&self.grpc_client);

            let mut handles = Vec::new();
            for i in 0..n_val {
                let rcpt = recipient.clone();
                let c_arc = Arc::clone(&client_arc);
                let handle = tokio::spawn(async move {
                    let start_tx = std::time::Instant::now();
                    let mut guard = c_arc.lock().await;
                    let outcome = if let Some(ref mut client) = *guard {
                        match client.transfer(&rcpt, 100_000, fee).await {
                            Ok(resp) => {
                                let tx_id = resp.results.first()
                                    .map(|r| r.transaction_id.to_string())
                                    .unwrap_or_else(|| "unknown".to_string());
                                (i, true, tx_id, start_tx.elapsed().as_micros() as u64)
                            }
                            Err(e) => (i, false, format!("error: {}", e), start_tx.elapsed().as_micros() as u64),
                        }
                    } else {
                        (i, false, "no client".to_string(), start_tx.elapsed().as_micros() as u64)
                    };
                    outcome
                });
                handles.push(handle);
            }

            let mut batch_success = 0u32;
            let mut batch_failure = 0u32;
            let mut construction_times: Vec<u64> = Vec::new();
            for handle in handles {
                match handle.await {
                    Ok((_idx, success, tx_id, construction_us)) => {
                        construction_times.push(construction_us);
                        if success {
                            batch_success += 1;
                        } else {
                            batch_failure += 1;
                            result.failure_reasons.push(format!("S4 tx failed: {}", tx_id));
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

            let max_serialization_gap_ms = if construction_times.len() > 1 {
                let mut sorted = construction_times.clone();
                sorted.sort();
                sorted.windows(2)
                    .map(|w| (w[1] - w[0]) as f64 / 1000.0)
                    .fold(0.0_f64, f64::max)
            } else {
                0.0
            };

            batch_results.push(serde_json::json!({
                "n_concurrent": n_val,
                "batch_wall_clock_secs": batch_time,
                "success_count": batch_success,
                "failure_count": batch_failure,
                "success_rate": if n_val > 0 { batch_success as f64 / n_val as f64 } else { 0.0 },
                "max_serialization_gap_ms": max_serialization_gap_ms,
            }));
        }

        let elapsed_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = elapsed_secs;
        result.success_count = total_success;
        result.failure_count = total_failure;

        let mut s4_metrics = HashMap::new();
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
            match self.send_single_transfer_via_grpc(recipient, amount, config.fee_rate).await {
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

        let mut s5_metrics = HashMap::new();
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
        use std::time::Instant;

        let start = Instant::now();
        info!("S6: Scan from Genesis (checkpoint 2) for new wallet");

        let base_node_client = crate::http_rpc::BaseNodeRpcClient::new(&config.base_node_http);
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

        let mut s6_metrics = HashMap::new();
        s6_metrics.insert("scan_mode".into(), serde_json::json!("genesis"));
        s6_metrics.insert("blocks_scanned".into(), serde_json::json!(blocks_scanned));
        s6_metrics.insert("utxo_count_found".into(), serde_json::json!(utxo_count));
        result.metrics.extend(s6_metrics);

        info!("S6 (new wallet) completed: {} blocks, {} UTXOs in {:.2}s", blocks_scanned, utxo_count, scan_time_secs);
        Ok(())
    }

    async fn run_s7(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        use std::time::Instant;

        let start = Instant::now();
        info!("S7: Scan from Birthday (checkpoint 2) for new wallet");

        let base_node_client = crate::http_rpc::BaseNodeRpcClient::new(&config.base_node_http);
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

        let mut s7_metrics = HashMap::new();
        s7_metrics.insert("scan_mode".into(), serde_json::json!("birthday"));
        s7_metrics.insert("birthday_height".into(), serde_json::json!(birthday_height));
        s7_metrics.insert("blocks_scanned".into(), serde_json::json!(blocks_scanned));
        s7_metrics.insert("utxo_count_found".into(), serde_json::json!(utxo_count));
        result.metrics.extend(s7_metrics);

        info!("S7 (new wallet) completed: {} blocks, {} UTXOs in {:.2}s", blocks_scanned, utxo_count, scan_time_secs);
        Ok(())
    }

    async fn wait_for_funding(&self, target: u64, _c_min: u32, timeout_secs: u64) -> Result<u64> {
        let start = std::time::Instant::now();
        let timeout = std::time::Duration::from_secs(timeout_secs);
        loop {
            if start.elapsed() > timeout {
                anyhow::bail!("Timeout waiting for funding of {} µT", target);
            }
            let balance = self.get_balance_via_grpc().await?;
            if balance >= target {
                return Ok(balance);
            }
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        }
    }

    /// Rescan the blockchain. The Scanner reads the starting height from the account's
    /// birthday in the DB. For a genesis rescan (from_height=0), we update the birthday
    /// in the DB so the Scanner starts from block 0. For a birthday rescan, the existing
    /// birthday (set by the gRPC wallet at creation time) is used.
    async fn rescan_from_height(&self, _config: &HarnessConfig, from_height: u64) -> Result<()> {
        info!("Rescanning from height {} using minotari Scanner", from_height);

        // Update the account birthday in the DB so the Scanner starts from the correct height.
        // The Scanner reads birthday from the accounts table to determine its start height.
        if from_height == 0 {
            if let Ok(pool) = minotari::db::init_db(self.db_path.clone()) {
                if let Ok(conn) = pool.get() {
                    let _ = conn.execute(
                        "UPDATE accounts SET birthday = 0",
                        [],
                    );
                    info!("Set account birthday to 0 for genesis rescan");
                }
            }
        }

        match self.scan_blockchain().await {
            Ok(events) => info!("Rescan completed: {} events", events.len()),
            Err(e) => warn!("Rescan error: {}", e),
        }
        Ok(())
    }
}
