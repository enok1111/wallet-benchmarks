//! Scenario execution orchestrator
//!
//! Runs all nine scenarios (B0, S0-S7) for each wallet mode.
//! Each scenario is implemented per-mode to capture mode-specific behavior.

pub mod b0_baseline;

use anyhow::Result;
use log::info;
use std::collections::HashMap;

use crate::config::HarnessConfig;
use crate::metrics::{ModeResult, ScenarioResult};

/// Run all scenarios for old wallet mode
pub async fn run_old_wallet_mode(config: &HarnessConfig) -> Result<ModeResult> {
    use crate::modes::old_wallet::OldWalletMode;
    use crate::modes::WalletMode;

    // Keep TempDir alive to prevent directory leaks
    let temp_dir = tempfile::TempDir::new()?;
    let mut mode = OldWalletMode::new(temp_dir.path().to_path_buf(), config.old_wallet_grpc_base_port);

    // Initialize wallet
    mode.initialize(config).await?;

    let mut scenarios = HashMap::new();

    // Run each scenario in order
    for scenario_id in &config.scenarios {
        info!("Running scenario {} for old wallet", scenario_id);
        let result = mode.run_scenario(scenario_id, config).await?;
        scenarios.insert(scenario_id.clone(), result);
    }

    // Teardown (process killed before temp_dir dropped)
    mode.teardown().await?;

    // temp_dir automatically cleaned up when dropped
    Ok(ModeResult {
        mode: "old".to_string(),
        scenarios,
    })
}

/// Run all scenarios for new wallet mode
pub async fn run_new_wallet_mode(config: &HarnessConfig) -> Result<ModeResult> {
    use crate::modes::new_wallet::NewWalletMode;
    use crate::modes::WalletMode;

    // Keep TempDir alive to prevent directory leaks
    let temp_dir = tempfile::TempDir::new()?;
    let mut mode = NewWalletMode::new(temp_dir.path().to_path_buf());

    // Initialize wallet
    mode.initialize(config).await?;

    let mut scenarios = HashMap::new();

    // Run each scenario in order
    for scenario_id in &config.scenarios {
        info!("Running scenario {} for new wallet", scenario_id);
        let result = mode.run_scenario(scenario_id, config).await?;
        scenarios.insert(scenario_id.clone(), result);
    }

    // Teardown
    mode.teardown().await?;

    // temp_dir automatically cleaned up when dropped
    Ok(ModeResult {
        mode: "new".to_string(),
        scenarios,
    })
}

/// Run all scenarios for payment processor mode
pub async fn run_payment_processor_mode(config: &HarnessConfig) -> Result<ModeResult> {
    use crate::modes::payment_processor::PaymentProcessorMode;
    use crate::modes::WalletMode;

    // Keep TempDir alive to prevent directory leaks
    let temp_dir = tempfile::TempDir::new()?;
    let mut mode = PaymentProcessorMode::new(temp_dir.path().to_path_buf());

    // Initialize wallet
    mode.initialize(config).await?;

    let mut scenarios = HashMap::new();

    // Run each scenario in order
    for scenario_id in &config.scenarios {
        info!(
            "Running scenario {} for payment processor",
            scenario_id
        );
        let result = mode.run_scenario(scenario_id, config).await?;
        scenarios.insert(scenario_id.clone(), result);
    }

    // Teardown
    mode.teardown().await?;

    // temp_dir automatically cleaned up when dropped
    Ok(ModeResult {
        mode: "payment_processor".to_string(),
        scenarios,
    })
}
