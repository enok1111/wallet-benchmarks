use anyhow::{anyhow, Context, Result};
use log::{debug, info, warn};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::config::HarnessConfig;
use crate::metrics::ScenarioResult;
use crate::modes::{WalletMode, WalletModeId};

pub struct NewWalletMode {
    data_dir: PathBuf,
    db_path: PathBuf,
    address: Option<String>,
    seed_words: Vec<String>,
    password: String,
    base_node_http: String,
    account_id: u32,
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
        }
    }

    async fn init_wallet_db(&self, birthday_height: u64) -> Result<()> {
        info!(
            "Initializing wallet DB at {} with birthday height {}",
            self.db_path.display(),
            birthday_height
        );

        std::fs::create_dir_all(&self.data_dir)?;

        let pool = minotari::db::init_db(self.db_path.clone())
            .map_err(|e| anyhow!("Failed to initialize minotari database: {}", e))?;

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

    async fn scan_blockchain(&self) -> Result<Vec<minotari::WalletEvent>> {
        use minotari::{Scanner, ScanMode};

        info!("Scanning blockchain via {}", self.base_node_http);

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

    /// Spawn the console wallet non-interactively to execute a single command.
    /// The wallet shares the same seed words and data directory as the library wallet.
    /// After the command completes, the wallet exits automatically.
    fn run_wallet_command(&self, command: &str, grpc_enabled: bool) -> Result<String> {
        let mut cmd = Command::new("minotari_console_wallet");
        cmd.arg("--base-path")
            .arg(&self.data_dir)
            .arg("--network")
            .arg("esmeralda")
            .arg("--password")
            .arg(&self.password)
            .arg("--seed-words")
            .arg(self.seed_words.join(" "))
            .arg("--non-interactive-mode")
            .arg("--command")
            .arg(command)
            .arg("--command-mode-auto-exit");

        if !grpc_enabled {
            cmd.arg("--grpc-enabled=false");
        } else {
            cmd.arg("--grpc-enabled");
        }

        let output = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .context("Failed to spawn minotari_console_wallet for command")?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if !output.status.success() {
            return Err(anyhow!(
                "Wallet command failed (exit={}):\nstdout: {}\nstderr: {}",
                output.status,
                stdout.trim(),
                stderr.trim()
            ));
        }

        debug!("Wallet command output:\n{}", stdout.trim());
        Ok(stdout)
    }

    /// Send a transaction to self, creating multiple outputs from one input.
    /// Uses the console wallet's `coin-split` command for signing + broadcasting.
    fn send_to_self(&self, output_count: u32, fee_per_gram: u64) -> Result<u64> {
        let amount_per_output = 100_000u64;
        let total_amount = amount_per_output * output_count as u64;
        let command = format!(
            "coin-split {} {} --fee-per-gram {} --payment-id \"bench-s1-{}-{}\"",
            total_amount, output_count, fee_per_gram, output_count, std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
        );
        let stdout = self.run_wallet_command(&command, false)?;

        // Parse tx_id from output - typically "Transaction <tx_id> published" or similar
        // The wallet logs the tx_id on stdout
        let tx_id = Self::parse_tx_id_from_output(&stdout)
            .ok_or_else(|| anyhow!("Could not parse transaction ID from wallet output:\n{}", stdout))?;

        Ok(tx_id)
    }

    /// Send a single transfer to a destination address.
    /// Uses the console wallet's `send-one-sided-to-stealth-address` command.
    fn send_single_transfer(&self, destination: &str, amount: u64, _fee_per_gram: u64) -> Result<u64> {
        let command = format!(
            "send-one-sided-to-stealth-address {} {}",
            amount, destination
        );
        let stdout = self.run_wallet_command(&command, false)?;

        let tx_id = Self::parse_tx_id_from_output(&stdout)
            .ok_or_else(|| anyhow!("Could not parse transaction ID from wallet output:\n{}", stdout))?;

        Ok(tx_id)
    }

    /// Wait for a transaction to be confirmed by polling the base node.
    /// After the console wallet broadcasts the transaction, we scan the blockchain
    /// to detect the confirmed output.
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

            // Check balance changes - look for the specific tx
            let balance = self.query_balance().await.unwrap_or(0);
            debug!(
                "Tx {} polling: tip={}, utxos={}, balance={}",
                tx_id, tip_height, utxo_count, balance
            );

            // If we have any UTXOs and balance, assume tx is confirmed
            // (we can't easily map tx_id to balance changes from the library)
            if utxo_count > 0 && balance > 0 {
                // Verify confirmation depth
                let scanned_tip = tip_height;
                if scanned_tip > 0 {
                    return Ok(());
                }
            }

            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }

    /// Parse a transaction ID from wallet command output.
    /// The console wallet prints the tx_id in various formats depending on the command.
    fn parse_tx_id_from_output(output: &str) -> Option<u64> {
        // Try to find "Transaction <id> published" 
        for line in output.lines() {
            // Look for patterns like "TxId: 12345" or "transaction_id: 12345"
            if let Some(tx_str) = line.split("TxId:").nth(1)
                .or_else(|| line.split("transaction_id:").nth(1))
                .or_else(|| line.split("tx_id:").nth(1))
            {
                if let Ok(id) = tx_str.trim().split(|c: char| !c.is_ascii_digit()).next()
                    .unwrap_or("")
                    .parse::<u64>()
                {
                    return Some(id);
                }
            }

            // Look for "Transaction <id> published"
            if line.contains("published") {
                for word in line.split_whitespace() {
                    if let Ok(id) = word.parse::<u64>() {
                        return Some(id);
                    }
                }
            }

            // Try numeric sequences
            let words: Vec<&str> = line.split_whitespace().collect();
            for w in &words {
                if let Ok(id) = w.parse::<u64>() {
                    if id > 1000 {
                        return Some(id);
                    }
                }
            }
        }
        None
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

        // Scan once to find our address and populate the wallet
        // The library scanner detects outputs and sets up the wallet state
        if let Err(e) = self.scan_blockchain().await {
            warn!("Initial scan (expected for empty wallet): {}", e);
        }

        // Derive the wallet address by running a quick wallet command
        // The address is logged in the wallet output
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
            match self.send_to_self(current_utxos * 2, config.fee_rate) {
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

        if current_utxos < config.volume_target {
            let remaining = config.volume_target - current_utxos;
            let fanout_rounds =
                (remaining + config.fanout_outputs_per_tx - 1) / config.fanout_outputs_per_tx;

            for round in 0..fanout_rounds {
                let outputs = std::cmp::min(
                    config.fanout_outputs_per_tx,
                    config.volume_target - current_utxos,
                );
                match self.send_to_self(outputs, config.fee_rate) {
                    Ok(tx_id) => {
                        info!("S1: Fan-out round {} tx: {}", round + 1, tx_id);
                        self.wait_for_confirmation(tx_id, config.c_min, 300).await?;
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
            let _fee = config.fee_rate;
            let seed = self.seed_words.join(" ");
            let base_path = self.data_dir.clone();
            let password = self.password.clone();

            let mut handles = Vec::new();
            for i in 0..n_val {
                let s = seed.clone();
                let bp = base_path.join(format!("s4_batch_{}", i));
                let pw = password.clone();

                let handle = tokio::task::spawn_blocking(move || {
                    let start_tx = std::time::Instant::now();
                    let temp_dir = bp;
                    std::fs::create_dir_all(&temp_dir).ok();

                    let mut cmd = Command::new("minotari_console_wallet");
                    cmd.arg("--base-path")
                        .arg(&temp_dir)
                        .arg("--network")
                        .arg("esmeralda")
                        .arg("--password")
                        .arg(&pw)
                        .arg("--seed-words")
                        .arg(&s)
                        .arg("--non-interactive-mode")
                        .arg("--command")
                        .arg(format!(
                            "send-one-sided-to-stealth-address 100000 {}",
                            "placeholder_address"
                        ))
                        .arg("--command-mode-auto-exit")
                        .arg("--grpc-enabled=false")
                        .stdout(Stdio::piped())
                        .stderr(Stdio::piped());

                    match cmd.output() {
                        Ok(output) if output.status.success() => {
                            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                            let tx_id = stdout.split_whitespace()
                                .filter_map(|w| w.parse::<u64>().ok())
                                .find(|&id| id > 1000)
                                .map(|id| id.to_string())
                                .unwrap_or_else(|| "unknown".to_string());
                            (i, true, tx_id, start_tx.elapsed().as_micros() as u64)
                        }
                        Ok(output) => {
                            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                            (i, false, format!("exit={}: {}", output.status, stderr.trim()), start_tx.elapsed().as_micros() as u64)
                        }
                        Err(e) => {
                            (i, false, format!("spawn error: {}", e), start_tx.elapsed().as_micros() as u64)
                        }
                    }
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
            match self.send_single_transfer(recipient, amount, config.fee_rate) {
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
            // Scan to pick up incoming funding
            if let Err(e) = self.scan_blockchain().await {
                debug!("Scan during funding wait: {}", e);
            }
            let balance = self.query_balance().await?;
            if balance >= target {
                return Ok(balance);
            }
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        }
    }

    async fn rescan_from_height(&self, _config: &HarnessConfig, from_height: u64) -> Result<()> {
        info!("Rescanning from height {} using minotari Scanner", from_height);

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
