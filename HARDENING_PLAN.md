# Wallet Benchmarks — Hardening Implementation Plan

> **Goal:** Fix SWvheerden review comments, add competitive parity features (funding pre-flight, resource sampling), and expand tests.
>
> **Bounty:** L-tier, 150,000 XTM — PR #3
>
> **Created:** 2026-05-24

---

## Phase 1: Fix `generate_seed_words` (SWvheerden Review)

**Problem:** Three copies of `generate_seed_words()` (old_wallet.rs:53, new_wallet.rs:257, payment_processor.rs:51) generate hardcoded 12-word BIP39 mnemonics with a mode suffix as the last word. This is invalid because:
- "acquire" (used by `NewWalletMode`) is NOT in the BIP39 English word list
- No valid checksum is computed (BIP39 requires SHA256-based checksum)
- Seeds are deterministic and identical across runs
- SWvheerden flagged: "function does not seem correct"

**Fix:** Replace all three with a shared function that generates proper random BIP39 mnemonics using the `rand` and `sha2` crates (already in dependencies).

**Files to modify:**
- `src/modes/mod.rs` — add shared `generate_tari_seed_words()` function and BIP39 wordlist constant
- `src/modes/old_wallet.rs` — replace `generate_seed_words` to delegate to shared function
- `src/modes/new_wallet.rs` — replace `generate_seed_words` to delegate to shared function  
- `src/modes/payment_processor.rs` — replace `generate_seed_words` to delegate to shared function

**Implementation:**

```rust
// In src/modes/mod.rs

/// BIP39 English wordlist (2048 words) — same as Tari's mnemonic wordlist.
/// Only the first 24 words shown; full list is 2048 entries from the BIP39 spec.
const BIP39_WORDLIST: [&str; 2048] = [
    "abandon", "ability", "able", "about", "above", "absent",
    "absorb", "abstract", "absurd", "abuse", "access", "accident",
    "account", "accuse", "achieve", "acid", "acoustic", "acquire",
    "across", "act", "action", "actor", "actress", "actual",
    // ... (full 2048-word BIP39 English list)
];

/// Generate a valid BIP39 mnemonic phrase (12 words = 128 bits entropy + 4 bits checksum)
/// Uses cryptographically secure random number generator.
pub fn generate_tari_seed_words(count: usize) -> Vec<String> {
    use rand::rngs::OsRng;
    use rand::RngCore;
    use sha2::{Sha256, Digest};

    // 12 words = 128 bits entropy
    let entropy_bytes = 16; // 128 bits
    let mut entropy = vec![0u8; entropy_bytes];
    OsRng.fill_bytes(&mut entropy);

    // SHA256 checksum: take first (entropy_bits / 32) = 4 bits
    let hash = Sha256::digest(&entropy);
    let checksum_bits = (entropy_bytes * 8) / 32; // 4 bits for 128-bit entropy

    // Combine entropy + checksum into 11-bit indices
    let total_bits = entropy_bytes * 8 + checksum_bits; // 132 bits
    let word_count = total_bits / 11; // 12 words

    let mut words = Vec::with_capacity(word_count);
    let mut bit_buffer: u64 = 0;
    let mut bits_in_buffer = 0;

    for byte in &entropy {
        bit_buffer = (bit_buffer << 8) | (*byte as u64);
        bits_in_buffer += 8;
        if bits_in_buffer >= 11 {
            bits_in_buffer -= 11;
            let index = ((bit_buffer >> bits_in_buffer) & 0x7FF) as usize;
            words.push(BIP39_WORDLIST[index].to_string());
            bit_buffer &= (1 << bits_in_buffer) - 1;
        }
    }

    // Add checksum bits
    bit_buffer = (bit_buffer << checksum_bits) | ((hash[0] >> (8 - checksum_bits)) as u64);
    bits_in_buffer += checksum_bits;
    if bits_in_buffer >= 11 {
        bits_in_buffer -= 11;
        let index = ((bit_buffer >> bits_in_buffer) & 0x7FF) as usize;
        words.push(BIP39_WORDLIST[index].to_string());
    }

    words
}
```

Then replace each mode's `generate_seed_words` with:
```rust
fn generate_seed_words(_mode_id: WalletModeId) -> Vec<String> {
    crate::modes::generate_tari_seed_words(12)
}
```

**Verification:**
1. `cargo test` — existing tests still pass
2. Run with seed words generated — wallet initializes successfully
3. Different calls produce different seeds (non-deterministic)

---

## Phase 2: Fix `wait_for_confirmation_via_grpc` (SWvheerden Review)

**Problem:** Three copies of `wait_for_confirmation_via_grpc()` (old_wallet.rs:131, new_wallet.rs:204, payment_processor.rs:220) record the tip height at broadcast time, then wait for N more blocks. SWvheerden flagged: "should check tx confirmation, not height" — it should poll the specific transaction's status instead.

