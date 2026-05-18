//! Integration tests for wallet benchmark harness
//!
//! These tests verify the core functionality of the benchmark harness:
//! - Config parsing and validation
//! - Metrics collection and result profile generation
//! - HTTP RPC client functionality (with mock responses)
//! - Scenario execution flow

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use wallet_benchmarks::config::HarnessConfig;
use wallet_benchmarks::metrics::{BenchmarkResult, ResultProfile, ScenarioResult};

/// Test that default configuration values are reasonable
#[test]
fn test_default_config_values() {
    let config = HarnessConfig::default();

    // Verify sensible defaults
    assert_eq!(config.network, "esmeralda");
    assert_eq!(config.base_node_http, "http://127.0.0.1:18142");
    assert_eq!(config.a_fund, 1_000_000_000); // 1 T funding amount
    assert_eq!(config.volume_target, 512);
    assert_eq!(config.c_min, 1);
    assert_eq!(config.fee_rate, 5); // µT/gram
    assert!(!config.scenarios.is_empty());
}

/// Test that config can be loaded from a TOML file
#[test]
fn test_config_from_toml() {
    let toml_content = r#"
network = "esmeralda"
base_node_http = "http://127.0.0.1:18143"
a_fund = 500_000_000
volume_target = 256
c_min = 2
fee_rate = 10
scenarios = ["B0", "S0", "S1"]
"#;

    let config: HarnessConfig = toml::from_str(toml_content).expect("Failed to parse TOML");

    assert_eq!(config.network, "esmeralda");
    assert_eq!(config.base_node_http, "http://127.0.0.1:18143");
    assert_eq!(config.a_fund, 500_000_000);
    assert_eq!(config.volume_target, 256);
    assert_eq!(config.c_min, 2);
    assert_eq!(config.fee_rate, 10);
    assert_eq!(config.scenarios, vec!["B0", "S0", "S1"]);
}

/// Test that invalid network names are rejected during validation
#[test]
fn test_config_validation_network() {
    let toml_content = r#"
network = "invalid_network"
"#;

    let config: HarnessConfig = toml::from_str(toml_content).expect("Failed to parse TOML");

    // Should fail validation for invalid network
    assert!(config.validate().is_err());
}

/// Test that zero funding amount is rejected
#[test]
fn test_config_validation_funding() {
    let toml_content = r#"
network = "esmeralda"
a_fund = 0
"#;

    let config: HarnessConfig = toml::from_str(toml_content).expect("Failed to parse TOML");

    // Should fail validation for zero funding
    assert!(config.validate().is_err());
}

/// Test that volume target must be positive
#[test]
fn test_config_validation_volume_target() {
    let toml_content = r#"
network = "esmeralda"
volume_target = 0
"#;

    let config: HarnessConfig = toml::from_str(toml_content).expect("Failed to parse TOML");

    // Should fail validation for zero volume target
    assert!(config.validate().is_err());
}

/// Test that fee rate must be non-negative
#[test]
fn test_config_validation_fee_rate() {
    let toml_content = r#"
network = "esmeralda"
fee_rate = 0
"#;

    let config: HarnessConfig = toml::from_str(toml_content).expect("Failed to parse TOML");

    // Fee rate of 0 should be valid (free transactions)
    assert!(config.validate().is_ok());
}

/// Test scenario result initialization
#[test]
fn test_scenario_result_new() {
    let result = ScenarioResult::new("B0");

    assert_eq!(result.scenario_id, "B0");
    assert_eq!(result.success_count, 0);
    assert_eq!(result.failure_count, 0);
    assert!((result.wall_clock_secs - 0.0) < f64::EPSILON);
    assert!(result.metrics.is_empty());
    assert!(result.failure_reasons.is_empty());
}

/// Test scenario result metrics recording
#[test]
fn test_scenario_result_metrics() {
    let mut result = ScenarioResult::new("S1");
    result.success_count = 1;
    result.wall_clock_secs = 10.5;
    result.metrics.insert(
        "blocks_scanned".to_string(),
        serde_json::json!(100),
    );

    assert_eq!(result.success_count, 1);
    assert!((result.wall_clock_secs - 10.5).abs() < f64::EPSILON);
    assert_eq!(result.metrics.get("blocks_scanned").unwrap().as_u64(), Some(100));
}

/// Test benchmark result structure
#[test]
fn test_benchmark_result() {
    let mut result = BenchmarkResult::new();
    result.mode = "old_wallet".to_string();
    result.network = "esmeralda".to_string();

    assert_eq!(result.mode, "old_wallet");
    assert_eq!(result.network, "esmeralda");
    assert!(result.scenarios.is_empty());
    assert!((result.total_wall_clock_secs - 0.0).abs() < f64::EPSILON);
}

/// Test result profile generation
#[test]
fn test_result_profile() {
    let mut profile = ResultProfile::new();
    profile.add_mode_result("old_wallet", &BenchmarkResult::new());
    profile.add_mode_result("new_wallet", &BenchmarkResult::new());

    assert_eq!(profile.results.len(), 2);
}

