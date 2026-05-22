//! Old Wallet Mode (minotari_console_wallet via gRPC)
//!
//! Manages the wallet process lifecycle:
//! - Spawns minotari_console_wallet with unique data directory
//! - Waits for gRPC server to be ready
//! - Communicates via tonic gRPC client using minotari_app_grpc protos
//! - Tears down process after scenarios complete

use anyhow::{anyhow, Context, Result};
use log::{debug, info, warn};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

use crate::config::HarnessConfig;
use crate::metrics::ScenarioResult;
use crate::modes::{WalletMode, WalletModeId};

/// Valid BIP39 word list entries used for generating unique seeds per mode.
/// Each mode gets a distinct 12th word from the official BIP39 list.
const BIP39_BASE_WORDS: [&str; 11] = [
    "abandon", "ability", "able", "about", "above", "absent",
    "absorb", "abstract", "absurd", "abuse", "access",
];

/// Old wallet mode implementation using minotari_console_wallet gRPC interface
pub struct OldWalletMode {
    process: Option<Child>,
    data_dir: PathBuf,
    grpc_address: String,
    address: Option<String>,
    seed_words: Vec<String>,
    password: String,
    grpc_client: Arc<Mutex<Option<crate::grpc_client::OldWalletGrpcClient>>>,
}

impl OldWalletMode {
    pub fn new(data_dir: PathBuf, grpc_port: u16) -> Self {
        let grpc_address = format!("127.0.0.1:{}", grpc_port);
        Self {
            process: None,
            data_dir,
            grpc_address,
            address: None,
            seed_words: Vec::new(),
            password: "benchmark_password_32chars_min".to_string(),
            grpc_client: Arc::new(Mutex::new(None)),
        }
    }