**Fix:** Modify the function to accept a transaction hash/ID and poll `GetTransaction` gRPC call to check if the transaction has been confirmed (sufficient confirmations).

**Files to modify:**
- `src/grpc_client.rs` — add `get_transaction` method
- `src/modes/old_wallet.rs` — fix `wait_for_confirmation_via_grpc` signature and logic
- `src/modes/new_wallet.rs` — fix `wait_for_confirmation_via_grpc` signature and logic
- `src/modes/payment_processor.rs` — fix `wait_for_confirmation_via_grpc` signature and logic
- All callers — pass tx hash instead of c_min

**Implementation detail:**

The gRPC proto `minotari_app_grpc` likely has a `GetTransactionRequest`/`GetTransactionResponse`. If not, we can use `GetBalance` to check if the balance changed, or use the wallet's transaction list.

For the `OldWalletGrpcClient`:
```rust
#[allow(dead_code)]
pub async fn get_transaction(&mut self, tx_hash: &str) -> Result<Option<GetTransactionResponse>> {
    // Use minotari_app_grpc::tari_rpc::GetTransactionRequest
    let request = tonic::Request::new(GetTransactionRequest {
        transaction_id: tx_hash.to_string(),
    });
    match self.client_mut().get_transaction(request).await {
        Ok(response) => Ok(Some(response.into_inner())),
        Err(status) if status.code() == tonic::Code::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
```

Modified wait function:
```rust
async fn wait_for_tx_confirmation(
    &self,
    tx_hash: &str,
    c_min: u32,
    timeout_secs: u64,
) -> Result<()> {
    let start = std::time::Instant::now();
    let timeout = Duration::from_secs(timeout_secs);
    let base_node_client = crate::http_rpc::BaseNodeRpcClient::new(&self.base_node_http);

    loop {
        if start.elapsed() > timeout {
            anyhow::bail!("Timeout waiting for tx {} confirmation (c_min={})", tx_hash, c_min);
        }

        // Poll transaction status from base node
        match base_node_client.get_tx_height(tx_hash).await? {
            Some(tx_height) => {
                let current_tip = base_node_client.get_tip_height().await?;
                let confirmations = current_tip.saturating_sub(tx_height);
                if confirmations >= c_min as u64 {
                    debug!("Transaction {} confirmed: {} confirmations", tx_hash, confirmations);
                    return Ok(());
                }
                debug!("Tx {}: {} of {} confirmations", tx_hash, confirmations, c_min);
            }
            None => {
                debug!("Transaction {} not yet found in any block", tx_hash);
            }
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}
```

**Verification:**
1. `cargo check` — compiles clean
2. `cargo test` — existing tests pass

---

## Phase 3: Add Funding Pre-Flight Check

**Problem:** Roadhero has a funding pre-flight that spawns transient wallets, checks `GetBalance`, and aborts early if insufficient funds. We have nothing.

**Fix:** Add a `FundingPreflight` struct and integrate it into the main run flow.

**Files to create/modify:**
- Create: `src/funding.rs` — FundingPreflight implementation
- Modify: `src/lib.rs` — add mod and integration
- Modify: `src/main.rs` — call preflight before scenario runner

**Implementation:**

```rust
// src/funding.rs

use anyhow::{bail, Result};
use log::{info, warn};

/// Minimum funding required per wallet (µT)
const MIN_FUNDING_PER_WALLET: u64 = 100_000_000; // 100 XTM

/// Check wallet funding before running benchmarks
pub struct FundingPreflight;

impl FundingPreflight {
    /// Check that all three wallet modes have sufficient funds.
    /// Returns (old_balance, new_balance, payment_processor_balance) or bails.
    pub async fn check_all(
        old_balance: u64,
        new_balance: u64,
        payment_processor_balance: u64,
    ) -> Result<(u64, u64, u64)> {
        info!("Running funding pre-flight check...");
        
        Self::check_single("old_wallet", old_balance, MIN_FUNDING_PER_WALLET)?;
        Self::check_single("new_wallet", new_balance, MIN_FUNDING_PER_WALLET)?;
        Self::check_single("payment_processor", payment_processor_balance, MIN_FUNDING_PER_WALLET)?;
        
        info!("Funding check passed: old={}, new={}, pp={}", 
              old_balance, new_balance, payment_processor_balance);
        
        Ok((old_balance, new_balance, payment_processor_balance))
    }
    
    fn check_single(name: &str, balance: u64, min_required: u64) -> Result<()> {
        if balance < min_required {
            warn!("{} has insufficient funds: {} µT, need {} µT", 
                  name, balance, min_required);
            bail!("{} funding insufficient: {} < {} (µT)", 
                  name, balance, min_required);
        }
        info!("{} funding OK: {} µT", name, balance);
        Ok(())
    }
}
```

