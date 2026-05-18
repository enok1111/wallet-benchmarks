//! Metrics collection and result profile generation
//!
//! Captures cross-cutting metrics required by the bounty:
//! - Hardware/environment disclosure
//! - Pinned wallet versions
//! - Per-scenario, per-mode metrics
//! - Computed deltas (T_scan differences, throughput multipliers)

use anyhow::Result;
use chrono::serde::ts_seconds_option;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use sysinfo::System;

use crate::config::HarnessConfig;

/// Hardware and environment disclosure for reproducibility
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentInfo {
    /// CPU model name
    pub cpu_model: String,
    /// Number of logical CPU cores
    pub cpu_count: usize,
    /// Total RAM in bytes
    pub total_ram_bytes: u64,
    /// Disk type (SSD/HDD/unknown)
    pub disk_type: String,
    /// Operating system
    pub os: String,
    /// OS version
    pub os_version: String,
    /// Network path to base node (local vs remote)
    pub base_node_path: String,
    /// Timestamp of run start
    #[serde(with = "ts_seconds_option")]
    pub run_start: Option<chrono::DateTime<chrono::Utc>>,
}

impl EnvironmentInfo {
    /// Collect current environment information
    pub fn collect() -> Result<Self> {
        let mut sys = System::new_all();
        sys.refresh_cpu_specifics(sysinfo::CpuRefreshKind::everything());

        let cpu_model = sys
            .cpus()
            .first()
            .map(|c| c.brand().to_string())
            .unwrap_or_else(|| "Unknown".to_string());

        let cpu_count = sys.cpus().len();
        let total_ram_bytes = sys.total_memory();

        let os = std::env::consts::OS.to_string();
        let os_version = get_os_version().unwrap_or_else(|| "unknown".to_string());

        Ok(Self {
            cpu_model,
            cpu_count,
            total_ram_bytes,
            disk_type: "unknown".to_string(),
            os,
            os_version,
            base_node_path: "local".to_string(), // Default; can be overridden
            run_start: Some(chrono::Utc::now()),
        })
    }
}

/// Detect disk type using heuristics
fn detect_disk_type() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        // Check if / is on NVMe or SSD
        if std::path::Path::new("/sys/block/nvme0n1").exists() {
            return Some("nvme-ssd".to_string());
        }
        // Check rotational attribute for traditional disks
        if let Ok(content) = std::fs::read_to_string("/sys/block/sda/queue/rotational") {
            if content.trim() == "0" {
                return Some("ssd".to_string());
            }
            if content.trim() == "1" {
                return Some("hdd".to_string());
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        // macOS typically uses SSDs; check SMART data if available
        return Some("ssd".to_string());
    }

    None
}

/// Get OS version string
fn get_os_version() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        if let Ok(content) = std::fs::read_to_string("/etc/os-release") {
            for line in content.lines() {
                if let Some(version) = line.strip_prefix("PRETTY_NAME=") {
                    return Some(version.trim_matches('"').to_string());
                }
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        // Could parse sw_vers output; simplified for now
        return Some("macOS".to_string());
    }

    None
}

/// Comprehensive result profile containing all benchmark data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultProfile {
    /// Environment disclosure
    pub environment: EnvironmentInfo,
    /// Configuration parameters used for the run
    pub config: HarnessConfig,
    /// Per-mode results keyed by mode name
    pub mode_results: HashMap<String, ModeResult>,
    /// Computed deltas and derived metrics
    pub computed_deltas: HashMap<String, serde_json::Value>,
    /// Run metadata
    #[serde(with = "ts_seconds_option")]
    pub run_end: Option<chrono::DateTime<chrono::Utc>>,
    /// Total wall-clock duration in seconds
    pub total_duration_secs: f64,
}

impl ResultProfile {
    pub fn new(environment: EnvironmentInfo, config: HarnessConfig) -> Self {
        Self {
            environment,
            config,
            mode_results: HashMap::new(),
            computed_deltas: HashMap::new(),
            run_end: None,
            total_duration_secs: 0.0,
        }
    }

    /// Add results for a wallet mode
    pub fn add_mode_results(&mut self, mode: String, results: ModeResult) {
        self.mode_results.insert(mode, results);
    }

    /// Save result profile to JSON file
    pub fn save(&self, path: &str) -> Result<()> {
        // Ensure output directory exists
        if let Some(parent) = Path::new(path).parent() {
            fs::create_dir_all(parent)?;
        }

        let json = serde_json::to_string_pretty(self)?;
        fs::write(path, json)?;
        Ok(())
    }
}

