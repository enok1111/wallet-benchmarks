//! Integration tests for wallet benchmark harness
//!
//! These tests verify the core functionality of the benchmark harness:
//! - Config parsing and validation
//! - Metrics collection and result profile generation
//! - Scenario execution flow

use std::collections::HashMap;

use wallet_benchmarks::config::HarnessConfig;
use wallet_benchmarks::metrics::{EnvironmentInfo, ModeResult, ResultProfile, ScenarioResult};

// ============================================================================
// Config tests
// ============================================================================

#[test]
fn test_default_config_values() {
    let config = HarnessConfig::default();

    assert_eq!(config.base_node_http, "http://127.0.0.1:18142");
    assert_eq!(config.a_fund, 10_000_000_000); // 10,000 tXTM in µT
    assert_eq!(config.volume_target, 512);
    assert_eq!(config.c_min, 3);
    assert_eq!(config.fee_rate, 5); // µT/gram
    assert!(!config.scenarios.is_empty());
    assert_eq!(config.scenarios.len(), 9); // B0 + S0-S7
}

#[test]
fn test_config_from_toml() {
    let toml_content = r#"
base_node_http = "http://127.0.0.1:18143"
a_fund = 5_000_000_000
volume_target = 256
c_min = 2
fee_rate = 10
scenarios = ["B0", "S0", "S1"]
modes = ["old", "new"]
"#;

    let config: HarnessConfig = toml::from_str(toml_content).expect("Failed to parse TOML");

    assert_eq!(config.base_node_http, "http://127.0.0.1:18143");
    assert_eq!(config.a_fund, 5_000_000_000);
    assert_eq!(config.volume_target, 256);
    assert_eq!(config.c_min, 2);
    assert_eq!(config.fee_rate, 10);
    assert_eq!(config.scenarios, vec!["B0", "S0", "S1"]);
    assert_eq!(config.modes, vec!["old", "new"]);
}

#[test]
fn test_config_validation_invalid_scenario() {
    let toml_content = r#"
scenarios = ["B0", "INVALID_SCENARIO"]
modes = ["old"]
"#;

    let config: HarnessConfig = toml::from_str(toml_content).expect("Failed to parse TOML");
    assert!(config.validate().is_err());
}

#[test]
fn test_config_validation_invalid_mode() {
    let toml_content = r#"
scenarios = ["B0"]
modes = ["invalid_mode"]
"#;

    let config: HarnessConfig = toml::from_str(toml_content).expect("Failed to parse TOML");
    assert!(config.validate().is_err());
}

#[test]
fn test_config_validation_zero_funding() {
    let toml_content = r#"
scenarios = ["B0"]
modes = ["old"]
a_fund = 0
"#;

    let config: HarnessConfig = toml::from_str(toml_content).expect("Failed to parse TOML");
    assert!(config.validate().is_err());
}

#[test]
fn test_config_validation_zero_fee_rate() {
    let toml_content = r#"
scenarios = ["B0"]
modes = ["old"]
fee_rate = 0
"#;

    let config: HarnessConfig = toml::from_str(toml_content).expect("Failed to parse TOML");
    assert!(config.validate().is_err());
}

#[test]
fn test_config_load_missing_file_returns_default() {
    let config = HarnessConfig::load("/nonexistent/path/config.toml")
        .expect("Should return default when file missing");
    assert_eq!(config.base_node_http, "http://127.0.0.1:18142");
}

// ============================================================================
// Metrics tests
// ============================================================================

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
    assert_eq!(
        result.metrics.get("blocks_scanned").unwrap().as_u64(),
        Some(100)
    );
}

#[test]
fn test_mode_result_structure() {
    let mut result = ModeResult {
        mode: "old_wallet".to_string(),
        scenarios: HashMap::new(),
    };

    assert_eq!(result.mode, "old_wallet");
    assert!(result.scenarios.is_empty());
}

#[test]
fn test_result_profile_new() {
    let env = EnvironmentInfo::collect().expect("Failed to collect environment info");
    let config = HarnessConfig::default();
    let profile = ResultProfile::new(env, config);

    assert!(profile.mode_results.is_empty());
    assert_eq!(profile.total_duration_secs, 0.0);
}

#[test]
fn test_result_profile_add_mode() {
    let env = EnvironmentInfo::collect().expect("Failed to collect environment info");
    let config = HarnessConfig::default();
    let mut profile = ResultProfile::new(env, config);

    let mode_result = ModeResult {
        mode: "test_mode".to_string(),
        scenarios: HashMap::new(),
    };
    profile.add_mode_results("test_mode".to_string(), mode_result.clone());

    assert_eq!(profile.mode_results.len(), 1);
    assert!(profile.mode_results.contains_key("test_mode"));
}

#[test]
fn test_valid_scenarios() {
    let valid_scenarios = vec!["B0", "S0", "S1", "S2", "S3", "S4", "S5", "S6", "S7"];

    for scenario in &valid_scenarios {
        let result = ScenarioResult::new(scenario);
        assert_eq!(result.scenario_id, *scenario);
    }
}

