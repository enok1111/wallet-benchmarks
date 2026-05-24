//! Wallet mode implementations
//!
//! Three wallet modes as defined by the bounty:
//! - Old Wallet (minotari_console_wallet via gRPC)
//! - New Wallet (minotari-cli library with offline signing)
//! - Payment Processor (batch 1-to-many transactions)

pub mod old_wallet;
pub mod new_wallet;
pub mod payment_processor;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::config::HarnessConfig;
use crate::metrics::ScenarioResult;

/// Wallet mode trait - all modes implement this interface
#[async_trait::async_trait]
pub trait WalletMode {
    /// Initialize the wallet mode (setup data dirs, generate keys, etc.)
    async fn initialize(&mut self, config: &HarnessConfig) -> Result<()>;

    /// Run a specific scenario and return results
    async fn run_scenario(
        &mut self,
        scenario_id: &str,
        config: &HarnessConfig,
    ) -> Result<ScenarioResult>;

    /// Get the wallet address for this mode
    #[allow(dead_code)]
    fn get_address(&self) -> Option<&str>;

    /// Get the current balance in µT
    #[allow(dead_code)]
    async fn get_balance(&self) -> Result<u64>;

    /// Tear down the wallet mode (cleanup processes, temp dirs)
    async fn teardown(&mut self) -> Result<()>;
}

/// Identifies which wallet mode is active
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalletModeId {
    Old,
    New,
    PaymentProcessor,
}

impl WalletModeId {
    pub fn suffix_word(&self) -> &'static str {
        match self {
            WalletModeId::Old => "account",
            WalletModeId::New => "acquire",
            WalletModeId::PaymentProcessor => "actress",
        }
    }
}

/// Shared state for wallet mode execution
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletState {
    /// Wallet address
    pub address: String,
    /// Current balance in µT
    pub balance_ut: u64,
    /// Number of UTXOs
    pub utxo_count: u32,
    /// Birthday block height
    pub birthday_height: u64,
    /// Current tip height
    pub tip_height: u64,
}
