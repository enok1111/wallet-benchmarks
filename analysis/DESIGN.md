# Wallet Benchmarks — Design Document

> **Repository:** `tari-project/wallet-benchmarks`
> **Bounty:** L-tier / 150,000 XTM — [#1](https://github.com/tari-project/wallet-benchmarks/issues/1)
> **Status:** Active development
> **Date:** 2026-05-24

---

## 1. Architecture Overview

### 1.1 High-Level Design

```
┌─────────────────────────────────────────────────────────────┐
│                        wallet-benchmarks                     │
├─────────────────────────────────────────────────────────────┤
│  CLI (main.rs)                                              │
│   └─> Config (config.toml → HarnessConfig)                 │
│        └─> Scenario Orchestrator (scenarios/mod.rs)        │
│             ├─> OldWalletMode   (gRPC → console_wallet)    │
│             ├─> NewWalletMode   (lib → minotari crate)     │
│             └─> PaymentProcessorMode (batch tx engine)     │
│                  ├─> 9 Scenarios (B0, S0–S7)               │
│                  └─> ResultProfile → JSON output            │
└─────────────────────────────────────────────────────────────┘
```

### 1.2 Core Abstractions

**WalletMode Trait** (`src/modes/mod.rs`):

```rust
#[async_trait]
trait WalletMode {
    async fn initialize(&mut self, config: &HarnessConfig) -> Result<()>;
    async fn run_scenario(&mut self, scenario_id: &str, config: &HarnessConfig)
        -> Result<ScenarioResult>;
    async fn teardown(&mut self) -> Result<()>;
}
```

Each mode implementation encapsulates the wallet backend type, owns its connection state, and manages its subprocess lifecycle.

**Scenario Orchestrator** (`src/scenarios/mod.rs`):

```rust
pub async fn run_all_scenarios(mode: &mut impl WalletMode, config: &HarnessConfig)
    -> Result<Vec<ScenarioResult>>
```

Iterates B0 through S7, collects wall-clock time, UTXO counts, tip heights, and per-scenario metrics. Outputs a structured `ResultProfile`.

### 1.3 Data Flow

1. CLI reads `config.toml` → `HarnessConfig`
2. Orchestrator creates mode instance with `tempfile::TempDir`
3. `mode.initialize()` — spawns subprocess / opens library DB
4. For each scenario (B0–S7):
   a. Records `wall_clock_secs`, `tip_height_start`
   b. Executes scenario logic (scan, send, wait, measure)
   c. Records `tip_height_end`, success/failure counts
5. `mode.teardown()` — kills subprocess, drops TempDir
6. All results collected into `ResultProfile` → serialized to JSON

---

## 2. Wallet Modes

### 2.1 Old Wallet (minotari_console_wallet via gRPC)

- **File:** `src/modes/old_wallet.rs`
- **Backend:** Spawns `minotari_console_wallet -n --grpc-enabled` as subprocess
- **Protocol:** Tonic gRPC client using `minotari_app_grpc` protos
- **Lifecycle:**
  - Spawn wallet with `std::process::Command`
  - Wait for gRPC port to be ready (poll with backoff)
  - All operations via gRPC (GetBalance, Transfer, GetState, etc.)
  - SIGTERM → SIGKILL on teardown
- **Strengths:** Real wallet binary, end-to-end testing
- **Weaknesses:** Subprocess overhead, gRPC dependency, proto drift

### 2.2 New Wallet (minotari crate library)

- **File:** `src/modes/new_wallet.rs`
- **Backend:** Direct Rust library linking to `minotari` crate
- **Protocol:** In-process API calls (Scanner, TransactionSender, get_balance)
- **Lifecycle:**
  - Create SQLite DB in TempDir
  - Open connection pool with `r2d2_sqlite`
  - Scanner client connects to base node HTTP RPC
  - Close pool on teardown
- **Strengths:** No subprocess overhead, same-process, fast
- **Weaknesses:** Library API stability, password/seed management

### 2.3 Payment Processor

- **File:** `src/modes/payment_processor.rs`
- **Backend:** Same minotari crate library + gRPC for old-wallet path
- **Protocol:** Batch transaction builder + FundLocker
- **Lifecycle:**
  - Database in TempDir (same as new wallet)
  - Spin-wallet via minotari library API
  - Batch mode: single tx with K recipients
  - Individual mode: K sequential txs
- **Strengths:** Measures batch throughput differential
- **Weaknesses:** Needs compiled wallet binary for gRPC path

---

## 3. Scenario Protocol

### 3.1 Scenario Definitions

| ID | Name | Description | Measures |
|----|------|-------------|----------|
| B0 | Baseline Scan | Empty wallet scan from genesis | Floor scan cost |
| S0 | Funding Baseline | Wallet init + receive funding | Init + confirm latency |
| S1 | UTXO Build-up | Doubling rounds → 512 UTXOs | Tx throughput |
| S2 | Scan from Genesis | Checkpoint 1 scan after S1 | UTXO storage overhead |
| S3 | Scan from Birthday | C_p2 height scan | Birthday optimization |
| S4 | Concurrent Construction | Ramped concurrent txs | Concurrency perf |
| S5 | Payment Processor | Batch vs individual | Batch efficiency |
| S6 | Scan from Genesis | Checkpoint 2 post-S5 | Growth delta |
| S7 | Scan from Birthday | C_p2 birthday scan | Lifecycle impact |

### 3.2 Configuration Parameters

All parameters defined in `config.example.toml`:
- `a_fund` — funding amount per wallet (default 10_000_000 µT)
- `c_min` — min confirmations (default 2)
- `s1_rounds` — UTXO doubling rounds (default 9 → 512 UTXOs)
- `s4_ramps` — concurrency levels [8, 16, 32, 64, 128]
- `s5_m` — payment processor recipients per batch (default 10)
- `s5_k` — payment processor batches (default 5)
- `base_node_http_rpc` — base node HTTP endpoint
- `seed_words` — optional seed words for wallet recovery
- `fee_per_gram` — transaction fee in µT/gram

### 3.3 Computed Deltas

| Metric | Formula |
|--------|---------|
| `T_scan_genesis_B0` | S2.genesis_scan_time - B0.scan_time |
| `T_scan_birthday_B0` | S3.birthday_scan_time - B0.scan_time |
| `throughput_multiplier` | S5.individual_total / S5.batch_total |
| `fee_per_recipient_batch` | S5.batch_fees / S5_m |

---

## 4. Key Design Decisions

### 4.1 Why Standalone B0 Functions in b0_baseline.rs?

B0 (baseline scan) is fundamentally different from S0–S7:
- No wallet requires funding
- No transactions are created
- It's a pure scan of blockchain header chain + view-key check

The standalone functions in `b0_baseline.rs` provide a clean, testable implementation that each mode's `run_b0()` delegates to. The code is shared rather than duplicated.

### 4.2 Why tempfile::TempDir?

Each wallet mode gets its own TempDir for data directory. On Drop, the directory is automatically cleaned up. This prevents:
- Wallet state leakage between modes
- Inadvertent testnet pollution
- Orphan temp directories

### 4.3 Why No Automatic Retry?

The harness measures wallet pain — if a transaction takes too long, we record the failure, not retry. This is deliberate per the bounty spec. The harness surfaces the wallet's behavior under load.

---

## 5. API Drift

### 5.1 Bounty Description vs Actual CLI

| Bounty Says | Actual `minotari-cli` |
|-------------|----------------------|
| `--from-birthday` flag | `--max-blocks-to-scan` |
| `Balance` returns JSON | Human stdout (parse `µT` sentinel) |
| `list-utxos` command | Does not exist |
| `import-seed` command | `Create --seed-words "..."` |

### 5.2 gRPC vs HTTP RPC

Old wallet uses gRPC (proto `minotari_app_grpc`).
New wallet and base node use HTTP RPC (REST JSON).

We maintain both interfaces via `OldWalletGrpcClient` and `BaseNodeRpcClient`.

---

## 6. Competitor Analysis

See `COMPETITIVE_LANDSCACE.md` for full analysis. Key design differences from roadhero PR #6:

| Area | Ours | roadhero |
|------|------|----------|
| `ScenarioCtx` | Scenarios/mod orchestrator | Dedicated `ScenarioCtx` with chaining |
| S4 dispatch | Match arm in run_scenario | `S4Dispatcher` trait per mode |
| Subprocess lifecycle | basic kill() | SIGTERM → SIGKILL on Drop |
| Resource sampler | stub (returns 0) | per-PID 1Hz via /proc |
| Funding pre-flight | none | transient wallet x3 check |

Our approach is simpler and sufficient. roadhero's is more robust for production.
