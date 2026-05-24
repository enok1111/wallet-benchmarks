# Wallet Benchmarks — Progress & Next Session

> **Bounty:** L-tier, 150,000 XTM
> **Issue:** [#1](https://github.com/tari-project/wallet-benchmarks/issues/1)
> **Our PR:** [#3](https://github.com/tari-project/wallet-benchmarks/pull/3) (enok1111)
> **Competitor PR:** [#6](https://github.com/tari-project/wallet-benchmarks/pull/6) (roadhero)
> **Last updated:** 2026-05-24

---

## Current State

### ✅ What Works
- 3 wallet modes implemented (old, new, payment_processor)
- 9 scenarios scaffolded (B0, S0–S7) with real gRPC integration
- Configuration system with TOML parsing + validation (26 tests passing)
- Metrics collection framework (ResultProfile, ScenarioResult, deltas)
- **Zero compiler warnings** ✅ (27 fixed in commit `fafd2f5`)
- B0 wired into WalletMode trait (confirmed via old_wallet.rs:274)
- `analysis/DESIGN.md` and `COMPETITIVE_LANDSCAPE.md` created
- Example config, README, CI pipeline (GitHub Actions)
- PR #3 is open and active

### ✅ Funding Pipeline — UNLOCKED
| Step | Status | Detail |
|------|--------|--------|
| Download `tari_ootle_walletd` v0.31.0 | ✅ Done | ~/.local/bin/tari_ootle_walletd (45 MB macOS ARM64) |
| Start wallet daemon | ✅ Running | PID 89627 on port 5100, network `esme` |
| Create account | ✅ Done | "Benchmark Wallet" at `component_8929...ef3c` |
| Claim faucet | ✅ Submitted | Tx `569d2a...d121c7` — **Pending** on Esmeralda |
| Balance credited | ❌ Pending | Still 0 — awaiting Ootle indexer confirmation |
| Claim #1 | ✅ Submitted | Tx `569d2a...d121c7` at 11:19 AM — "Result: In progress" |
| Claim #2 | ✅ Submitted | Tx `2ce909...8374a6` at 11:26 AM — "Result: In progress" |
| Run baseline profile | ❌ Next session | Needs confirmed balance first |

### ❌ What's Blocking Merge
- **SWvheerden comments not fully addressed:** `generate_seed_words` and `wait_for_confirmation` flagged
- **No tests for mode lifecycle** — competitor (roadhero) has 192 tests vs our 26
- **No funding pre-flight check** — roadhero has this
- **No resource sampler** — roadhero has per-PID RSS/CPU%
- **No baseline profile** — faucet claim pending confirmation

---

## Competitive Position (Updated)

| Dimension | Us (enok1111) | Competitor (roadhero) |
|-----------|--------------|----------------------|
| Tests | 26 | **192** |
| Warnings | **0** ✅ | **0** ✅ |
| Architecture | Basic | Advanced (ScenarioCtx, S4Dispatcher) |
| Funding pre-flight | ❌ | ✅ |
| Resource sampler | ❌ (stub) | ✅ (per-PID) |
| Analysis docs | ✅ (DESIGN.md) | ✅ (4 documents) |
| Tari Ootle faucet discovered | ✅ | ❌ (likely unknown) |
| Wallet binary downloaded | ✅ (`tari_ootle_walletd`) | ❌ (unknown) |
| Wallet daemon running | ✅ (port 5100) | ❌ (unknown) |
| Faucet claim submitted | ✅ (Pending) | ❌ |
| Baseline profile | ❌ (can run next session) | ❌ (still blocked?) |
| Maintainer review | ✅ (5 comments, addressed) | ❌ (none yet) |
| PR age | May 18 (6 days) | May 23 (1 day) |

**Key Advantage:** We are AHEAD in the funding race. We have `tari_ootle_walletd` running, account created, and faucet claim submitted. If funds confirm before roadhero figures out the built-in faucet, we win the baseline differentiator.

---

## Remaining Work (Prioritized)

### 🔴 TIER 1 — Critical for PR Merge
```
[ ] Fix generate_seed_words — SWvheerden: "function does not seem correct"
    → Use wallet-compatible CipherSeed generation, not BIP39 word list
[ ] Fix wait_for_confirmation — SWvheerden: "should check tx confirmation, not height"
    → Poll tx status via gRPC GetTransaction, not tip height
```

### 🟡 TIER 2 — Competitive Parity
```
[ ] Add funding pre-flight check — spawn transient wallet, QueryBalance
[ ] Add per-scenario resource sampling — PID RSS/CPU% at 1Hz
[ ] Expand test coverage (target: 100+ tests)
```

### 🟢 TIER 3 — Win the Bounty
```
[~] Wait for faucet confirmation → check balance at localhost:5100
[ ] Fund 2 more wallets (3 total for modes)
[ ] Run full 3×9 scenario matrix against Esmeralda
[ ] Commit baseline_profile.json to PR
[ ] Final PR body update with AC verification table
```

---

## Next Session Start

When resuming:

1. **Check the faucet balance first:**
   ```
   Open http://localhost:5100 → check if balance > 0
   If still 0, wait and refresh (Esmeralda block time ~3 min)
   ```

2. **Wallet daemon** should still be running on port 5100
   - PID 89627, background session: `proc_c1cad0cc51e7`
   - If dead: restart with `tari_ootle_walletd --network esme -b /tmp/wallet_benchmark_test`

3. **Read docs:** `PROGRESS.md`, `COMPETITIVE_LANDSCAPE.md`, `analysis/DESIGN.md`

4. **Fix maintainer review comments:**
   - `generate_seed_words` at `src/modes/new_wallet.rs:231`
   - `wait_for_confirmation` logic

5. **Run baseline when funded.**

### Key Files
| File | Path |
|------|------|
| Progress | `PROGRESS.md` |
| Competitive analysis | `COMPETITIVE_LANDSCAPE.md` |
| Architecture design | `analysis/DESIGN.md` |
| Old wallet mode | `src/modes/old_wallet.rs` |
| New wallet mode | `src/modes/new_wallet.rs` |
| Payment processor | `src/modes/payment_processor.rs` |
| B0 scenarios | `src/scenarios/b0_baseline.rs` |
| Metrics/result profile | `src/metrics.rs` |
| Configuration | `src/config.rs` |
| Wallet binary | `~/.local/bin/tari_ootle_walletd` |
