//! B0 - Baseline Scan (empty wallet)
//!
//! Floor cost of block-walk + view-key check with nothing to store.
//! This is the baseline for all scan scenarios (S2, S3, S6, S7).

use anyhow::{Context, Result};
use log::{debug, info};
use std::time::Instant;

use crate::config::HarnessConfig;
use crate::http_rpc::BaseNodeRpcClient;
use crate::metrics::{ScenarioResult, ScanMetrics};

/// Run B0 baseline scan for old wallet mode (subprocess + gRPC)
pub async fn run_b0_old_wallet(config: &HarnessConfig) -> Result<ScenarioResult> {
    info!("Running B0 baseline scan for old wallet mode");

    let mut result = ScenarioResult::new("B0");
    let start_time = Instant::now();

    // Record chain tip at start
    let base_node_client = BaseNodeRpcClient::new(&config.base_node_http);
    let tip_height_start = base_node_client
        .get_tip_height()
        .await
        .context("Failed to get initial tip height")?;

    result.tip_height_start = tip_height_start;
    info!("Chain tip at start: {}", tip_height_start);

    // Spawn empty old wallet with unique data directory
    let data_dir = tempfile::TempDir::new()
        .context("Failed to create temp data directory")?;

    let grpc_port = config.old_wallet_grpc_base_port;
    let mut process = spawn_old_wallet(
        &config.old_wallet_binary,
        data_dir.path(),
        grpc_port,
        "esmeralda",
    )
    .context("Failed to spawn old wallet")?;

    // Wait for gRPC server to be ready
    wait_for_grpc_ready(grpc_port, 60).await.context("Timeout waiting for gRPC server")?;

    // Trigger full scan from genesis
    trigger_full_scan(grpc_port).await.context("Failed to trigger scan")?;

    // Wait for scan to complete (wallet catches up to chain tip)
    wait_for_scan_complete(grpc_port, tip_height_start, 300)
        .await
        .context("Timeout waiting for scan completion")?;

    let scan_time_secs = start_time.elapsed().as_secs_f64();

    // Get final state
    let tip_height_end = get_wallet_tip_height(grpc_port).await.unwrap_or(tip_height_start);
    result.tip_height_end = tip_height_end;

    // Calculate blocks scanned
    let blocks_scanned = tip_height_end.saturating_sub(tip_height_start);

    // Record metrics
    result.wall_clock_secs = scan_time_secs;
    result.success_count = if blocks_scanned > 0 { 1 } else { 0 };

    // Add scan-specific metrics
    let scan_metrics = ScanMetrics {
        t_scan_secs: scan_time_secs,
        blocks_per_sec: if scan_time_secs > 0.0 {
            blocks_scanned as f64 / scan_time_secs
        } else {
            0.0
        },
        h_tip_start: tip_height_start,
        h_tip_end: tip_height_end,
        peak_rss_bytes: get_process_peak_rss(&mut process),
        peak_cpu_pct: 0.0, // TODO: track CPU usage
        outputs_found: 0, // Empty wallet should find nothing
    };

    result.metrics.insert(
        "scan_metrics".to_string(),
        serde_json::to_value(&scan_metrics).unwrap_or_default(),
    );

    // Cleanup
    kill_process(&mut process);

    info!(
        "B0 baseline scan completed: {} blocks in {:.2}s ({:.2} blocks/sec)",
        blocks_scanned, scan_time_secs, scan_metrics.blocks_per_sec
    );

    Ok(result)
}

/// Run B0 baseline scan for new wallet mode (library integration)
pub async fn run_b0_new_wallet(config: &HarnessConfig) -> Result<ScenarioResult> {
    info!("Running B0 baseline scan for new wallet mode");

    let mut result = ScenarioResult::new("B0");
    let start_time = Instant::now();

    // Record chain tip at start
    let base_node_client = BaseNodeRpcClient::new(&config.base_node_http);
    let tip_height_start = base_node_client
        .get_tip_height()
        .await
        .context("Failed to get initial tip height")?;

    result.tip_height_start = tip_height_start;
    info!("Chain tip at start: {}", tip_height_start);

    // Create empty wallet database
    let data_dir = tempfile::TempDir::new()
        .context("Failed to create temp data directory")?;
    let db_path = data_dir.path().join("wallet.db");

    // Initialize empty wallet with view key (no spend key)
    // This simulates a watch-only wallet for baseline scan
    init_empty_wallet(&db_path, config.wallet_password.as_deref().unwrap_or("benchmark"))
        .await
        .context("Failed to initialize empty wallet")?;

    // Run scanner from genesis
    let scan_result = run_scanner(
        &db_path,
        &config.base_node_http,
        config.wallet_password.as_deref().unwrap_or("benchmark"),
        100, // batch size
        None, // max blocks (scan to tip)
    )
    .await
    .context("Scanner failed")?;

    let scan_time_secs = start_time.elapsed().as_secs_f64();

    // Get final state
    let tip_height_end = base_node_client
        .get_tip_height()
        .await
        .unwrap_or(tip_height_start);
    result.tip_height_end = tip_height_end;

    // Calculate blocks scanned
    let blocks_scanned = tip_height_end.saturating_sub(tip_height_start);

    // Record metrics
    result.wall_clock_secs = scan_time_secs;
    result.success_count = if blocks_scanned > 0 { 1 } else { 0 };

    // Add scan-specific metrics for new wallet
    let scan_metrics = ScanMetrics {
        t_scan_secs: scan_time_secs,
        blocks_per_sec: if scan_time_secs > 0.0 {
            blocks_scanned as f64 / scan_time_secs
        } else {
            0.0
        },
        h_tip_start: tip_height_start,
        h_tip_end: tip_height_end,
        peak_rss_bytes: 0, // TODO: track process memory
        peak_cpu_pct: 0.0, // TODO: track CPU usage
        outputs_found: scan_result.outputs_found,
    };

    result.metrics.insert(
        "scan_metrics".to_string(),
        serde_json::to_value(&scan_metrics).unwrap_or_default(),
    );

    info!(
        "B0 baseline scan (new wallet) completed: {} blocks in {:.2}s ({:.2} blocks/sec)",
        blocks_scanned, scan_time_secs, scan_metrics.blocks_per_sec
    );

    Ok(result)
}

