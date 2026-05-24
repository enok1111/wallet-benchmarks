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
    #[allow(dead_code)]
    pub fn suffix_word(&self) -> &'static str {
        match self {
            WalletModeId::Old => "account",
            WalletModeId::New => "acquire",
            WalletModeId::PaymentProcessor => "actress",
        }
    }
}

/// Generate valid Tari CipherSeed mnemonic words (24 words from BIP-39 wordlist).
///
/// Uses `tari_common_types::seeds::CipherSeed::random()` to generate a proper
/// CipherSeed (versioned, with birthday, entropy, salt, MAC, and CRC32 checksum)
/// and converts it to a mnemonic phrase. This is the **correct** way to generate
/// seed words for Tari wallets — the previous hardcoded BIP39 words with mode
/// suffix produced invalid mnemonics that would fail wallet validation.
///
/// Each call produces a fresh random seed (non-deterministic via kernel CSPRNG).
pub fn generate_tari_seed_words() -> Vec<String> {
    use tari_common_types::seeds::{
        cipher_seed::CipherSeed,
        mnemonic::{Mnemonic, MnemonicLanguage},
    };

    let seed = CipherSeed::random();
    let mnemonic = seed
        .to_mnemonic(MnemonicLanguage::English, None)
        .expect("CipherSeed to mnemonic conversion is infallible for English wordlist");

    let mut words = Vec::with_capacity(mnemonic.len());
    for i in 0..mnemonic.len() {
        words.push(
            mnemonic
                .get_word(i)
                .expect("Valid index within seed word count")
                .clone(),
        );
    }
    words
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
