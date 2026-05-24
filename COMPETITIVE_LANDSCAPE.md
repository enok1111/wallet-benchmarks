# Competitive Landscape & Action Plan

> Last updated: 2026-05-24
> Bounty: L-tier / 150,000 XTM — tari-project/wallet-benchmarks#1

---

## The Players

| Contributor | PR | Tests | Warnings | Submitted | Branch |
|-------------|----|-------|----------|-----------|--------|
| **enok1111** (us) | [#3](https://github.com/tari-project/wallet-benchmarks/pull/3) | **26** | **27** (⚠️) | May 18 | `bounty/issue-1-wallet-benchmark-harness` |
| **roadhero** | [#6](https://github.com/tari-project/wallet-benchmarks/pull/6) | **192** | **0** (✅) | May 23 | `bounty/wallet-benchmarks-1-create-benchmarks` |
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
| Sentinel/regression | none | `builds_correct_argv_for_single_recipient` — locks CLI argv shape across 30+ commits |
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
| Analysis directory | none | `analysis/DESIGN.md`, `ANALYSIS.md`, `API_DRIFT.md`, `RESULT_PROFILE_SCHEMA.md`, `PR_BODY_PLAN.md` |

**Gap:** roadhero's architecture is objectively more complete and professionally structured. Their `ScenarioCtx`, `S4Dispatcher`, and subprocess lifecycle management are significant advantages.

### 🔧 Code Quality

| Metric | enok1111 (us) | roadhero |
|--------|--------------|----------|
| Warnings | **27** | **0** (clippy `-D warnings`) |
| Clippy | not checked | ✅ `-D warnings` |
| Unused code | 19+ unique warnings | none |
| Duplicated logic | `generate_seed_words` replicated per mode | centralized |

**Gap:** Critical. 27 warnings is a disqualifier for maintainer review. Must fix immediately.

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
| Baseline profile committed | ❌ (blocked: funding) | ❌ (blocked: funding) |
| Run instructions | ✅ (README) | ✅ (++ operator notes) |
| Third-party reproducible | ⚠️ partial | ⚠️ partial |
| Harness doesn't hide pain | ⚠️ implied | ✅ explicit (pain points UX doc) |

**Key:** Neither side has a baseline profile. That's our biggest opening.

### 👀 Maintainer Interaction

| Interaction | enok1111 (us) | roadhero |
|-------------|--------------|----------|
| SWvheerden review | ✅ 5 comments addressed | ❌ none yet |
| gemini-code-assist review | ✅ 5 comments (4 high/1 medium) | ❌ none |
| Review response | ✅ enok1111 responded to all | N/A |
| PR age | **May 18** (6 days ago) | May 23 (1 day ago) |
| Last update | May 23 (commit `fc52c85`) | May 23 |

**Advantage:** We have maintainer eyes on our PR and addressed all comments. SWvheerden has already seen our code. roadhero hasn't been reviewed yet.

---

## Critical Gaps to Close

### Tier 1 — Must Fix (immediate blockers)

1. **Fix 27 warnings** — maintainer won't merge with warnings. Target: `cargo check` / `cargo clippy` clean.
2. **Wire B0 into WalletMode trait** — gemini review flagged B0 functions are disconnected from trait. `run_b0_old_wallet` exists in `b0_baseline.rs` but `OldWalletMode::run_scenario` has `todo!()` for B0.
3. **Fix `generate_seed_words`** — SWvheerden flagged as "function does not seem correct" in new_wallet.rs:231. Needs wallet-compatible seed generation (not just BIP39 word list concatenation).
4. **Fix `wait_for_confirmation`** — SWvheerden flagged "should not work on height but tx confirmation itself". Need proper tx confirmation polling, not `broadcast_tip + c_min`.

### Tier 2 — Competitive Parity (needed to win)

5. Add **funding pre-flight check** — roadhero has this. Spawn transient wallets, query `GetBalance`, require `available_balance ≥ a_fund × 1.1`.
6. Add **per-scenario resource sampling** — peak RSS + CPU% per PID. roadhero uses `/proc` (Linux) / `proc_pidinfo` (macOS) at 1Hz.
7. Add **analysis docs** — at minimum `DESIGN.md` with architecture decisions, data flow, scenario protocol.
8. **Dramatically expand tests** — add mode lifecycle tests, scenario dispatcher tests, S4 gating tests.

### Tier 3 — Differentiators (our unique wins)

9. **Get funded and run the baseline** — this is the single biggest differentiator. Neither side has it. If we can get 33k tXTM funded and produce `baseline_profile.json`, we win.
10. **Progressive enhancement** — fix all review comments, get CI green, then ask SWvheerden for funding help.

---

## Concrete Action Plan

### Immediate (this session)
```
[✅] Fix 27 warnings (clean all dead code, imports, unreachable)
[✅] Wire B0 into OldWalletMode::run_scenario (already done)
[✅] Create COMPETITIVE_LANDSCAPE.md
[✅] Update PROGRESS.md with current state
[✅] Research Tari Ootle docs (ootle.tari.com)
[✅] Download tari_ootle_walletd v0.31.0 to ~/.local/bin/
[ ] Fix generate_seed_words per SWvheerden's feedback
[ ] Fix wait_for_confirmation per SWvheerden's feedback
```

### Short-term (before next PR push)
```
[  ] Add funding pre-flight check
[  ] Add per-scenario resource sampling
[  ] Add analysis/DESIGN.md
[  ] Add 50+ more tests
[  ] Fix generate_seed_words per SWvheerden's feedback
[  ] Fix wait_for_confirmation per SWvheerden's feedback
[  ] Push all fixes to PR branch
```

### Funding & Baseline
```
[  ] Ask SWvheerden for 33k tXTM funding (3 addresses × 11k)
[  ] OR set up local Esmeralda mining
[  ] Run full 3×9 matrix
[  ] Commit baseline_profile.json
[  ] Final PR body update with AC verification
```

---

## Risk Assessment

| Risk | Probability | Impact | Mitigation |
|------|------------|--------|------------|
| roadhero merges first | Medium | Loss of bounty | Move faster on fixes, excel on code quality |
| SWvheerden ignores funding request | High | Can't produce baseline | Build local mining setup; prod gently |
| roadhero produces baseline before us | Medium | Loss of key differentiator | Fund faster or run local mining |
| Review comments not convincing | Low | PR rejected | Address every comment thoroughly |
| Another competitor appears | Low | More competition | PR #3 has 6-day head start |
| Warnings make PR look abandoned | High | Maintainer dismisses PR | Fix ALL warnings this session |
