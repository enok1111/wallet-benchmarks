//! Wallet Performance Benchmark Harness
//!
//! Measures `minotari-cli` wallet performance across three modes:
//! - Old Wallet (`minotari_console_wallet` via gRPC)
//! - New Wallet (`minotari-cli` library with offline signing)
//! - Payment Processor (batch 1-to-many transactions)
//!
//! All nine scenarios (B0, S0-S7) run per wallet mode on Esmeralda testnet.

mod config;
mod grpc_client;
mod http_rpc;
mod metrics;
mod modes;
mod scenarios;

use anyhow::Result;
use clap::Parser;
use log::{info, warn};

/// Wallet Performance Benchmark Harness
#[derive(Parser, Debug)]
#[command(name = "wallet-benchmarks", about, version)]
struct Cli {
    /// Path to configuration file
    #[arg(short, long, default_value = "config.toml")]
    config: String,

    /// Run only specific scenarios (e.g., "B0,S0,S1")
    #[arg(long)]
    scenarios: Option<String>,

    /// Run only specific wallet modes (e.g., "old,new,payment_processor")
    #[arg(long)]
    modes: Option<String>,

    /// Output directory for results
    #[arg(long, default_value = "results")]
    output_dir: String,

    /// Skip baseline result profile generation
    #[arg(long)]
    skip_baseline: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    init_logging()?;

    let cli = Cli::parse();

    // Load configuration
    let mut cfg = config::HarnessConfig::load(&cli.config)?;
    info!("Loaded configuration: {:?}", cfg);

    // Override scenarios/modes from CLI if provided
    if let Some(scenarios_str) = &cli.scenarios {
        cfg.scenarios = scenarios_str
            .split(',')
            .map(|s| s.trim().to_string())
            .collect();
    }
    if let Some(modes_str) = &cli.modes {
        cfg.modes = modes_str
            .split(',')
            .map(|m| m.trim().to_string())
            .collect();
    }

    // Validate configuration
    cfg.validate()?;

    // Collect environment metadata
    let env_info = metrics::EnvironmentInfo::collect()?;
    info!("Environment: {:?}", env_info);

    // Run benchmarks
    let mut result_profile = metrics::ResultProfile::new(env_info, cfg.clone());

    for mode in &cfg.modes {
        info!("=== Running mode: {} ===", mode);
        let mode_results = match mode.as_str() {
            "old" => scenarios::run_old_wallet_mode(&cfg).await?,
            "new" => scenarios::run_new_wallet_mode(&cfg).await?,
            "payment_processor" => scenarios::run_payment_processor_mode(&cfg).await?,
            _ => {
                warn!("Unknown mode: {}, skipping", mode);
                continue;
            }
        };
        result_profile.add_mode_results(mode.clone(), mode_results);
    }

    // Compute derived metrics and deltas
    result_profile.compute_deltas();

    // Record run end time and total duration
    let run_end = chrono::Utc::now();
    result_profile.run_end = Some(run_end);
    if let Some(run_start) = result_profile.environment.run_start {
        result_profile.total_duration_secs = (run_end - run_start).num_milliseconds() as f64 / 1000.0;
    }

    // Save results
    let output_path = format!("{}/result_profile.json", cli.output_dir);
    result_profile.save(&output_path)?;
    info!("Results saved to {}", output_path);

    Ok(())
}

fn init_logging() -> Result<()> {
    log4rs::init_file("log4rs.yml", Default::default()).ok();
    // Fallback to simple logger if config not found
    let _ = env_logger::builder().is_test(false).try_init();
    Ok(())
}
