//! Old Wallet Mode (minotari_console_wallet via gRPC)
//!
//! Manages the wallet process lifecycle:
//! - Spawns minotari_console_wallet with unique data directory
//! - Waits for gRPC server to be ready
//! - Communicates via tonic gRPC client using minotari_app_grpc protos
//! - Tears down process after scenarios complete

use anyhow::{anyhow, Context, Result};
use log::{debug, info, warn};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use crate::config::HarnessConfig;
use crate::metrics::ScenarioResult;
use crate::modes::WalletMode;

/// Old wallet mode implementation using minotari_console_wallet gRPC interface
pub struct OldWalletMode {
    /// Process handle for the wallet instance
    process: Option<Child>,
    /// Data directory path for this wallet instance
    data_dir: PathBuf,
    /// gRPC address the wallet is listening on
    grpc_address: String,
    /// Wallet address (retrieved after initialization)
    address: Option<String>,
    /// Seed words for this wallet
    seed_words: Vec<String>,
    /// Password for wallet encryption
    password: String,
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
        }
    }

    /// Generate a unique 12-word seed phrase for this mode.
    /// Each mode gets a different seed to avoid cryptographic collisions
    /// when running sequentially or concurrently on the same network.
    fn generate_seed_words(mode_suffix: &str) -> Vec<String> {
        // Base BIP39 words - each mode appends a unique suffix word
        let mut words = vec![
            "abandon".to_string(), "ability".to_string(), "able".to_string(), "about".to_string(),
            "above".to_string(), "absent".to_string(), "absorb".to_string(), "abstract".to_string(),
            "absurd".to_string(), "abuse".to_string(), "access".to_string(),
        ];
        // Unique suffix word per mode to ensure different keys
        words.push(format!("{}{}", "accident", mode_suffix));
        words
    }

    /// Wait for gRPC server to be ready
    async fn wait_for_grpc_ready(&self, timeout_secs: u64) -> Result<()> {
        let start = std::time::Instant::now();
        let timeout = Duration::from_secs(timeout_secs);

        loop {
            if start.elapsed() > timeout {
                return Err(anyhow!("Timeout waiting for gRPC server to be ready"));
            }

            // Try to connect via health check or simple probe
            match self.probe_grpc().await {
                Ok(true) => {
                    info!("Old wallet gRPC server ready at {}", self.grpc_address);
                    return Ok(());
                }
                Ok(false) => {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                Err(e) => {
                    debug!("gRPC probe failed: {}", e);
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            }
        }
    }

    /// Probe gRPC server readiness
    async fn probe_grpc(&self) -> Result<bool> {
        // Use tonic to attempt a connection and call GetVersion or similar
        // This will be implemented once we have the proto definitions compiled
        Ok(false)
    }
}

#[async_trait::async_trait]
impl WalletMode for OldWalletMode {
    async fn initialize(&mut self, config: &HarnessConfig) -> Result<()> {
        info!("Initializing old wallet mode at {}", self.grpc_address);

        // Create data directory
        std::fs::create_dir_all(&self.data_dir)?;

        // Generate or use provided seed words (unique per mode)
        if self.seed_words.is_empty() {
            self.seed_words = config
                .seed_words_old
                .clone()
                .unwrap_or_else(|| Self::generate_seed_words("old"));
        }

        // Build command to spawn minotari_console_wallet
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
            .arg("-n") // Non-interactive mode
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        info!(
            "Spawning old wallet: {} --base-path {} --grpc-address {}",
            config.old_wallet_binary, self.data_dir.display(), self.grpc_address
        );

        let child = cmd
            .spawn()
            .context("Failed to spawn minotari_console_wallet")?;

        self.process = Some(child);

        // Wait for gRPC server to be ready
        self.wait_for_grpc_ready(60).await?;

        // Retrieve wallet address after initialization
        // This would use the gRPC client to call GetAddress or similar
        self.address = Some("placeholder_address".to_string());

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

        // Record tip height at start
        // result.tip_height_start = self.get_tip_height().await?;

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

        // Record tip height at end
        // result.tip_height_end = self.get_tip_height().await?;

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
        // Use gRPC client to call GetBalance
        // This will be implemented with tonic + minotari_app_grpc protos
        Ok(0)
    }

    async fn teardown(&mut self) -> Result<()> {
        info!("Tearing down old wallet mode");

        if let Some(mut child) = self.process.take() {
            // Gracefully terminate the process
            if let Err(e) = child.kill() {
                warn!("Failed to kill wallet process: {}", e);
            }
            let _ = child.wait();
        }

        info!("Old wallet torn down");
        Ok(())
    }
}

// Scenario implementations for old wallet mode
impl OldWalletMode {
    async fn run_b0(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        // B0 - Baseline Scan (empty wallet)
        // Delegate to standalone implementation that creates its own temp wallet
        let b0_result = crate::scenarios::b0_baseline::run_b0_old_wallet(config).await?;

        // Merge metrics into our result
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
        // S0 - Funding Baseline
        // 1. Wallet is already initialized in `initialize()`
        // 2. Receive funding UTXO (external funding to wallet address)
        // 3. Wait for C_min confirmations
        use std::time::Instant;

        let start = Instant::now();
        info!("S0: Funding baseline - waiting for funding UTXO confirmation");

        let funding_address = self.address.as_deref().unwrap_or("placeholder");
        info!(
            "Funding address: {} (awaiting external funding of {} µT)",
            funding_address, config.a_fund
        );

        // Wait for funding transaction to be received and confirmed
        // In production: poll GetBalance via gRPC until balance >= a_fund with c_min confirmations
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
        // S1 - UTXO Build-up (doubling + fan-out → volume_target)
        use std::time::Instant;

        let start = Instant::now();
        info!(
            "S1: UTXO build-up - target {} UTXOs via {} doubling rounds + fan-out",
            config.volume_target, config.doubling_rounds
        );

        // Phase 1: Doubling rounds (1 → 2 → 4 → 8 → ... → 2^rounds)
        let mut current_utxos = 1; // Start with funding UTXO

        for round in 0..config.doubling_rounds {
            info!(
                "S1: Doubling round {} ({} → {})",
                round + 1, current_utxos, current_utxos * 2
            );

            // Send to self: spend all UTXOs, create 2x outputs
            match self.send_to_self(current_utxos * 2, config.fee_rate).await {
                Ok(tx_id) => {
                    info!("S1: Doubling round {} tx: {}", round + 1, tx_id);
                    self.wait_for_confirmation(config.c_min, 300).await?;
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

        // Phase 2: Fan-out to reach volume_target
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

                match self.send_to_self(outputs_this_round, config.fee_rate).await {
                    Ok(tx_id) => {
                        info!("S1: Fan-out round {} tx: {}", round + 1, tx_id);
                        self.wait_for_confirmation(config.c_min, 300).await?;
                        current_utxos += outputs_this_round - 1; // -1 for change consumed
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

        // Record UTXO count metrics
        use std::collections::HashMap;
        let mut s1_metrics: HashMap<String, serde_json::Value> = HashMap::new();
        s1_metrics.insert(
            "final_utxo_count".into(),
            serde_json::json!(current_utxos),
        );
        s1_metrics.insert(
            "doubling_rounds_completed".into(),
            serde_json::json!(config.doubling_rounds),
        );
        s1_metrics.insert(
            "volume_target".into(),
            serde_json::json!(config.volume_target),
        );
        result.metrics.extend(s1_metrics);

        info!("S1 completed: {} UTXOs in {:.2}s", current_utxos, elapsed_secs);
        Ok(())
    }

    // Helper methods for S0/S1 scenarios

    /// Wait for funding transaction to be received and confirmed
    async fn wait_for_funding(
        &self,
        target_balance: u64,
        c_min: u32,
        timeout_secs: u64,
    ) -> Result<u64> {
        let start = std::time::Instant::now();
        let timeout = Duration::from_secs(timeout_secs);

        loop {
            if start.elapsed() > timeout {
                anyhow::bail!("Timeout waiting for funding of {} µT", target_balance);
            }

            // TODO: Implement gRPC call to GetBalance
            // For now, return 0 (will be replaced with actual balance query)
            let current_balance = self.get_balance().await?;

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

    /// Send to self transaction (split UTXOs)
    async fn send_to_self(&self, output_count: u32, fee_rate: u64) -> Result<String> {
        // TODO: Implement gRPC call to create and broadcast self-send transaction
        // This would use Transfer or a custom RPC endpoint
        debug!(
            "Sending to self: {} outputs at fee rate {} µT/g",
            output_count, fee_rate
        );

        // Placeholder - returns fake tx ID
        Ok(format!("tx_self_{}_outputs", output_count))
    }

    /// Wait for transaction confirmation at depth c_min
    async fn wait_for_confirmation(&self, c_min: u32, timeout_secs: u64) -> Result<()> {
        let start = std::time::Instant::now();
        let timeout = Duration::from_secs(timeout_secs);

        loop {
            if start.elapsed() > timeout {
                anyhow::bail!("Timeout waiting for confirmation (c_min={})", c_min);
            }

            // TODO: Implement gRPC call to check transaction confirmation depth
            debug!("Waiting for confirmation at depth {}", c_min);
            tokio::time::sleep(Duration::from_secs(10)).await;

            // Placeholder - assume confirmed after delay
            return Ok(());
        }
    }

    async fn run_s2(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        // S2 - Scan from Genesis (checkpoint 1)
        // Preconditions: S1 complete; wipe wallet data dir; birthday = 0.
        // Steps: launch scan from genesis -> wait for height == tip.
        // Verification: 512 spendable UTXOs rediscovered; balance matches post-S1.
        use std::time::Instant;

        let start = Instant::now();
        info!("S2: Scan from Genesis (checkpoint 1) - birthday = 0");

        // Record chain tip at start
        let tip_height_start = self.get_tip_height_from_base_node(config).await?;
        result.tip_height_start = tip_height_start;

        // Wipe wallet data dir and reinitialize with birthday = 0
        self.rescan_from_height(config, 0).await?;

        let scan_time_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = scan_time_secs;

        // Get final state
        let tip_height_end = self.get_tip_height_from_base_node(config).await?;
        result.tip_height_end = tip_height_end;

        let blocks_scanned = tip_height_end.saturating_sub(tip_height_start);

        // Get balance after scan
        let balance_after = self.get_balance().await?;

        // Verification: should find 512 UTXOs
        let utxo_count = self.get_utxo_count().await.unwrap_or(0);
        result.success_count = if utxo_count >= config.volume_target { 1 } else { 0 };

        if utxo_count < config.volume_target {
            result.failure_reasons.push(format!(
                "Expected {} UTXOs, found {}",
                config.volume_target, utxo_count
            ));
        }

        result.balance_delta_ut = balance_after as i64;

        // Record scan metrics
        let mut s2_metrics: HashMap<String, serde_json::Value> = HashMap::new();
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
        // S3 - Scan from Birthday (checkpoint 1)
        // Identical to S2 except birthday = H_birth.
        // blocks_scanned = H_tip_end - H_birth.
        use std::time::Instant;

        let start = Instant::now();
        info!("S3: Scan from Birthday (checkpoint 1)");

        // Record chain tip at start
        let tip_height_start = self.get_tip_height_from_base_node(config).await?;
        result.tip_height_start = tip_height_start;

        // Birthday = genesis height + 1 (wallet birthday)
        let birthday_height = 1; // genesis height + 1
        info!("S3: Birthday height = {}", birthday_height);

        // Wipe wallet data dir and reinitialize with birthday
        self.rescan_from_height(config, birthday_height).await?;

        let scan_time_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = scan_time_secs;

        // Get final state
        let tip_height_end = self.get_tip_height_from_base_node(config).await?;
        result.tip_height_end = tip_height_end;

        let blocks_scanned = tip_height_end.saturating_sub(birthday_height);

        // Get balance after scan
        let balance_after = self.get_balance().await?;

        // Verification: should find 512 UTXOs
        let utxo_count = self.get_utxo_count().await.unwrap_or(0);
        result.success_count = if utxo_count >= config.volume_target { 1 } else { 0 };

        if utxo_count < config.volume_target {
            result.failure_reasons.push(format!(
                "Expected {} UTXOs, found {}",
                config.volume_target, utxo_count
            ));
        }

        result.balance_delta_ut = balance_after as i64;

        // Record scan metrics
        let mut s3_metrics: HashMap<String, serde_json::Value> = HashMap::new();
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
        // S4 - Concurrent Construction
        // Measures what each wallet mode does under concurrent load.
        // Locking, serialization, selection contention, and stalls are the signal.
        // No retry. No backoff. No UTXO pre-partitioning.
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

            // Generate recipient addresses (self-sends OK for benchmark)
            let recipient_addr = self.address.as_deref().unwrap_or("placeholder");

            // Fire all construction+broadcast calls in parallel
            let mut handles = Vec::new();
            let n_concurrent_val = *n_concurrent; // Clone to owned value for spawn
            for i in 0..n_concurrent_val {
                let addr = recipient_addr.to_string();
                let fee_rate = config.fee_rate;
                let nc = n_concurrent_val;
                // TODO: Replace with actual gRPC Transfer call
                let handle = tokio::spawn(async move {
                    // Simulate transaction construction + broadcast
                    // In production, this would be a real gRPC call
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    let _ = (addr, fee_rate); // suppress unused warnings
                    (i, true, format!("tx_s4_{}_{}", nc, i))
                });
                handles.push(handle);
            }

            // Collect results - no retry, no backoff
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

            let batch_result = serde_json::json!({
                "n_concurrent": n_concurrent,
                "batch_wall_clock_secs": batch_time,
                "success_count": batch_success,
                "failure_count": batch_failure,
                "success_rate": if *n_concurrent > 0 { batch_success as f64 / *n_concurrent as f64 } else { 0.0 },
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

        // Record concurrency metrics
        let mut s4_metrics: HashMap<String, serde_json::Value> = HashMap::new();
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
        // S5 - Payment Processor Throughput (individual arm for old wallet)
        // Submit M = 100 single-output txs, back-to-back.
        // Wait for all confirmed at depth >= C_min.
        // Record T_individual.
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

        // Submit M single-output txs back-to-back
        for i in 0..config.s5_m {
            let amount_per_recipient = config.a_fund / config.s5_m as u64;

            // TODO: Replace with actual gRPC Transfer call
            match self
                .send_single_transfer(recipient_addr, amount_per_recipient, config.fee_rate)
                .await
            {
                Ok(_tx_id) => {
                    success_count += 1;
                    total_fees += config.fee_rate; // approximate
                }
                Err(e) => {
                    failure_count += 1;
                    result.failure_reasons.push(format!("Transfer {} failed: {}", i, e));
                }
            }
        }

        // Wait for all confirmed at depth >= C_min
        // TODO: Implement actual confirmation wait via gRPC
        info!(
            "S5: Waiting for {} txs to confirm at depth >= {}",
            success_count, config.c_min
        );

        let elapsed_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = elapsed_secs;
        result.success_count = success_count;
        result.failure_count = failure_count;
        result.fees_paid_ut = total_fees;

        // Record throughput metrics
        let mut s5_metrics: HashMap<String, serde_json::Value> = HashMap::new();
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
        // S6 - Scan from Genesis (checkpoint 2)
        // Same shape as S2 but after S5. Expected history ~250+ txs.
        use std::time::Instant;

        let start = Instant::now();
        info!("S6: Scan from Genesis (checkpoint 2) - post-S5 state");

        let tip_height_start = self.get_tip_height_from_base_node(config).await?;
        result.tip_height_start = tip_height_start;

        // Wipe and rescan from genesis
        self.rescan_from_height(config, 0).await?;

        let scan_time_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = scan_time_secs;

        let tip_height_end = self.get_tip_height_from_base_node(config).await?;
        result.tip_height_end = tip_height_end;

        let blocks_scanned = tip_height_end.saturating_sub(tip_height_start);
        let utxo_count = self.get_utxo_count().await.unwrap_or(0);
        let balance_after = self.get_balance().await?;

        result.success_count = if utxo_count > 0 { 1 } else { 0 };
        result.balance_delta_ut = balance_after as i64;

        let mut s6_metrics: HashMap<String, serde_json::Value> = HashMap::new();
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
        // S7 - Scan from Birthday (checkpoint 2)
        // Same shape as S3 but after S5.
        use std::time::Instant;

        let start = Instant::now();
        info!("S7: Scan from Birthday (checkpoint 2) - post-S5 state");

        let tip_height_start = self.get_tip_height_from_base_node(config).await?;
        result.tip_height_start = tip_height_start;

        let birthday_height = 1; // genesis height + 1
        self.rescan_from_height(config, birthday_height).await?;

        let scan_time_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = scan_time_secs;

        let tip_height_end = self.get_tip_height_from_base_node(config).await?;
        result.tip_height_end = tip_height_end;

        let blocks_scanned = tip_height_end.saturating_sub(birthday_height);
        let utxo_count = self.get_utxo_count().await.unwrap_or(0);
        let balance_after = self.get_balance().await?;

        result.success_count = if utxo_count > 0 { 1 } else { 0 };
        result.balance_delta_ut = balance_after as i64;

        let mut s7_metrics: HashMap<String, serde_json::Value> = HashMap::new();
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

    // Additional helper methods for S2-S7 scenarios

    /// Get tip height from base node via HTTP RPC
    async fn get_tip_height_from_base_node(&self, config: &HarnessConfig) -> Result<u64> {
        let client = crate::http_rpc::BaseNodeRpcClient::new(&config.base_node_http);
        client
            .get_tip_height()
            .await
            .context("Failed to get tip height from base node")
    }

    /// Rescan wallet from a specific height (wipes data dir and reinitializes)
    async fn rescan_from_height(&mut self, config: &HarnessConfig, from_height: u64) -> Result<()> {
        // TODO: Implement via gRPC RescanWallet call
        // For now, trigger a rescan via the wallet subprocess
        info!(
            "Rescanning wallet from height {} (gRPC integration pending)",
            from_height
        );

        // Placeholder: trigger rescan via gRPC
        // In production: use minotari_app_grpc::WalletClient::rescan_wallet
        Ok(())
    }

    /// Get UTXO count from wallet
    async fn get_utxo_count(&self) -> Result<u32> {
        // TODO: Implement via gRPC GetState or GetAllCompletedTransactions
        // Count unspent outputs
        debug!("Getting UTXO count via gRPC (pending implementation)");
        Ok(0)
    }

    /// Send a single transfer to a recipient
    async fn send_single_transfer(
        &self,
        destination: &str,
        amount: u64,
        fee_per_gram: u64,
    ) -> Result<String> {
        // TODO: Implement via gRPC Transfer call
        debug!(
            "Sending {} µT to {} at fee {} µT/g (gRPC pending)",
            amount, destination, fee_per_gram
        );
        Ok(format!("tx_s5_{}", amount))
    }
}