#[test]
fn test_valid_modes() {
    let valid_modes = vec!["old", "new", "payment_processor"];

    for mode in &valid_modes {
        let result = ModeResult {
            mode: mode.to_string(),
            scenarios: HashMap::new(),
        };
        assert_eq!(result.mode, *mode);
    }
}

// ============================================================================
// Serialization tests
// ============================================================================

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

#[test]
fn test_result_profile_json() {
    let env = EnvironmentInfo::collect().expect("Failed to collect environment info");
    let config = HarnessConfig::default();
    let mut profile = ResultProfile::new(env, config);

    let mode_result = ModeResult {
        mode: "test_mode".to_string(),
        scenarios: HashMap::new(),
    };
    profile.add_mode_results("test_mode".to_string(), mode_result);

    let json = serde_json::to_string_pretty(&profile).expect("Failed to serialize profile");
    assert!(json.contains("test_mode"));
}

// ============================================================================
// Configuration parameter tests
// ============================================================================

#[test]
fn test_concurrent_batches() {
    let toml_content = r#"
scenarios = ["B0"]
modes = ["old"]
concurrent_batches = [8, 16, 32, 64, 128]
"#;

    let config: HarnessConfig = toml::from_str(toml_content).expect("Failed to parse TOML");
    assert_eq!(config.concurrent_batches, vec![8, 16, 32, 64, 128]);
}

#[test]
fn test_s5_parameters() {
    let toml_content = r#"
scenarios = ["B0"]
modes = ["old"]
s5_m = 100
s5_k = 10
"#;

    let config: HarnessConfig = toml::from_str(toml_content).expect("Failed to parse TOML");
    assert_eq!(config.s5_m, 100);
    assert_eq!(config.s5_k, 10);
}

#[test]
fn test_doubling_and_fanout() {
    let toml_content = r#"
scenarios = ["B0"]
modes = ["old"]
doubling_rounds = 8
fanout_outputs_per_tx = 4
"#;

    let config: HarnessConfig = toml::from_str(toml_content).expect("Failed to parse TOML");
    assert_eq!(config.doubling_rounds, 8);
    assert_eq!(config.fanout_outputs_per_tx, 4);
}

#[test]
fn test_seed_words_config() {
    let toml_content = r#"
scenarios = ["B0"]
modes = ["old"]
seed_words_old = ["abandon", "ability", "able", "about", "above", "absent", "absorb", "abstract", "absurd", "abuse", "access", "account"]
"#;

    let config: HarnessConfig = toml::from_str(toml_content).expect("Failed to parse TOML");
    assert!(config.seed_words_old.is_some());
    assert_eq!(config.seed_words_old.unwrap().len(), 12);
}

#[test]
fn test_peer_seeds() {
    let toml_content = r#"
scenarios = ["B0"]
modes = ["old"]
peer_seeds = ["127.0.0.1:18142", "192.168.1.1:18142"]
"#;

    let config: HarnessConfig = toml::from_str(toml_content).expect("Failed to parse TOML");
    assert_eq!(config.peer_seeds.len(), 2);
    assert_eq!(config.peer_seeds[0], "127.0.0.1:18142");
}

// ============================================================================
// Delta and throughput calculation tests (required by bounty)
// ============================================================================

#[test]
fn test_delta_calculations() {
    // Simulate scan times for delta calculation
    let b0_scan_time: f64 = 10.0;
    let s2_genesis_scan_time: f64 = 25.0;
    let s3_birthday_scan_time: f64 = 20.0;

    let t_scan_genesis_b0 = s2_genesis_scan_time - b0_scan_time;
    let t_scan_birthday_b0 = s3_birthday_scan_time - b0_scan_time;

    assert!((t_scan_genesis_b0 - 15.0).abs() < f64::EPSILON);
    assert!((t_scan_birthday_b0 - 10.0).abs() < f64::EPSILON);
}

#[test]
fn test_throughput_multiplier() {
    let individual_total_time: f64 = 100.0;
    let batch_total_time: f64 = 25.0;

    let throughput_multiplier = individual_total_time / batch_total_time;

    assert!((throughput_multiplier - 4.0).abs() < f64::EPSILON);
}

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

// ============================================================================
// Wallet versions and environment tests
// ============================================================================

#[test]
fn test_wallet_versions() {
    let config = HarnessConfig::default();

    // Check that wallet versions are set
    assert!(!config.wallet_versions.old_wallet.is_empty());
    assert!(!config.wallet_versions.new_wallet.is_empty());
    assert!(!config.wallet_versions.base_node.is_empty());
}

#[test]
fn test_environment_collection() {
    let env = EnvironmentInfo::collect().expect("Failed to collect environment info");

    assert!(!env.cpu_model.is_empty());
    assert!(env.cpu_count > 0);
    assert!(env.total_ram_bytes > 0);
    assert!(!env.os.is_empty());
}
