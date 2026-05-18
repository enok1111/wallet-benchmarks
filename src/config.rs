//! Harness configuration
//!
//! Loads and validates benchmark parameters from TOML config file.
//! All configuration parameters are exposed and recorded in the result profile
//! as required by the bounty acceptance criteria.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;

/// Main harness configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessConfig {
    /// Esmeralda testnet base node gRPC address
    #[serde(default = "default_base_node_grpc")]
    pub base_node_grpc: String,

    /// Esmeralda testnet base node HTTP RPC address
    #[serde(default = "default_base_node_http")]
    pub base_node_http: String,

    /// Public Esmeralda testnet peer seeds for wallet connection
    #[serde(default)]
    pub peer_seeds: Vec<String>,

    // Wallet mode paths and settings
    /// Path to minotari_console_wallet binary (old wallet)
    #[serde(default = "default_old_wallet_path")]
    pub old_wallet_binary: String,

    /// Path to minotari CLI binary (new wallet)
    #[serde(default = "default_new_wallet_path")]
    pub new_wallet_binary: String,

    /// gRPC port for old wallet (each instance gets a unique port)
    #[serde(default = "default_old_wallet_grpc_base_port")]
    pub old_wallet_grpc_base_port: u16,

    // Scenario parameters
    /// Single funding UTXO amount per mode (~71% of one Esmeralda block reward)
    #[serde(default = "default_a_fund")]
    pub a_fund: u64,

    /// Minimum confirmation depth before UTXO is treated as spendable
    #[serde(default = "default_c_min")]
    pub c_min: u32,

    /// Target UTXO count after S1
    #[serde(default = "default_volume_target")]
    pub volume_target: u32,

    /// Number of doubling rounds (1 -> 64 UTXOs)
    #[serde(default = "default_doubling_rounds")]
    pub doubling_rounds: u8,

    /// Outputs per transaction in fan-out phase
    #[serde(default = "default_fanout_outputs_per_tx")]
    pub fanout_outputs_per_tx: u32,

    /// Concurrent batch sizes for S4 ramp test
    #[serde(default = "default_concurrent_batches")]
    pub concurrent_batches: Vec<usize>,

    /// Budget time per batch in S4 (seconds)
    #[serde(default = "default_s4_budget_seconds")]
    pub s4_t_budget: u64,

    /// Total recipients paid in S5
    #[serde(default = "default_s5_m")]
    pub s5_m: u32,

    /// Outputs per batch tx in S5
    #[serde(default = "default_s5_k")]
    pub s5_k: u32,

    /// Explicit fee rate passed to all modes (µT per gram)
    #[serde(default = "default_fee_rate")]
    pub fee_rate: u64,

    /// Wallet binary versions (pinned commit/tag)
    #[serde(default)]
    pub wallet_versions: WalletVersions,

    // Execution control
    /// Scenarios to run (default: all)
    #[serde(default = "default_scenarios")]
    pub scenarios: Vec<String>,

    /// Wallet modes to benchmark
    #[serde(default = "default_modes")]
    pub modes: Vec<String>,

    /// Password for wallet encryption (can also be set via env var)
    #[serde(default)]
    pub wallet_password: Option<String>,

    /// Seed words for old wallet mode (12-word mnemonic)
    #[serde(default)]
    pub seed_words_old: Option<Vec<String>>,

    /// Seed words for new wallet mode
    #[serde(default)]
    pub seed_words_new: Option<Vec<String>>,

    /// Seed words for payment processor mode
    #[serde(default)]
    pub seed_words_payment: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WalletVersions {
    /// minotari_console_wallet version/commit
    #[serde(default = "default_old_wallet_version")]
    pub old_wallet: String,
    /// minotari-cli version/commit
    #[serde(default = "default_new_wallet_version")]
    pub new_wallet: String,
    /// Base node version/commit
    #[serde(default = "default_base_node_version")]
    pub base_node: String,
}

impl Default for HarnessConfig {
    fn default() -> Self {
        Self {
            base_node_grpc: default_base_node_grpc(),
            base_node_http: default_base_node_http(),
            peer_seeds: Vec::new(),
            old_wallet_binary: default_old_wallet_path(),
            new_wallet_binary: default_new_wallet_path(),
            old_wallet_grpc_base_port: default_old_wallet_grpc_base_port(),
            a_fund: default_a_fund(),
            c_min: default_c_min(),
            volume_target: default_volume_target(),
            doubling_rounds: default_doubling_rounds(),
            fanout_outputs_per_tx: default_fanout_outputs_per_tx(),
            concurrent_batches: default_concurrent_batches(),
            s4_t_budget: default_s4_budget_seconds(),
            s5_m: default_s5_m(),
            s5_k: default_s5_k(),
            fee_rate: default_fee_rate(),
            wallet_versions: WalletVersions::default(),
            scenarios: default_scenarios(),
            modes: default_modes(),
            wallet_password: None,
            seed_words_old: None,
            seed_words_new: None,
            seed_words_payment: None,
        }
    }
}