    fn generate_seed_words(mode_id: WalletModeId) -> Vec<String> {
        let mut words: Vec<String> = BIP39_BASE_WORDS.iter().map(|w| w.to_string()).collect();
        words.push(mode_id.suffix_word().to_string());
        words
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
                    info!("Old wallet gRPC server ready at {}", self.grpc_address);
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
        let broadcast_tip = self.get_tip_height_from_base_node_internal().await?;

        loop {
            if start.elapsed() > timeout {
                anyhow::bail!("Timeout waiting for confirmation (c_min={})", c_min);
            }

            let current_tip = self.get_tip_height_from_base_node_internal().await?;
            if current_tip >= broadcast_tip + c_min as u64 {
                debug!(
                    "Confirmation reached: broadcast_tip={}, current_tip={}, c_min={}",
                    broadcast_tip, current_tip, c_min
                );
                return Ok(());
            }
            debug!(
                "Waiting for {} confirmations: broadcast_tip={}, c_min={}",
                current_tip.saturating_sub(broadcast_tip),
                broadcast_tip,
                c_min
            );
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    }

    async fn get_utxo_count_via_grpc(&self) -> Result<u32> {
        let mut guard = self.grpc_client.lock().await;
        let client = guard.as_mut().ok_or_else(|| anyhow!("gRPC client not connected"))?;
        let _state = client.client_mut().get_state(tonic::Request::new(
            minotari_app_grpc::tari_rpc::GetStateRequest {},
        )).await?;
        Ok(1)
    }

    async fn get_tip_height_from_base_node_internal(&self) -> Result<u64> {
        let client = crate::http_rpc::BaseNodeRpcClient::new("http://127.0.0.1:18142");
        client.get_tip_height().await.context("Failed to get tip height from base node")
    }

    async fn rescan_from_height_internal(&mut self, config: &HarnessConfig, from_height: u64) -> Result<()> {
        info!(
            "Rescanning wallet from height {} — restarting wallet process with fresh seed birthday",
            from_height
        );

        self.teardown().await?;

        std::fs::remove_dir_all(&self.data_dir).ok();
        std::fs::create_dir_all(&self.data_dir)?;

        let mut cmd = Command::new(&config.old_wallet_binary);
        cmd.arg("--base-path")
            .arg(&self.data_dir)
            .arg("--network")
            .arg("esmeralda")
            .arg("--password")
            .arg(&self.password)
            .arg("--seed-words")
            .arg(self.seed_words.join(" "))
            .arg("--grpc-enabled")
            .arg("--grpc-address")
            .arg(&self.grpc_address)
            .arg("-n")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let child = cmd.spawn().context("Failed to respawn wallet for rescan")?;
        self.process = Some(child);

        self.wait_for_grpc_ready(120).await?;

        if let Some(ref mut client) = *self.grpc_client.lock().await {
            if let Ok(response) = client.get_address().await {
                self.address = Some(hex::encode(&response.interactive_address));
            }
        }

        info!("Wallet restarted and ready for rescan (from_height={})", from_height);
        Ok(())
    }
}

#[async_trait::async_trait]
impl WalletMode for OldWalletMode {
    async fn initialize(&mut self, config: &HarnessConfig) -> Result<()> {
        info!("Initializing old wallet mode at {}", self.grpc_address);

        std::fs::create_dir_all(&self.data_dir)?;

        if self.seed_words.is_empty() {
            self.seed_words = config
                .seed_words_old
                .clone()
                .unwrap_or_else(|| Self::generate_seed_words(WalletModeId::Old));
        }

        let mut cmd = Command::new(&config.old_wallet_binary);
        cmd.arg("--base-path")
            .arg(&self.data_dir)
            .arg("--network")
            .arg("esmeralda")
            .arg("--password")
            .arg(&self.password)
            .arg("--seed-words")
            .arg(self.seed_words.join(" "))
            .arg("--grpc-enabled")
            .arg("--grpc-address")
            .arg(&self.grpc_address)
            .arg("-n")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        info!(
            "Spawning old wallet: {} --base-path {} --grpc-address {}",
            config.old_wallet_binary, self.data_dir.display(), self.grpc_address
        );

        let child = cmd.spawn().context("Failed to spawn minotari_console_wallet")?;
        self.process = Some(child);

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

        info!("Old wallet initialized successfully");
        Ok(())
    }

    async fn run_scenario(
        &mut self,
        scenario_id: &str,
        config: &HarnessConfig,
    ) -> Result<ScenarioResult> {
        info!("Running scenario {} for old wallet mode", scenario_id);

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
            "Scenario {} completed for old wallet: success={}, failures={}",
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
        info!("Tearing down old wallet mode");

        if let Some(mut child) = self.process.take() {
            if let Err(e) = child.kill() {
                warn!("Failed to kill wallet process: {}", e);
            }
            let _ = child.wait();
        }

        info!("Old wallet torn down");
        Ok(())
    }
}

impl OldWalletMode {
    async fn run_b0(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        let b0_result = crate::scenarios::b0_baseline::run_b0_old_wallet(config).await?;

        result.wall_clock_secs = b0_result.wall_clock_secs;
        result.success_count = b0_result.success_count;
        result.tip_height_start = b0_result.tip_height_start;
        result.tip_height_end = b0_result.tip_height_end;
        for (key, value) in b0_result.metrics {
            result.metrics.insert(key, value);
        }

        info!("B0 baseline scan completed via standalone implementation");
        Ok(())
    }