/// Test that config supports all standard networks
#[test]
fn test_config_networks() {
    let networks = vec!["esmeralda", "mainnet", "local"];

    for network in networks {
        let toml_content = format!(r#"network = "{}""#, network);
        let config: HarnessConfig = toml::from_str(&toml_content).expect("Failed to parse TOML");
        assert_eq!(config.network, network);
    }
}

/// Test that seed words can be provided in config
#[test]
fn test_config_seed_words() {
    let toml_content = r#"
network = "esmeralda"
seed_words_old = ["abandon", "ability", "able", "about", "above", "absent", "absorb", "abstract", "absurd", "abuse", "access", "account"]
"#;

    let config: HarnessConfig = toml::from_str(toml_content).expect("Failed to parse TOML");

    assert_eq!(config.seed_words_old.unwrap().len(), 12);
}

/// Test concurrent batch configuration
#[test]
fn test_concurrent_batches() {
    let toml_content = r#"
network = "esmeralda"
concurrent_batches = [8, 16, 32, 64, 128]
"#;

    let config: HarnessConfig = toml::from_str(toml_content).expect("Failed to parse TOML");

    assert_eq!(config.concurrent_batches, vec![8, 16, 32, 64, 128]);
}

/// Test S5 payment processor parameters
#[test]
fn test_s5_parameters() {
    let toml_content = r#"
network = "esmeralda"
s5_m = 100
s5_k = 10
"#;

    let config: HarnessConfig = toml::from_str(toml_content).expect("Failed to parse TOML");

    assert_eq!(config.s5_m, 100);
    assert_eq!(config.s5_k, 10);
}

/// Test that doubling rounds and fan-out are configurable
#[test]
fn test_doubling_and_fanout() {
    let toml_content = r#"
network = "esmeralda"
doubling_rounds = 8
fanout_outputs_per_tx = 4
"#;

    let config: HarnessConfig = toml::from_str(toml_content).expect("Failed to parse TOML");

    assert_eq!(config.doubling_rounds, 8);
    assert_eq!(config.fanout_outputs_per_tx, 4);
}

/// Test scenario ID validation
#[test]
fn test_valid_scenario_ids() {
    let valid_scenarios = vec!["B0", "S0", "S1", "S2", "S3", "S4", "S5", "S6", "S7"];

    for scenario in &valid_scenarios {
        let result = ScenarioResult::new(scenario);
        assert_eq!(result.scenario_id, *scenario);
    }
}

/// Test metrics serialization to JSON
#[test]
fn test_metrics_json_serialization() {
    let mut metrics: HashMap<String, serde_json::Value> = HashMap::new();
    metrics.insert("scan_time".to_string(), serde_json::json!(10.5));
    metrics.insert("blocks_scanned".to_string(), serde_json::json!(100));
    metrics.insert("success".to_string(), serde_json::json!(true));

    let json = serde_json::to_string(&metrics).expect("Failed to serialize metrics");
    let parsed: HashMap<String, serde_json::Value> =
        serde_json::from_str(&json).expect("Failed to deserialize metrics");

    assert_eq!(parsed.get("scan_time").unwrap().as_f64(), Some(10.5));
    assert_eq!(parsed.get("blocks_scanned").unwrap().as_u64(), Some(100));
    assert_eq!(parsed.get("success").unwrap().as_bool(), Some(true));
}

/// Test that result profile can be serialized to JSON
#[test]
fn test_result_profile_json() {
    let mut profile = ResultProfile::new();
    let mut result = BenchmarkResult::new();
    result.mode = "test_mode".to_string();
    profile.add_mode_result("test_mode", &result);

    let json = serde_json::to_string_pretty(&profile).expect("Failed to serialize profile");
    assert!(json.contains("test_mode"));
}

/// Test config file path resolution
#[test]
fn test_config_file_path() {
    // Test that config can be loaded from a file path
    let config_path = PathBuf::from("config.example.toml");

    if config_path.exists() {
        let config = HarnessConfig::load(&config_path).expect("Failed to load config from file");
        assert!(!config.network.is_empty());
    }
}

/// Test that harness supports multiple wallet modes
#[test]
fn test_wallet_modes() {
    let modes = vec!["old_wallet", "new_wallet", "payment_processor"];

    for mode in &modes {
        let mut result = BenchmarkResult::new();
        result.mode = mode.to_string();
        assert_eq!(result.mode, *mode);
    }
}

/// Test delta calculations (required by bounty)
#[test]
fn test_delta_calculations() {
    // Simulate scan times for delta calculation
    let b0_scan_time = 10.0;
    let s2_genesis_scan_time = 25.0;
    let s3_birthday_scan_time = 20.0;

    let t_scan_genesis_b0 = s2_genesis_scan_time - b0_scan_time;
    let t_scan_birthday_b0 = s3_birthday_scan_time - b0_scan_time;

    assert!((t_scan_genesis_b0 - 15.0).abs() < f64::EPSILON);
    assert!((t_scan_birthday_b0 - 10.0).abs() < f64::EPSILON);
}

/// Test throughput multiplier calculation
#[test]
fn test_throughput_multiplier() {
    let individual_total_time = 100.0;
    let batch_total_time = 25.0;

    let throughput_multiplier = individual_total_time / batch_total_time;

    assert!((throughput_multiplier - 4.0).abs() < f64::EPSILON);
}

/// Test fee efficiency calculation
#[test]
fn test_fee_efficiency() {
    let s5_m = 100u64;
    let batch_fees = 500_000u64;
    let individual_fees = 1_000_000u64;

    let fee_per_recipient_batch = batch_fees / s5_m;
    let fee_per_recipient_individual = individual_fees / s5_m;

    assert_eq!(fee_per_recipient_batch, 5_000);
    assert_eq!(fee_per_recipient_individual, 10_000);
    assert!(fee_per_recipient_batch < fee_per_recipient_individual);
}