/// Run B0 baseline scan for payment processor mode (same as new wallet)
pub async fn run_b0_payment_processor(config: &HarnessConfig) -> Result<ScenarioResult> {
    // Payment processor uses the same scanning mechanism as new wallet
    run_b0_new_wallet(config).await
}

// Helper functions for old wallet subprocess management

fn spawn_old_wallet(
    binary: &str,
    data_dir: &std::path::Path,
    grpc_port: u16,
    network: &str,
) -> Result<std::process::Child> {
    let mut cmd = std::process::Command::new(binary);
    cmd.arg("--base-path")
        .arg(data_dir)
        .arg("--network")
        .arg(network)
        .arg("--grpc-enabled")
        .arg("--grpc-address")
        .arg(format!("127.0.0.1:{}", grpc_port))
        .arg("-n") // Non-interactive mode
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    info!(
        "Spawning old wallet: {} --base-path {} --grpc-address 127.0.0.1:{} -n",
        binary,
        data_dir.display(),
        grpc_port
    );

    let child = cmd.spawn().context("Failed to spawn wallet process")?;
    Ok(child)
}

async fn wait_for_grpc_ready(grpc_port: u16, timeout_secs: u64) -> Result<()> {
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(timeout_secs);

    loop {
        if start.elapsed() > timeout {
            anyhow::bail!("Timeout waiting for gRPC server on port {}", grpc_port);
        }

        // Try to connect to the gRPC port
        match tokio::net::TcpStream::connect(format!("127.0.0.1:{}", grpc_port)).await {
            Ok(_) => {
                debug!("gRPC server ready on port {}", grpc_port);
                return Ok(());
            }
            Err(e) => {
                debug!("gRPC not ready yet: {}", e);
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        }
    }
}

async fn trigger_full_scan(grpc_port: u16) -> Result<()> {
    // TODO: Implement gRPC call to trigger full scan
    // For now, the wallet will scan automatically on startup
    debug!("Triggering full scan via gRPC on port {}", grpc_port);
    Ok(())
}

async fn wait_for_scan_complete(
    grpc_port: u16,
    target_height: u64,
    timeout_secs: u64,
) -> Result<()> {
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(timeout_secs);

    loop {
        if start.elapsed() > timeout {
            anyhow::bail!("Timeout waiting for scan completion");
        }

        // TODO: Check wallet state via gRPC to see if scan is complete
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
}

async fn get_wallet_tip_height(grpc_port: u16) -> Result<u64> {
    // TODO: Get wallet tip height via gRPC
    Ok(0)
}

fn get_process_peak_rss(_process: &mut std::process::Child) -> u64 {
    // TODO: Track process memory usage
    0
}

fn kill_process(process: &mut std::process::Child) {
    if let Err(e) = process.kill() {
        debug!("Failed to kill process: {}", e);
    }
    let _ = process.wait();
}

// Helper functions for new wallet library integration

async fn init_empty_wallet(_db_path: &std::path::Path, _password: &str) -> Result<()> {
    // TODO: Initialize empty wallet database using minotari crate
    // This would use minotari::utils::init_wallet::init_with_view_key
    debug!("Initializing empty wallet at {:?}", _db_path);
    Ok(())
}

struct ScanResult {
    outputs_found: u32,
}

async fn run_scanner(
    _db_path: &std::path::Path,
    _base_url: &str,
    _password: &str,
    _batch_size: u32,
    _max_blocks: Option<u32>,
) -> Result<ScanResult> {
    // TODO: Run scanner using minotari crate
    // This would use minotari::Scanner::new(...).mode(ScanMode::Full).run()
    debug!(
        "Running scanner from {:?} against {}",
        _db_path, _base_url
    );
    Ok(ScanResult { outputs_found: 0 })
}