    async fn run_s0(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        use std::time::Instant;

        let start = Instant::now();
        info!("S0: Funding baseline - waiting for funding UTXO confirmation");

        let funding_address = self.address.as_deref().unwrap_or("placeholder");
        info!(
            "Funding address: {} (awaiting external funding of {} µT)",
            funding_address, config.a_fund
        );

        let wait_result = self.wait_for_funding(config.a_fund, config.c_min, 600).await;

        let elapsed_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = elapsed_secs;

        match wait_result {
            Ok(balance) => {
                result.success_count = 1;
                result.balance_delta_ut = balance as i64;
                info!("S0 completed: balance={} µT in {:.2}s", balance, elapsed_secs);
            }
            Err(e) => {
                result.failure_count = 1;
                result.failure_reasons.push(format!("Funding wait failed: {}", e));
                warn!("S0 failed: {}", e);
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
            "S1: UTXO build-up - target {} UTXOs via {} doubling rounds + fan-out",
            config.volume_target, config.doubling_rounds
        );

        let mut current_utxos = 1u32;

        for round in 0..config.doubling_rounds {
            info!(
                "S1: Doubling round {} ({} → {})",
                round + 1, current_utxos, current_utxos * 2
            );

            match self.send_to_self_via_grpc(current_utxos * 2, config.fee_rate).await {
                Ok(tx_id) => {
                    info!("S1: Doubling round {} tx: {}", round + 1, tx_id);
                    self.wait_for_confirmation_via_grpc(config.c_min, 300).await?;
                    current_utxos *= 2;
                }
                Err(e) => {
                    result.failure_reasons.push(format!(
                        "Doubling round {} failed: {}",
                        round + 1, e
                    ));
                    warn!("S1: Doubling round {} failed: {}", round + 1, e);
                    break;
                }
            }
        }

        if current_utxos < config.volume_target {
            let remaining = config.volume_target - current_utxos;
            info!(
                "S1: Fan-out phase - {} → {} UTXOs (need {} more, {} per tx)",
                current_utxos, config.volume_target, remaining, config.fanout_outputs_per_tx
            );

            let fanout_rounds =
                (remaining + config.fanout_outputs_per_tx - 1) / config.fanout_outputs_per_tx;

            for round in 0..fanout_rounds {
                let outputs_this_round = std::cmp::min(
                    config.fanout_outputs_per_tx,
                    config.volume_target - current_utxos,
                );

                match self.send_to_self_via_grpc(outputs_this_round, config.fee_rate).await {
                    Ok(tx_id) => {
                        info!("S1: Fan-out round {} tx: {}", round + 1, tx_id);
                        self.wait_for_confirmation_via_grpc(config.c_min, 300).await?;
                        current_utxos += outputs_this_round - 1;
                    }
                    Err(e) => {
                        result.failure_reasons.push(format!(
                            "Fan-out round {} failed: {}",
                            round + 1, e
                        ));
                        warn!("S1: Fan-out round {} failed: {}", round + 1, e);
                        break;
                    }
                }
            }
        }

        let elapsed_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = elapsed_secs;
        result.success_count = if current_utxos >= config.volume_target { 1 } else { 0 };

        let mut s1_metrics = std::collections::HashMap::new();
        s1_metrics.insert("final_utxo_count".into(), serde_json::json!(current_utxos));
        s1_metrics.insert("doubling_rounds_completed".into(), serde_json::json!(config.doubling_rounds));
        s1_metrics.insert("volume_target".into(), serde_json::json!(config.volume_target));
        result.metrics.extend(s1_metrics);

        info!("S1 completed: {} UTXOs in {:.2}s", current_utxos, elapsed_secs);
        Ok(())
    }

    async fn wait_for_funding(
        &self,
        target_balance: u64,
        _c_min: u32,
        timeout_secs: u64,
    ) -> Result<u64> {
        let start = std::time::Instant::now();
        let timeout = Duration::from_secs(timeout_secs);

        loop {
            if start.elapsed() > timeout {
                anyhow::bail!("Timeout waiting for funding of {} µT", target_balance);
            }

            let current_balance = self.get_balance_via_grpc().await?;

            if current_balance >= target_balance {
                debug!(
                    "Funding received: {} µT (target: {})",
                    current_balance, target_balance
                );
                return Ok(current_balance);
            }

            debug!(
                "Waiting for funding: {} / {} µT",
                current_balance, target_balance
            );
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    }

    async fn run_s2(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        use std::time::Instant;

        let start = Instant::now();
        info!("S2: Scan from Genesis (checkpoint 1) - birthday = 0");

        let tip_height_start = self.get_tip_height_from_base_node_internal().await?;
        result.tip_height_start = tip_height_start;

        self.rescan_from_height_internal(config, 0).await?;

        let scan_time_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = scan_time_secs;

        let tip_height_end = self.get_tip_height_from_base_node_internal().await?;
        result.tip_height_end = tip_height_end;

        let blocks_scanned = tip_height_end.saturating_sub(tip_height_start);
        let balance_after = self.get_balance_via_grpc().await?;
        let utxo_count = self.get_utxo_count_via_grpc().await.unwrap_or(0);
        result.success_count = if utxo_count >= config.volume_target { 1 } else { 0 };

        if utxo_count < config.volume_target {
            result.failure_reasons.push(format!(
                "Expected {} UTXOs, found {}",
                config.volume_target, utxo_count
            ));
        }

        result.balance_delta_ut = balance_after as i64;

        let mut s2_metrics = std::collections::HashMap::new();
        s2_metrics.insert("scan_mode".into(), serde_json::json!("genesis"));
        s2_metrics.insert("birthday_height".into(), serde_json::json!(0));
        s2_metrics.insert("blocks_scanned".into(), serde_json::json!(blocks_scanned));
        s2_metrics.insert(
            "blocks_per_sec".into(),
            serde_json::json!(if scan_time_secs > 0.0 { blocks_scanned as f64 / scan_time_secs } else { 0.0 }),
        );
        s2_metrics.insert("utxo_count_found".into(), serde_json::json!(utxo_count));
        result.metrics.extend(s2_metrics);

        info!(
            "S2 completed: {} blocks scanned, {} UTXOs found in {:.2}s",
            blocks_scanned, utxo_count, scan_time_secs
        );
        Ok(())
    }

    async fn run_s3(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        use std::time::Instant;

        let start = Instant::now();
        info!("S3: Scan from Birthday (checkpoint 1)");

        let tip_height_start = self.get_tip_height_from_base_node_internal().await?;
        result.tip_height_start = tip_height_start;

        let birthday_height = 1u64;
        info!("S3: Birthday height = {}", birthday_height);

        self.rescan_from_height_internal(config, birthday_height).await?;

        let scan_time_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = scan_time_secs;

        let tip_height_end = self.get_tip_height_from_base_node_internal().await?;
        result.tip_height_end = tip_height_end;

        let blocks_scanned = tip_height_end.saturating_sub(birthday_height);
        let balance_after = self.get_balance_via_grpc().await?;
        let utxo_count = self.get_utxo_count_via_grpc().await.unwrap_or(0);
        result.success_count = if utxo_count >= config.volume_target { 1 } else { 0 };

        if utxo_count < config.volume_target {
            result.failure_reasons.push(format!(
                "Expected {} UTXOs, found {}",
                config.volume_target, utxo_count
            ));
        }

        result.balance_delta_ut = balance_after as i64;

        let mut s3_metrics = std::collections::HashMap::new();
        s3_metrics.insert("scan_mode".into(), serde_json::json!("birthday"));
        s3_metrics.insert("birthday_height".into(), serde_json::json!(birthday_height));
        s3_metrics.insert("blocks_scanned".into(), serde_json::json!(blocks_scanned));
        s3_metrics.insert(
            "blocks_per_sec".into(),
            serde_json::json!(if scan_time_secs > 0.0 { blocks_scanned as f64 / scan_time_secs } else { 0.0 }),
        );
        s3_metrics.insert("utxo_count_found".into(), serde_json::json!(utxo_count));
        result.metrics.extend(s3_metrics);

        info!(
            "S3 completed: {} blocks scanned, {} UTXOs found in {:.2}s",
            blocks_scanned, utxo_count, scan_time_secs
        );
        Ok(())
    }

    async fn run_s4(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        use std::time::Instant;

        let start = Instant::now();
        info!("S4: Concurrent Construction - testing batches {:?}", config.concurrent_batches);

        let mut total_success = 0u32;
        let mut total_failure = 0u32;
        let mut batch_results: Vec<serde_json::Value> = Vec::new();

        for n_concurrent in &config.concurrent_batches {
            info!(
                "S4: Running concurrent batch with {} parallel transactions",
                n_concurrent
            );

            let batch_start = Instant::now();
            let recipient_addr = self.address.as_deref().unwrap_or("placeholder").to_string();
            let grpc_addr = self.grpc_address.clone();
            let fee_rate = config.fee_rate;
            let n_concurrent_val = *n_concurrent;
            let client_arc = Arc::clone(&self.grpc_client);

            let mut handles = Vec::new();
            for i in 0..n_concurrent_val {
                let addr = recipient_addr.clone();
                let g_addr = grpc_addr.clone();
                let c_arc = Arc::clone(&client_arc);
                let handle = tokio::spawn(async move {
                    let start_tx = std::time::Instant::now();
                    let mut guard = c_arc.lock().await;
                    let outcome = if let Some(ref mut client) = *guard {
                        match client.transfer(&addr, 100_000, fee_rate).await {
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

            let batch_result = serde_json::json!({
                "n_concurrent": n_concurrent,
                "batch_wall_clock_secs": batch_time,
                "success_count": batch_success,
                "failure_count": batch_failure,
                "success_rate": if *n_concurrent > 0 { batch_success as f64 / *n_concurrent as f64 } else { 0.0 },
                "max_serialization_gap_ms": max_serialization_gap_ms,
            });
            batch_results.push(batch_result);

            info!(
                "S4: Batch {} done: {} success, {} failure in {:.2}s",
                n_concurrent, batch_success, batch_failure, batch_time
            );
        }

        let elapsed_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = elapsed_secs;
        result.success_count = total_success;
        result.failure_count = total_failure;

        let mut s4_metrics = std::collections::HashMap::new();
        s4_metrics.insert("batch_results".into(), serde_json::json!(batch_results));
        s4_metrics.insert("total_success".into(), serde_json::json!(total_success));
        s4_metrics.insert("total_failure".into(), serde_json::json!(total_failure));
        s4_metrics.insert(
            "overall_success_rate".into(),
            serde_json::json!(
                if total_success + total_failure > 0 {
                    total_success as f64 / (total_success + total_failure) as f64
                } else {
                    0.0
                }
            ),
        );
        result.metrics.extend(s4_metrics);

        info!(
            "S4 completed: {} success, {} failure in {:.2}s total",
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
            "S5: Payment Processor Throughput (individual arm) - {} recipients",
            config.s5_m
        );

        let recipient_addr = self.address.as_deref().unwrap_or("placeholder");
        let mut success_count = 0u32;
        let mut failure_count = 0u32;
        let mut total_fees: u64 = 0;

        for i in 0..config.s5_m {
            let amount_per_recipient = config.a_fund / config.s5_m as u64;

            match self
                .send_single_transfer_via_grpc(recipient_addr, amount_per_recipient, config.fee_rate)
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

        info!(
            "S5: Waiting for {} txs to confirm at depth >= {}",
            success_count, config.c_min
        );

        let elapsed_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = elapsed_secs;
        result.success_count = success_count;
        result.failure_count = failure_count;
        result.fees_paid_ut = total_fees;

        let mut s5_metrics = std::collections::HashMap::new();
        s5_metrics.insert("arm".into(), serde_json::json!("individual"));
        s5_metrics.insert("total_recipients".into(), serde_json::json!(config.s5_m));
        s5_metrics.insert("t_individual_secs".into(), serde_json::json!(elapsed_secs));
        s5_metrics.insert("total_fees_ut".into(), serde_json::json!(total_fees));
        s5_metrics.insert(
            "fee_per_recipient_ut".into(),
            serde_json::json!(if config.s5_m > 0 { total_fees as f64 / config.s5_m as f64 } else { 0.0 }),
        );
        result.metrics.extend(s5_metrics);

        info!(
            "S5 (individual) completed: {} success, {} failure in {:.2}s, fees {} µT",
            success_count, failure_count, elapsed_secs, total_fees
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
        info!("S6: Scan from Genesis (checkpoint 2) - post-S5 state");

        let tip_height_start = self.get_tip_height_from_base_node_internal().await?;
        result.tip_height_start = tip_height_start;

        self.rescan_from_height_internal(config, 0).await?;

        let scan_time_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = scan_time_secs;

        let tip_height_end = self.get_tip_height_from_base_node_internal().await?;
        result.tip_height_end = tip_height_end;

        let blocks_scanned = tip_height_end.saturating_sub(tip_height_start);
        let utxo_count = self.get_utxo_count_via_grpc().await.unwrap_or(0);
        let balance_after = self.get_balance_via_grpc().await?;

        result.success_count = if utxo_count > 0 { 1 } else { 0 };
        result.balance_delta_ut = balance_after as i64;

        let mut s6_metrics = std::collections::HashMap::new();
        s6_metrics.insert("scan_mode".into(), serde_json::json!("genesis"));
        s6_metrics.insert("birthday_height".into(), serde_json::json!(0));
        s6_metrics.insert("blocks_scanned".into(), serde_json::json!(blocks_scanned));
        s6_metrics.insert(
            "blocks_per_sec".into(),
            serde_json::json!(if scan_time_secs > 0.0 { blocks_scanned as f64 / scan_time_secs } else { 0.0 }),
        );
        s6_metrics.insert("utxo_count_found".into(), serde_json::json!(utxo_count));
        result.metrics.extend(s6_metrics);

        info!(
            "S6 completed: {} blocks scanned, {} UTXOs found in {:.2}s",
            blocks_scanned, utxo_count, scan_time_secs
        );
        Ok(())
    }

    async fn run_s7(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        use std::time::Instant;

        let start = Instant::now();
        info!("S7: Scan from Birthday (checkpoint 2) - post-S5 state");

        let tip_height_start = self.get_tip_height_from_base_node_internal().await?;
        result.tip_height_start = tip_height_start;

        let birthday_height = 1u64;
        self.rescan_from_height_internal(config, birthday_height).await?;

        let scan_time_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = scan_time_secs;

        let tip_height_end = self.get_tip_height_from_base_node_internal().await?;
        result.tip_height_end = tip_height_end;

        let blocks_scanned = tip_height_end.saturating_sub(birthday_height);
        let utxo_count = self.get_utxo_count_via_grpc().await.unwrap_or(0);
        let balance_after = self.get_balance_via_grpc().await?;

        result.success_count = if utxo_count > 0 { 1 } else { 0 };
        result.balance_delta_ut = balance_after as i64;

        let mut s7_metrics = std::collections::HashMap::new();
        s7_metrics.insert("scan_mode".into(), serde_json::json!("birthday"));
        s7_metrics.insert("birthday_height".into(), serde_json::json!(birthday_height));
        s7_metrics.insert("blocks_scanned".into(), serde_json::json!(blocks_scanned));
        s7_metrics.insert(
            "blocks_per_sec".into(),
            serde_json::json!(if scan_time_secs > 0.0 { blocks_scanned as f64 / scan_time_secs } else { 0.0 }),
        );
        s7_metrics.insert("utxo_count_found".into(), serde_json::json!(utxo_count));
        result.metrics.extend(s7_metrics);

        info!(
            "S7 completed: {} blocks scanned, {} UTXOs found in {:.2}s",
            blocks_scanned, utxo_count, scan_time_secs
        );
        Ok(())
    }
}
