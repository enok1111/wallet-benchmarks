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

    /// Generate a random 12-word seed phrase for testing
    fn generate_seed_words() -> Vec<String> {
        // In production, this would use proper BIP39 word list
        // For benchmarking, we use deterministic test words
        vec![
            "abandon".to_string(), "ability".to_string(), "able".to_string(), "about".to_string(), "above".to_string(), "absent".to_string(),
            "absorb".to_string(), "abstract".to_string(), "absurd".to_string(), "abuse".to_string(), "access".to_string(), "accident".to_string(),
        ]
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

        // Generate or use provided seed words
        if self.seed_words.is_empty() {
            self.seed_words = config
                .seed_words_old
                .clone()
                .unwrap_or_else(Self::generate_seed_words);
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
        _config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        // B0 - Baseline Scan (empty wallet)
        // Floor cost of block-walk + view-key check with nothing to store
        todo!("Implement B0 scenario for old wallet")
    }

    async fn run_s0(
        &mut self,
        _config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        // S0 - Funding Baseline
        // Init wallet, receive funding UTXO, wait for confirmation
        todo!("Implement S0 scenario for old wallet")
    }

    async fn run_s1(
        &mut self,
        _config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        // S1 - UTXO Build-up (doubling + fan-out → 512)
        todo!("Implement S1 scenario for old wallet")
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
