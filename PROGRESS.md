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
- Zero compiler warnings (27 fixed in commit fafd2f5)
- B0 wired into WalletMode trait (existing, not an issue)
- analysis/DESIGN.md and COMPETITIVE_LANDSCAPE.md created
- Example config, README, CI pipeline (GitHub Actions)
- PR #3 is open and active

### ❌ What's Blocking Merge
- **SWvheerden comments not fully addressed:** `generate_seed_words` and `wait_for_confirmation` are flagged
- **No tests for mode lifecycle** — competitor (roadhero) has 192 tests vs our 26
- **No funding pre-flight check** — roadhero has this
- **No resource sampler** — roadhero has per-PID RSS/CPU%
- **No baseline profile** — needs Esmeralda testnet run

### 🚨 BREAKING DISCOVERY: Funding is NO LONGER Blocked!

The **`tari_ootle_walletd`** (v0.31.0, downloaded to `~/.local/bin/`) has a **built-in faucet**:
- "Claim Testnet Funds" button in web UI at `http://localhost:5100`
- No Discord verification needed
- Testnet XTM is claimable immediately from the wallet web dashboard

This unblocks the **global bounty blocker** — neither we nor roadhero had baseline profiles because nobody could get funded. We can fix this right now.

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
| Tari Ootle docs researched | ✅ | ❌ (likely) |
| Wallet binary downloaded | ✅ (`tari_ootle_walletd`) | ❌ (unknown) |
| Baseline profile | ❌ (can now run!) | ❌ (still blocked?) |
| Maintainer review | ✅ (5 comments, addressed) | ❌ (none yet) |
| PR age | May 18 (6 days) | May 23 (1 day) |

**Key Advantage:** We now know `tari_ootle_walletd` has a built-in faucet. If we claim funds and run the baseline before roadhero figures this out, we win the differentiator.

---

## Critical Findings from Tari Ootle Docs

### Wallet Daemon (`tari_ootle_walletd` v0.31.0)

| Finding | Detail |
|---------|--------|
| **Binary** | Downloaded to `~/.local/bin/tari_ootle_walletd` (45 MB macOS ARM64) |
| **Latest release** | v0.31.0 (May 20, 2026) |
| **Built-in faucet** | ✅ "Claim Testnet Funds" in Web UI at localhost:5100 |
| **Network flag** | `--network esme` for Esmeralda |
| **Base path** | `-b /path` for custom data dir |
| **Seed restoration** | `--seed-words "..."` |
| **Auth** | `--auth webauthn` (passkey), defaults to none |
| **Subcommands** | `run`, `create-account`, `seed-words`, `reset`, `new-viewable-balance-key` |
| **JSON-RPC** | Wallet daemon serves JSON-RPC for programmatic interaction |
| **CLI (separate)** | `cargo install tari-ootle-cli` → `tari` command for template dev |
| **Indexer API** | OpenAPI at `ootle.tari.com/indexer/indexer-api.html` |

### Transaction API (`tari_ootle_transaction`)

- `Transaction::builder(Network)` → `.pay_fee_from_component()`, `.call_function()`, `.call_method()`, `.build_and_seal(secret_key)`
- Workspace for passing values between instructions (`put_last_instruction_output_on_workspace`)
- Blob system for large payloads (WASM templates)
- Account creation via `create_account(public_key)` (idempotent)

### How This Changes Our Approach

1. **Funding solved** — built-in faucet in wallet daemon web UI
2. **New wallet target** — benchmark should test `tari_ootle_walletd` JSON-RPC alongside/instead of raw `minotari` crate
3. **Differentiator unlocked** — if we claim funds and run baseline before roadhero, we win
4. **Old wallet** (`minotari_console_wallet`) = Minotari L1 — still needed for 3-mode requirement

---

## Remaining Work (Prioritized)

### 🔴 TIER 1 — Critical for PR Merge
```
[  ] Fix generate_seed_words — SWvheerden: "function does not seem correct"
     → Use wallet-compatible CipherSeed generation, not BIP39 word list
[  ] Fix wait_for_confirmation — SWvheerden: "should check tx confirmation, not height"
     → Poll tx status via gRPC GetTransaction, not tip height
```

### 🟡 TIER 2 — Competitive Parity
```
[  ] Add funding pre-flight check — spawn transient wallet, QueryBalance
[  ] Add per-scenario resource sampling — PID RSS/CPU% at 1Hz
[  ] Expand test coverage (target: 100+ tests)
```

### 🟢 TIER 3 — Win the Bounty
```
[  ] Run tari_ootle_walletd daemon with --network esme
[  ] Open http://localhost:5100 → Create account → Claim faucet funds
[  ] Fund 3 benchmark wallets from faucet
[  ] Run full 3×9 scenario matrix
[  ] Commit baseline_profile.json to PR
[  ] Final PR body update with AC verification table
```

---

## Next Session Start

When resuming:
1. Read docs: `PROGRESS.md`, `COMPETITIVE_LANDSCAPE.md`, `analysis/DESIGN.md`
2. Check compilation: `cargo check` — should be zero warnings
3. Fix generate_seed_words + wait_for_confirmation review comments
4. Run wallet daemon + claim faucet funds
5. Execute baseline benchmarks against Esmeralda
