# Competitive Landscape & Action Plan

> Last updated: 2026-05-24
> Bounty: L-tier / 150,000 XTM — tari-project/wallet-benchmarks#1

---

## The Players

| Contributor | PR | Tests | Warnings | Submitted | Branch |
|-------------|----|-------|----------|-----------|--------|
| **enok1111** (us) | [#3](https://github.com/tari-project/wallet-benchmarks/pull/3) | **26** | **0** ✅ | May 18 | `bounty/issue-1-wallet-benchmark-harness` |
| **roadhero** | [#6](https://github.com/tari-project/wallet-benchmarks/pull/6) | **192** | **0** ✅ | May 23 | `bounty/wallet-benchmarks-1-create-benchmarks` |
| **sanrishi** | ? (comments in issue #1) | unknown | unknown | ~May 20 | unknown |

---

## Detailed Comparison

### 🧪 Test Coverage

| Dimension | enok1111 (us) | roadhero |
|-----------|--------------|----------|
| Total tests | 26 | 192 |
| Config/validation | 9 | extensive |
| Metrics/calculations | included | extensive |
| Mode lifecycle | none | extensive (init/run/teardown per mode) |
| S4 dispatcher gating | none | static-grep test enforces no serialization |
| Sentinel/regression | none | `builds_correct_argv_for_single_recipient` — locks CLI argv shape |
| CLI subcommands | basic `run` | `run`, `gen-seed`, `print-address` + `--skip-funding-preflight` |

**Gap:** roadhero has **7.4x more tests** (192 vs 26). We need 100+ more.

### 🏗️ Architecture

| Feature | enok1111 (us) | roadhero |
|---------|--------------|----------|
| WalletMode trait | ✅ basic trait | ✅ `Mode` trait + `ScenarioCtx` with `ScenarioInput` chaining |
| S4 concurrency | multiplexes single gRPC | `S4Dispatcher` trait — per-mode dispatch |
| Subprocess lifecycle | basic spawn+kill | SIGTERM grace → SIGKILL escalation on Drop |
| Resource sampling | stub (returns 0) | per-PID via `/proc` (Linux) / `proc_pidinfo` (macOS) |
| Funding pre-flight | none | transient wallet spawns × 3, `GetBalance` check |
| Redaction safety | none | schema-locked denylist panics at serialize |
| Birthday management | init_wallet_db sets SQL | `CipherSeed` decode → `change_birthday` → `replace_mnemonic` |
| Config merge | `#[serde(default)]` (clean) | similar approach |
| Analysis directory | `analysis/DESIGN.md` | `analysis/DESIGN.md`, `ANALYSIS.md`, `API_DRIFT.md`, `RESULT_PROFILE_SCHEMA.md`, `PR_BODY_PLAN.md` |

**Gap:** roadhero's architecture is objectively more complete. Their `ScenarioCtx`, `S4Dispatcher`, and subprocess lifecycle management are significant advantages.

### 🔧 Code Quality

| Metric | enok1111 (us) | roadhero |
|--------|--------------|----------|
| Warnings | **0** ✅ (fixed in `fafd2f5`) | **0** (clippy `-D warnings`) |
| Clippy | not checked | ✅ `-D warnings` |
| Unused code | `#[allow(dead_code)]` on future methods | none |

### 📋 Bounty Deliverables

| Deliverable | enok1111 (us) | roadhero |
|-------------|--------------|----------|
| 3 wallet modes | ✅ | ✅ |
| 9 scenarios (B0-S7) | ✅ (some stub) | ✅ (full) |
| Config parameters exposed | ✅ | ✅ |
| Structured JSON output | ✅ | ✅ (with schema spec) |
| Environment disclosure | ✅ | ✅ (full) |
| Wallet version pinning | ✅ | ✅ |
| Computed deltas | ✅ | ✅ |
| Baseline profile committed | ❌ (faucet claim pending) | ❌ (unknown if funded) |
| Run instructions | ✅ (README) | ✅ (++ operator notes) |
| Third-party reproducible | ⚠️ partial | ⚠️ partial |
| Harness doesn't hide pain | ⚠️ implied | ✅ explicit (pain points UX doc) |

**Key:** Neither side has a baseline profile. We have a submitted faucet claim — if it confirms before roadhero figures out the built-in faucet, we win this differentiator.

### 👀 Maintainer Interaction

| Interaction | enok1111 (us) | roadhero |
|-------------|--------------|----------|
| SWvheerden review | ✅ 5 comments addressed | ❌ none yet |
| gemini-code-assist review | ✅ 5 comments (4 high/1 medium) | ❌ none |
| Review response | ✅ enok1111 responded to all | N/A |
| PR age | **May 18** (6 days ago) | May 23 (1 day ago) |
| Last update | May 24 (commit `f96fd68`) | May 23 |

**Advantage:** We have maintainer eyes on our PR and addressed all comments. SWvheerden has already seen our code. roadhero hasn't been reviewed yet.

---

## Funding Race — Current Status ⚠️

This is our biggest competitive advantage right now. Let's track it.

| Milestone | Us (enok1111) | roadhero |
|-----------|--------------|----------|
| Tari Ootle faucet discovered | ✅ | ❌ (unknown) |
| `tari_ootle_walletd` downloaded | ✅ (v0.31.0) | ❌ (unknown) |
| Wallet daemon running | ✅ (PID 89627, port 5100) | ❌ (unknown) |
| Account created | ✅ ("Benchmark Wallet") | ❌ (unknown) |
| Faucet claim submitted | ✅ (tx `569d2a...d121c7`) | ❌ (unknown) |
| Balance confirmed | ❌ (Pending on Esmeralda) | ❌ (unknown) |
| Baseline profile generated | ❌ | ❌ |

**Assessment:** We have a 1-2 session lead on the funding front. Don't waste it.

---

## Critical Gaps to Close

### Tier 1 — Must Fix (immediate blockers)

1. ~~Fix 27 warnings~~ **✅ DONE** — `cargo check` clean in commit `fafd2f5`
2. ~~Wire B0 into WalletMode trait~~ **✅ DONE** — confirmed at `old_wallet.rs:274`
3. **Fix `generate_seed_words`** — SWvheerden flagged as "function does not seem correct" in new_wallet.rs:231. Needs wallet-compatible CipherSeed generation.
4. **Fix `wait_for_confirmation`** — SWvheerden flagged "should not work on height but tx confirmation itself". Need proper tx confirmation polling.

### Tier 2 — Competitive Parity (needed to win)

5. Add **funding pre-flight check** — transient wallets, `GetBalance`, require `available_balance ≥ a_fund × 1.1`
6. Add **per-scenario resource sampling** — PID RSS/CPU% at 1Hz via `proc_pidinfo` (macOS)
7. **Expand tests to 100+** — mode lifecycle tests, scenario dispatcher tests, S4 gating tests

### Tier 3 — Win the Baseline Race

8. **Wait for faucet confirmation** — check `http://localhost:5100` next session
9. **Fund 2 more accounts** from faucet (need 3 wallets for 3 modes)
10. **Run full 3×9 benchmark matrix**
11. **Commit `baseline_profile.json`** to PR #3

---

## Concrete Action Plan

### Completed This Session
```
[✅] Fix 27 warnings (clean all dead code, imports, unreachable)
[✅] Confirm B0 wired into WalletMode trait
[✅] Create/update COMPETITIVE_LANDSCAPE.md
[✅] Update PROGRESS.md with current state
[✅] Research Tari Ootle docs
[✅] Download tari_ootle_walletd v0.31.0
[✅] Start wallet daemon on port 5100
[✅] Create Benchmark Wallet account
[✅] Submit faucet claim for testnet funds
[✅] Add analysis/DESIGN.md
[✅] Create analysis/ directory with DESIGN.md
```

### Next Session
```
[ ] Check if faucet balance confirmed at localhost:5100
[ ] Fix generate_seed_words per SWvheerden feedback
[ ] Fix wait_for_confirmation per SWvheerden feedback
[ ] Add funding pre-flight check
[ ] Add per-scenario resource sampling
[ ] Fund remaining wallets
[ ] Run benchmark baseline
[ ] Push fixes + baseline to PR #3
```

---

## Risk Assessment

| Risk | Probability | Impact | Mitigation |
|------|------------|--------|------------|
| roadhero merges first | Medium | Loss of bounty | Move faster on fixes, excel on code quality |
| Faucet fails to deliver funds | Low-Medium | Can't produce baseline | Retry claim, check explorer, use Discord fallback |
| roadhero discovers faucet | Medium | Lose differentiator | We're 1-2 sessions ahead — maintain lead |
| SWvheerden review comments not convincing | Low | PR rejected | Address every comment thoroughly |
| Another competitor appears | Low | More competition | PR #3 has 6-day head start |