// Default value functions
fn default_base_node_grpc() -> String {
    "http://127.0.0.1:18142".to_string()
}

fn default_base_node_http() -> String {
    "http://127.0.0.1:18142".to_string()
}

fn default_old_wallet_path() -> String {
    "minotari_console_wallet".to_string()
}

fn default_new_wallet_path() -> String {
    "minotari".to_string()
}

fn default_old_wallet_grpc_base_port() -> u16 {
    18200
}

fn default_a_fund() -> u64 {
    10_000_000_000 // 10,000 tXTM in µT (micro-Tari)
}

fn default_c_min() -> u32 {
    3
}

fn default_volume_target() -> u32 {
    512
}

fn default_doubling_rounds() -> u8 {
    6
}

fn default_fanout_outputs_per_tx() -> u32 {
    8
}

fn default_concurrent_batches() -> Vec<usize> {
    vec![8, 16, 32, 64, 128]
}

fn default_s4_budget_seconds() -> u64 {
    900 // 15 minutes
}

fn default_s5_m() -> u32 {
    100
}

fn default_s5_k() -> u32 {
    10
}

fn default_fee_rate() -> u64 {
    5 // µT per gram
}

fn default_scenarios() -> Vec<String> {
    vec![
        "B0".to_string(),
        "S0".to_string(),
        "S1".to_string(),
        "S2".to_string(),
        "S3".to_string(),
        "S4".to_string(),
        "S5".to_string(),
        "S6".to_string(),
        "S7".to_string(),
    ]
}

fn default_modes() -> Vec<String> {
    vec!["old".to_string(), "new".to_string(), "payment_processor".to_string()]
}

fn default_old_wallet_version() -> String {
    "v5.3.0-pre.1".to_string()
}

fn default_new_wallet_version() -> String {
    "main".to_string()
}

fn default_base_node_version() -> String {
    "v5.3.0-pre.1".to_string()
}

impl HarnessConfig {
    /// Load configuration from a TOML file.
    ///
    /// Fields with `#[serde(default)]` are automatically populated with defaults
    /// when missing from the config file, so no manual merging is needed.
    pub fn load(path: &str) -> Result<Self> {
        let path = Path::new(path);

        if !path.exists() {
            log::info!("Config file {} not found, using defaults", path.display());
            return Ok(Self::default());
        }

        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;

        // serde(default) handles missing fields automatically
        let parsed: HarnessConfig =
            toml::from_str(&content).with_context(|| "Failed to parse config file")?;

        Ok(parsed)
    }

    /// Validate configuration parameters
    pub fn validate(&self) -> Result<()> {
        // Validate scenarios are valid scenario IDs
        let valid_scenarios: HashSet<&str> = [
            "B0", "S0", "S1", "S2", "S3", "S4", "S5", "S6", "S7",
        ]
        .iter()
        .cloned()
        .collect();

        for scenario in &self.scenarios {
            if !valid_scenarios.contains(scenario.as_str()) {
                anyhow::bail!("Invalid scenario: {}", scenario);
            }
        }

        // Validate modes are valid mode names
        let valid_modes: HashSet<&str> = ["old", "new", "payment_processor"]
            .iter()
            .cloned()
            .collect();

        for mode in &self.modes {
            if !valid_modes.contains(mode.as_str()) {
                anyhow::bail!("Invalid mode: {}", mode);
            }
        }

        // Validate parameter ranges
        if self.a_fund == 0 {
            anyhow::bail!("a_fund must be > 0");
        }
        if self.c_min < 1 {
            anyhow::bail!("c_min must be >= 1");
        }
        if self.doubling_rounds < 1 || self.doubling_rounds > 20 {
            anyhow::bail!("doubling_rounds must be between 1 and 20");
        }
        if self.fanout_outputs_per_tx < 1 {
            anyhow::bail!("fanout_outputs_per_tx must be >= 1");
        }
        if self.concurrent_batches.is_empty() {
            anyhow::bail!("concurrent_batches must not be empty");
        }
        if self.s5_m == 0 || self.s5_k == 0 {
            anyhow::bail!("s5_m and s5_k must be > 0");
        }
        if self.fee_rate == 0 {
            anyhow::bail!("fee_rate must be > 0");
        }

        Ok(())
    }
}
