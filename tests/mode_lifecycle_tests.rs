//! Mode lifecycle tests for wallet-benchmarks
//!
//! Tests wallet mode lifecycle operations: configuration, seed word generation,
//! scenario metrics, and profile management.
//! Does NOT require a live base node or wallet.

use wallet_benchmarks::config::HarnessConfig;
use wallet_benchmarks::metrics::{EnvironmentInfo, ModeResult, ResultProfile, ScenarioResult};
use wallet_benchmarks::modes::WalletModeId;

// ============================================================================
// WalletModeId tests
// ============================================================================

#[test]
fn test_wallet_mode_id_variants() {
    assert_eq!(format!("{:?}", WalletModeId::Old), "Old");
    assert_eq!(format!("{:?}", WalletModeId::New), "New");
    assert_eq!(format!("{:?}", WalletModeId::PaymentProcessor), "PaymentProcessor");
}

#[test]
fn test_wallet_mode_id_suffix_word_unique() {
    let mut suffixes = std::collections::HashSet::new();
    suffixes.insert(WalletModeId::Old.suffix_word());
    suffixes.insert(WalletModeId::New.suffix_word());
    suffixes.insert(WalletModeId::PaymentProcessor.suffix_word());
    assert_eq!(suffixes.len(), 3, "All three mode suffix words must be unique");
}

#[test]
fn test_wallet_mode_id_suffix_not_empty() {
    for mode in [WalletModeId::Old, WalletModeId::New, WalletModeId::PaymentProcessor] {
        assert!(!mode.suffix_word().is_empty(), "Suffix for {:?} must not be empty", mode);
    }
}

// ============================================================================
// Seed words tests
// ============================================================================

#[test]
fn test_generate_tari_seed_words_default_length() {
    let words = wallet_benchmarks::modes::generate_tari_seed_words();
    assert_eq!(
        words.len(),
        24,
        "CipherSeed generates 24 words, got {}",
        words.len()
    );
}

#[test]
fn test_generate_tari_seed_words_unique_per_call() {
    let words1 = wallet_benchmarks::modes::generate_tari_seed_words();
    let words2 = wallet_benchmarks::modes::generate_tari_seed_words();
    assert_ne!(words1, words2, "Two consecutive calls must produce different seeds");
}

#[test]
fn test_generate_tari_seed_words_all_lowercase_ascii() {
    for _ in 0..5 {
        let words = wallet_benchmarks::modes::generate_tari_seed_words();
        for (i, word) in words.iter().enumerate() {
            assert!(
                word.chars().all(|c| c.is_ascii_lowercase()),
                "Word {} ('{}') contains non-lowercase characters",
                i, word
            );
        }
    }
}

#[test]
fn test_generate_tari_seed_words_no_empty_words() {
    for _ in 0..5 {
        let words = wallet_benchmarks::modes::generate_tari_seed_words();
        assert!(!words.is_empty(), "Seed words must not be empty");
        for (i, word) in words.iter().enumerate() {
            assert!(!word.is_empty(), "Word at index {} empty", i);
        }
    }
}

// ============================================================================
// HarnessConfig tests
// ============================================================================

#[test]
fn test_default_config_values() {
    let config = HarnessConfig::default();
    assert!(config.fee_rate > 0, "Default fee_rate must be > 0");
    assert!(config.c_min > 0, "Default c_min must be > 0");
    assert!(config.a_fund > 0, "Default a_fund must be > 0");
    assert!(config.doubling_rounds > 0, "Default doubling_rounds must be > 0");
    assert!(config.volume_target > 0, "Default volume_target must be > 0");
}

#[test]
fn test_config_seed_words_default_to_none() {
    let config = HarnessConfig::default();
    assert!(config.seed_words_old.is_none());
    assert!(config.seed_words_new.is_none());
    assert!(config.seed_words_payment.is_none());
}

#[test]
fn test_config_seed_words_custom_values() {
    let mut config = HarnessConfig::default();
    let words = vec!["test".to_string(); 24];
    config.seed_words_old = Some(words.clone());
    config.seed_words_new = Some(words.clone());
    config.seed_words_payment = Some(words);
    assert_eq!(config.seed_words_old.as_ref().unwrap().len(), 24);
    assert_eq!(config.seed_words_new.as_ref().unwrap().len(), 24);
    assert_eq!(config.seed_words_payment.as_ref().unwrap().len(), 24);
}

// ============================================================================
// ScenarioResult tests
// ============================================================================

