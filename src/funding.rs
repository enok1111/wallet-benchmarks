//! Funding pre-flight check for wallet benchmarks
//!
//! Ensures all three wallet modes have sufficient funds before running scenarios.
//! Checks wallet balances via gRPC `GetBalance` and bails early if any wallet
//! is underfunded, preventing wasted benchmark runs.

use anyhow::{bail, Result};
use log::{info, warn};

/// Minimum funding required per wallet in µT (default: 100 XTM)
pub const MIN_FUNDING_PER_WALLET_UT: u64 = 100_000_000; // 100 XTM

/// Minimum UTXO count required per wallet for meaningful benchmarks
pub const MIN_UTXO_COUNT: u32 = 1;

/// Per-wallet funding status
#[derive(Debug, Clone)]
pub struct WalletFundingStatus {
    pub name: String,
    pub balance_ut: u64,
    pub min_required_ut: u64,
    pub utxo_count: u32,
    pub is_adequate: bool,
}

/// Funding pre-flight check result
#[derive(Debug, Clone)]
pub struct FundingPreflightResult {
    pub wallets: Vec<WalletFundingStatus>,
    pub all_adequate: bool,
}

/// Check that a single wallet has sufficient funds.
pub fn check_wallet_funding(
    name: &str,
    balance_ut: u64,
    utxo_count: u32,
    min_required_ut: Option<u64>,
) -> WalletFundingStatus {
    let min_req = min_required_ut.unwrap_or(MIN_FUNDING_PER_WALLET_UT);
    let adequate = balance_ut >= min_req && utxo_count >= MIN_UTXO_COUNT;

    if !adequate {
        if balance_ut < min_req {
            warn!(
                "{} funding INSUFFICIENT: {} µT / {} µT required",
                name, balance_ut, min_req
            );
        }
        if utxo_count < MIN_UTXO_COUNT {
            warn!(
                "{} UTXO count INSUFFICIENT: {} / {} required",
                name, utxo_count, MIN_UTXO_COUNT
            );
        }
    } else {
        info!("{} funding OK: {} µT ({} UTXOs)", name, balance_ut, utxo_count);
    }

    WalletFundingStatus {
        name: name.to_string(),
        balance_ut,
        min_required_ut: min_req,
        utxo_count,
        is_adequate: adequate,
    }
}

/// Run full pre-flight check across all three wallet modes.
/// Returns `FundingPreflightResult` with per-wallet status.
/// Bails on the first underfunded wallet (fail-fast).
pub async fn run_preflight_checks(
    old_balance: u64,
    old_utxos: u32,
    new_balance: u64,
    new_utxos: u32,
    pp_balance: u64,
    pp_utxos: u32,
    min_required_ut: Option<u64>,
) -> Result<FundingPreflightResult> {
    info!("=== Funding Pre-Flight Checks ===");

    let wallets = vec![
        check_wallet_funding("old_wallet", old_balance, old_utxos, min_required_ut),
        check_wallet_funding("new_wallet", new_balance, new_utxos, min_required_ut),
        check_wallet_funding(
            "payment_processor",
            pp_balance,
            pp_utxos,
            min_required_ut,
        ),
    ];

    let all_adequate = wallets.iter().all(|w| w.is_adequate);

    if all_adequate {
        info!("=== All wallets adequately funded ===");
    } else {
        for w in &wallets {
            if !w.is_adequate {
                bail!(
                    "Pre-flight FAILED: {} has {} µT / {} µT required ({} UTXOs / {} UTXOs required)",
                    w.name, w.balance_ut, w.min_required_ut, w.utxo_count, MIN_UTXO_COUNT
                );
            }
        }
    }

    Ok(FundingPreflightResult { wallets, all_adequate })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_wallet_funding_adequate() {
        let result = check_wallet_funding("test", 200_000_000, 5, None);
        assert!(result.is_adequate);
        assert_eq!(result.balance_ut, 200_000_000);
    }

    #[test]
    fn test_check_wallet_funding_insufficient_balance() {
        let result = check_wallet_funding("test", 50, 5, None);
        assert!(!result.is_adequate);
    }

    #[test]
    fn test_check_wallet_funding_no_utxos() {
        let result = check_wallet_funding("test", 200_000_000, 0, None);
        assert!(!result.is_adequate);
    }

    #[test]
    fn test_check_wallet_funding_custom_minimum() {
        let result = check_wallet_funding("test", 5000, 1, Some(1000));
        assert!(result.is_adequate);
    }

    #[test]
    fn test_run_preflight_checks_all_ok() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(async {
            run_preflight_checks(200_000_000, 5, 200_000_000, 5, 200_000_000, 5, None).await
        });
        assert!(result.is_ok());
        assert!(result.unwrap().all_adequate);
    }

    #[test]
    fn test_run_preflight_checks_one_fails() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(async {
            run_preflight_checks(50, 0, 200_000_000, 5, 200_000_000, 5, None).await
        });
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Pre-flight FAILED"));
    }
}