/// Results for a single wallet mode across all scenarios
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModeResult {
    /// Wallet mode identifier
    pub mode: String,
    /// Per-scenario results
    pub scenarios: HashMap<String, ScenarioResult>,
}

/// Result from running a single scenario
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioResult {
    /// Scenario ID (B0, S0, S1, etc.)
    pub scenario_id: String,
    /// Wall-clock duration in seconds
    pub wall_clock_secs: f64,
    /// Fees paid in µT
    pub fees_paid_ut: u64,
    /// Success count
    pub success_count: u32,
    /// Failure count
    pub failure_count: u32,
    /// Failure reasons (if any)
    pub failure_reasons: Vec<String>,
    /// Final balance delta vs expected (µT)
    pub balance_delta_ut: i64,
    /// Chain tip height at scenario start
    pub tip_height_start: u64,
    /// Chain tip height at scenario end
    pub tip_height_end: u64,
    /// Scenario-specific metrics
    pub metrics: HashMap<String, serde_json::Value>,
}

impl ScenarioResult {
    pub fn new(scenario_id: &str) -> Self {
        Self {
            scenario_id: scenario_id.to_string(),
            wall_clock_secs: 0.0,
            fees_paid_ut: 0,
            success_count: 0,
            failure_count: 0,
            failure_reasons: Vec::new(),
            balance_delta_ut: 0,
            tip_height_start: 0,
            tip_height_end: 0,
            metrics: HashMap::new(),
        }
    }
}

/// Per-transaction metrics for detailed analysis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionMetrics {
    /// Transaction ID or index
    pub tx_id: String,
    /// Construction time in milliseconds
    pub construction_time_ms: f64,
    /// Time from broadcast to mempool acceptance (ms)
    pub broadcast_to_mempool_ms: f64,
    /// Time from broadcast to confirmation at depth C_min (ms)
    pub broadcast_to_confirmed_ms: f64,
    /// Fee paid in µT
    pub fee_paid_ut: u64,
    /// Outcome (success, rejected, stalled, timeout)
    pub outcome: String,
    /// Error string if failed
    pub error: Option<String>,
}

/// Scan-specific metrics for B0, S2, S3, S6, S7 scenarios
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanMetrics {
    /// Total scan time in seconds
    pub t_scan_secs: f64,
    /// Blocks scanned per second
    pub blocks_per_sec: f64,
    /// Tip height at scan start
    pub h_tip_start: u64,
    /// Tip height at scan end
    pub h_tip_end: u64,
    /// Peak RSS memory usage in bytes
    pub peak_rss_bytes: u64,
    /// Peak CPU utilization percentage
    pub peak_cpu_pct: f64,
    /// Number of outputs found during scan
    pub outputs_found: u32,
}

/// Concurrent construction metrics for S4 scenario
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConcurrencyMetrics {
    /// Number of concurrent transactions attempted
    pub n_concurrent: usize,
    /// Batch wall-clock time in seconds
    pub batch_wall_clock_secs: f64,
    /// Success rate (0.0 to 1.0)
    pub success_rate: f64,
    /// Max observed serialization gap between consecutive construction events (ms)
    pub max_serialization_gap_ms: f64,
    /// Double-selection rejection count
    pub double_selection_rejections: u32,
    /// Per-transaction metrics
    pub tx_metrics: Vec<TransactionMetrics>,
}

/// Payment processor throughput metrics for S5 scenario
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThroughputMetrics {
    /// Batch arm total time in seconds
    pub t_batch_secs: f64,
    /// Individual arm total time in seconds
    pub t_individual_secs: f64,
    /// Throughput multiplier (T_individual / T_batch)
    pub throughput_multiplier: f64,
    /// Total fees for batch arm (µT)
    pub batch_fees_ut: u64,
    /// Total fees for individual arm (µT)
    pub individual_fees_ut: u64,
    /// Fee per recipient for batch arm (µT)
    pub batch_fee_per_recipient: f64,
    /// Fee per recipient for individual arm (µT)
    pub individual_fee_per_recipient: f64,
    /// Blocks consumed by batch arm
    pub batch_blocks_consumed: u32,
    /// Blocks consumed by individual arm
    pub individual_blocks_consumed: u32,
}