#[test]
fn test_scenario_result_new() {
    let result = ScenarioResult::new("S1");
    assert_eq!(result.scenario_id, "S1");
    assert_eq!(result.wall_clock_secs, 0.0);
    assert_eq!(result.success_count, 0);
    assert_eq!(result.failure_count, 0);
    assert_eq!(result.fees_paid_ut, 0);
}

#[test]
fn test_scenario_result_custom_values() {
    let mut result = ScenarioResult::new("S2");
    result.success_count = 10;
    result.failure_count = 2;
    result.wall_clock_secs = 100.0;
    result.fees_paid_ut = 5000;
    assert_eq!(result.success_count, 10);
    assert_eq!(result.fees_paid_ut, 5000);
}

#[test]
fn test_scenario_result_json_roundtrip() {
    let mut result = ScenarioResult::new("S1");
    result.success_count = 5;
    result.fees_paid_ut = 100;
    let json = serde_json::to_string(&result).expect("Serialize ScenarioResult");
    let deserialized: ScenarioResult = serde_json::from_str(&json).expect("Deserialize ScenarioResult");
    assert_eq!(deserialized.scenario_id, "S1");
    assert_eq!(deserialized.success_count, 5);
}

// ============================================================================
// ModeResult tests
// ============================================================================

#[test]
fn test_mode_result_new() {
    let mut result = ModeResult {
        mode: "old_wallet".to_string(),
        scenarios: std::collections::HashMap::new(),
    };
    result.scenarios.insert("B0".to_string(), ScenarioResult::new("B0"));
    result.scenarios.insert("S1".to_string(), ScenarioResult::new("S1"));
    assert_eq!(result.scenarios.len(), 2);
    assert_eq!(result.mode, "old_wallet");
}

#[test]
fn test_mode_result_with_data() {
    let mut result = ModeResult {
        mode: "new_wallet".to_string(),
        scenarios: std::collections::HashMap::new(),
    };
    let mut s1 = ScenarioResult::new("S1");
    s1.success_count = 42;
    s1.fees_paid_ut = 1000;
    result.scenarios.insert("S1".to_string(), s1);
    let s1_ref = result.scenarios.get("S1").unwrap();
    assert_eq!(s1_ref.success_count, 42);
    assert_eq!(s1_ref.fees_paid_ut, 1000);
}

// ============================================================================
// EnvironmentInfo tests
// ============================================================================

#[test]
fn test_environment_info_collect() {
    let env = EnvironmentInfo::collect().expect("Environment collection should succeed");
    assert!(!env.os.is_empty(),
            "OS info should not be empty (was: '{}')",
            env.os);
}

// ============================================================================
// ResultProfile tests
// ============================================================================

#[test]
fn test_result_profile_new() {
    let env = EnvironmentInfo::collect().expect("Collect environment");
    let config = HarnessConfig::default();
    let profile = ResultProfile::new(env, config);
    assert!(profile.mode_results.is_empty());
    assert!(profile.computed_deltas.is_empty());
    assert_eq!(profile.total_duration_secs, 0.0);
}

#[test]
fn test_result_profile_add_mode_results() {
    let env = EnvironmentInfo::collect().expect("Collect environment");
    let config = HarnessConfig::default();
    let mut profile = ResultProfile::new(env, config);
    let result = ModeResult {
        mode: "old_wallet".to_string(),
        scenarios: std::collections::HashMap::new(),
    };
    profile.add_mode_results("old_wallet".to_string(), result);
    assert_eq!(profile.mode_results.len(), 1);
}

#[test]
fn test_result_profile_add_two_modes() {
    let env = EnvironmentInfo::collect().expect("Collect environment");
    let config = HarnessConfig::default();
    let mut profile = ResultProfile::new(env, config);
    profile.add_mode_results(
        "old_wallet".to_string(),
        ModeResult {
            mode: "old_wallet".to_string(),
            scenarios: std::collections::HashMap::new(),
        },
    );
    profile.add_mode_results(
        "new_wallet".to_string(),
        ModeResult {
            mode: "new_wallet".to_string(),
            scenarios: std::collections::HashMap::new(),
        },
    );
    assert_eq!(profile.mode_results.len(), 2);
}

// ============================================================================
// Config validation tests
// ============================================================================

#[test]
fn test_fee_rate_minimum() {
    let config = HarnessConfig::default();
    assert!(config.fee_rate >= 1, "Fee rate must be >= 1");
}

#[test]
fn test_config_load_missing_file_returns_default() {
    let config = HarnessConfig::load("/nonexistent/path/config.toml")
        .expect("Loading a nonexistent file should return default config");
    assert_eq!(config.fee_rate, 5, "Missing config should return default fee_rate");
    assert_eq!(config.doubling_rounds, 6, "Missing config should return default doubling_rounds");
}