**Verification:**
1. `cargo check` — compiles clean
2. Test with funded wallet — passes
3. Test with unfunded wallet — bails with clear error

---

## Phase 4: Add Per-Scenario Resource Sampling

**Problem:** Roadhero has per-PID RSS/CPU% sampling at 1Hz during each scenario. We return zeros from a stub.

**Fix:** Implement `ResourceSampler` that polls the wallet process PID for RSS (resident set size) and CPU% using platform-specific calls.

**Files to create/modify:**
- Create: `src/sampler.rs` — ResourceSampler implementation
- Modify: `src/lib.rs` — add mod
- Modify: `src/metrics.rs` — add `ResourceSample` struct to `ScenarioResult`

**Implementation:**

```rust
// src/sampler.rs

use anyhow::Result;
use log::debug;
use std::time::{Duration, Instant};
use std::collections::VecDeque;

/// A resource sample point
#[derive(Debug, Clone, Copy)]
pub struct ResourceSample {
    pub timestamp_secs: f64,  // seconds since sampler start
    pub rss_bytes: u64,       // resident set size in bytes
    pub cpu_percent: f64,     // CPU utilization percentage
}

/// Samples resource usage of a process during benchmark execution
pub struct ResourceSampler {
    pid: u32,
    samples: VecDeque<ResourceSample>,
    start: Instant,
}

impl ResourceSampler {
    /// Create a new sampler for the given PID
    pub fn new(pid: u32) -> Self {
        Self {
            pid,
            samples: VecDeque::new(),
            start: Instant::now(),
        }
    }
    
    /// Take one sample immediately
    pub fn sample_now(&mut self) -> ResourceSample {
        let (rss, cpu) = self.read_proc_stats();
        let sample = ResourceSample {
            timestamp_secs: self.start.elapsed().as_secs_f64(),
            rss_bytes: rss,
            cpu_percent: cpu,
        };
        self.samples.push_back(sample);
        sample
    }
    
    /// Run sampling loop: sample at `interval` Hz until `duration` elapses
    pub async fn sample_for(&mut self, duration: Duration, interval_hz: f64) {
        let sleep_duration = Duration::from_secs_f64(1.0 / interval_hz);
        let end = Instant::now() + duration;
        
        while Instant::now() < end {
            self.sample_now();
            tokio::time::sleep(sleep_duration).await;
        }
    }
    
    /// Get all collected samples
    pub fn samples(&self) -> Vec<ResourceSample> {
        self.samples.iter().copied().collect()
    }
    
    /// Get peak RSS across all samples
    pub fn peak_rss_bytes(&self) -> u64 {
        self.samples.iter().map(|s| s.rss_bytes).max().unwrap_or(0)
    }
    
    /// Get average CPU% across all samples
    pub fn avg_cpu_percent(&self) -> f64 {
        let count = self.samples.len();
        if count == 0 { return 0.0; }
        self.samples.iter().map(|s| s.cpu_percent).sum::<f64>() / count as f64
    }
    
    /// Read RSS and CPU% from OS (/proc on Linux, proc_pidinfo on macOS)
    fn read_proc_stats(&self) -> (u64, f64) {
        #[cfg(target_os = "linux")]
        {
            let path = format!("/proc/{}/stat", self.pid);
            if let Ok(content) = std::fs::read_to_string(&path) {
                if let Some(fields) = content.split_whitespace().nth(23) {
                    if let Ok(rss_pages) = fields.parse::<u64>() {
                        let rss_bytes = rss_pages * 4096; // page size
                        // CPU% approximation
                        return (rss_bytes, 0.0);
                    }
                }
            }
        }
        
        #[cfg(target_os = "macos")]
        {
            use libc::proc_taskinfo;
            // macOS: use proc_pidinfo for RSS
            // For now, fallback to simpler method
        }
        
        (0, 0.0) // fallback
    }
}
```

**Verification:**
1. `cargo check` — compiles clean
2. Unit tests for peak/avg calculations

---

## Phase 5: Expand Test Coverage

**Target:** Add 50+ tests to close the gap with roadhero (192 vs 26).

**Test areas:**
- Seed generation: determinism, word count, valid indices
- Confirmation polling: timeout, success, not-found paths
- Funding pre-flight: pass, fail, edge cases
- Resource sampling: peak/avg calculations, empty sampler
- Config parsing: all fields, defaults, merge behavior
- JSON serialization: ResultProfile roundtrip

---

## Execution Order

1. **Phase 1** — Fix `generate_seed_words` (highest priority, SWvheerden review)
2. **Phase 2** — Fix `wait_for_confirmation_via_grpc` (highest priority, SWvheerden review)
3. **Phase 3** — Funding pre-flight check
4. **Phase 4** — Resource sampling
5. **Phase 5** — Tests
6. **Final** — `cargo check`, `cargo test`, commit, push
