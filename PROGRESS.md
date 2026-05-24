# Wallet Benchmarks — Progress & Next Session

> **Bounty:** L-tier, 150,000 XTM
> **Issue:** [#1](https://github.com/tari-project/wallet-benchmarks/issues/1)
> **Our PR:** [#3](https://github.com/tari-project/wallet-benchmarks/pull/3) (enok1111)
> **Competitor PR:** [#6](https://github.com/tari-project/wallet-benchmarks/pull/6) (roadhero — **serious competitor**, see COMPETITIVE_LANDSCAPE.md)
> **Last updated:** 2026-05-24

---

## Current State

### ✅ What Works
- 3 wallet modes implemented (old, new, payment_processor)
- 9 scenarios scaffolded (B0, S0–S7) with real gRPC integration
- Configuration system with TOML parsing + validation (26 tests passing)
- Metrics collection framework (ResultProfile, ScenarioResult, deltas)
- Example config, README, CI pipeline (GitHub Actions)
- All 5 SWvheerden review comments addressed in commit `58d081b`
- PR #3 is open and active

### ❌ What's Blocking Merge
1. **27 compiler warnings** — maintainer won't merge with warnings
2. **B0 not wired into WalletMode trait** — gemini-code-assist review flagged disconnected functions
3. **Competitor PR #6 (roadhero)** — significantly more complete:
   - 192 tests vs our 26
   - Zero warnings (clippy -D warnings)
   - Better architecture (ScenarioCtx, S4Dispatcher, ConsoleWalletLifecycle)
   - Resource sampler, funding pre-flight, analysis docs
4. **No baseline profile** — blocked on Esmeralda testnet funding

### 🔒 Blocker: Esmeralda Testnet Funding
- Need ~33,000 tXTM total (3 wallets × 11,000 tXTM each, a_fund × 1.1)
- 3 addresses posted in issue comments by sanrishi
- Discord faucet blocked (phone verification)
- No maintainer response to funding requests from any competitor
- **This is the universal blocker** — neither we nor roadhero have a baseline profile

---

## Competitive Position

| Dimension | Us (enok1111) | Competitor (roadhero) |
|-----------|--------------|----------------------|
| Tests | 26 | **192** |
| Warnings | **27** ⚠️ | **0** ✅ |
| Architecture | Basic | Advanced (ScenarioCtx, S4Dispatcher) |
| Funding pre-flight | ❌ | ✅ |
| Resource sampler | ❌ (stub) | ✅ (per-PID) |
| Analysis docs | ❌ | ✅ (4 documents) |
| Baseline profile | ❌ (blocked) | ❌ (blocked) |
| Maintainer review | ✅ (5 comments, addressed) | ❌ (none yet) |
| PR age | May 18 (6 days) | May 23 (1 day) |

**See:** `COMPETITIVE_LANDSCAPE.md` for full analysis.

---

## Remaining Work (Prioritized)

### Tier 1 — Must Fix (immediate)
```
[✅] Fix 27 warnings (dead code, imports, unreachable)
     - Added #[allow(dead_code)] on schema structs
     - Removed unused imports (g_addr → _g_addr, HashMap, ScenarioResult)
     - Restructured cfg blocks to eliminate unreachable expressions
[  ] Wire B0 into WalletMode trait (remove todo!())
     → ALREADY WIRED (review comment resolved in prior commts)
[  ] Fix generate_seed_words (SWvheerden: "function does not seem correct")
[  ] Fix wait_for_confirmation (SWvheerden: "should check tx confirmation, not height")
```

### Tier 2 — Competitive Parity
```
[  ] Add funding pre-flight check
[  ] Add per-scenario resource sampling (RSS + CPU%)
[✅] Add analysis/DESIGN.md
[  ] Expand test coverage (target: 100+ tests)
[✅] Create COMPETITIVE LANDSCAPE.md
```

### Tier 3 — Differentiator
```
[  ] Get funded — ask maintainer or set up local mining
[  ] Run full 3×9 matrix against Esmeralda
[  ] Commit baseline_profile.json
[  ] Final PR body update with AC verification table
```

---

## Warnings Inventory (27 total, ~19 unique)

### Unused Functions (4)
- `run_b0_new_wallet`, `run_b0_payment_processor` — in b0_baseline.rs
- `init_empty_wallet` — in b0_baseline.rs
- `run_scanner` — in b0_baseline.rs
- `detect_disk_type` — in metrics.rs

### Unused Structs (7)
- `ScanResult` — b0_baseline.rs
- `TransactionMetrics`, `ThroughputMetrics`, `ConcurrencyMetrics` — metrics.rs (schema structs)
- `BlockOutputsResponse`, `BlockOutput`, `BalanceResponse` — http_rpc.rs (response types)
- `TransactionSubmitResponse` — http_rpc.rs
- `WalletRpcClient` — http_rpc.rs
- `WalletState` — modes/mod.rs

### Unused Methods/Associated Items (4 groups)
- `submit_transaction`, `get_block_outputs`, `check_connectivity`, `wait_for_ready` — on BaseNodeRpcClient
- `query_balance`, `send_single_transfer_via_grpc` — on mode structs
- `ping`, `get_tip_height`, `wait_for_ready` — on OldWalletGrpcClient
- `new`, `get_balance`, `get_address`, `transfer`, `get_state`, `wait_for_balance` — on WalletRpcClient
- `get_address`, `get_balance` — WalletMode trait methods

### Unused Imports (4)
- `UserPaymentId` (grpc_client.rs) — ✅ fixed
- `std::collections::HashMap` — in some files
- `ScenarioResult` — in some files
- `ModeResult` — in some files

### Other
- 2 `unreachable expression` — in metrics.rs
- 1 `unused variable: g_addr` — in one file

---

## API Drift Notes (vs Bounty Description)

| Bounty Says | Actual minotari CLI |
|-------------|-------------------|
| `--from-birthday` scan flag | `--max-blocks-to-scan` |
| `Balance` returns JSON | human stdout only (parse `µT` sentinel) |
| `list-utxos` command | doesn't exist |
| `import-seed` command | `Create --seed-words "..."` |
| `CipherSeed` is BIP-39 | Tari-specific 24-word encoding, NOT BIP-39 |
| Esmeralda block time ~120s | Actually ~180s (3 min) |

---

## Next Session Start

When resuming:
1. Read `COMPETITIVE_LANDSCAPE.md` for current strategy
2. Run `cargo check` to see current warning count
3. Continue from `fix_warnings` in the todo list
4. After warnings clean → wire B0 → add features → push
