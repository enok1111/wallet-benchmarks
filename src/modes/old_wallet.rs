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
        _config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        // S2 - Scan from Genesis (checkpoint 1)
        todo!("Implement S2 scenario for old wallet")
    }

    async fn run_s3(
        &mut self,
        _config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        // S3 - Scan from Birthday (checkpoint 1)
        todo!("Implement S3 scenario for old wallet")
    }

    async fn run_s4(
        &mut self,
        _config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        // S4 - Concurrent Construction
        todo!("Implement S4 scenario for old wallet")
    }

    async fn run_s5(
        &mut self,
        _config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        // S5 - Payment Processor Throughput
        todo!("Implement S5 scenario for old wallet")
    }

    async fn run_s6(
        &mut self,
        _config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        // S6 - Scan from Genesis (checkpoint 2)
        todo!("Implement S6 scenario for old wallet")
    }

    async fn run_s7(
        &mut self,
        _config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        // S7 - Scan from Birthday (checkpoint 2)
        todo!("Implement S7 scenario for old wallet")
    }
}
