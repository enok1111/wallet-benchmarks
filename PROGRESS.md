# Wallet Benchmarks — Progress & Status

> **Bounty:** L-tier, 150,000 XTM
> **Issue:** [#1](https://github.com/tari-project/wallet-benchmarks/issues/1)
> **Our PR:** [#3](https://github.com/tari-project/wallet-benchmarks/pull/3) (enok1111)
> **Competitors:** [#4 (sanrishi)](https://github.com/tari-project/wallet-benchmarks/pull/4), [#5 (mkcash)](https://github.com/tari-project/wallet-benchmarks/pull/5), [#6 (roadhero)](https://github.com/tari-project/wallet-benchmarks/pull/6)
> **Last updated:** 2026-05-24

---

## Current State

### ✅ What Works
- **3 wallet modes** implemented (old gRPC, new library+gRPC, payment processor)
- **9 scenarios scaffolded** (B0, S0–S7) with real gRPC integration
- **Config system** with TOML parsing + validation
- **Metrics framework** (ResultProfile, ScenarioResult, deltas)
- **58 tests passing** (26 original + 5 sampler + 6 funding + 21 lifecycle)
- **Zero compiler warnings** ✅
- **Resource sampler** — per-scenario RSS/CPU% at 1Hz with energy score
- **Funding pre-flight** — `src/funding.rs` with 100 XTM minimum check
- **Lifecycle tests** — `tests/mode_lifecycle_tests.rs` (21 tests)
- **Seed fix** — `CipherSeed::random()` per SWvheerden review
- **Confirmation polling** — polls by tx_id for `MinedConfirmed`
- **Wallet daemon** — `tari_ootle_walletd v0.31.0` running with `--authentication none`
- **Account created** — "Benchmark Wallet" at `component_8897...a382`
- **Faucet claim** — `accounts.create_free_test_coins` submitted (tx: `1ef824f3...fee8`)
- **Auth token** — Admin bearer token obtained via `auth.request(["Admin"], "None")`
- **PR #3 open** — 5 SWvheerden review comments all addressed

### ❌ Stuck: Faucet Transaction Won't Confirm
| Item | Detail |
|------|--------|
| Transaction hash | `1ef824f34404b3ec707f52f5c3cf309239fcae506d2f911c05f360ed65b8fee8` |
| Status | `Pending` (never `MinedConfirmed`) |
| Wallet polling | Every 5s → "Transaction result not found. Will check again later." |
| DB records | 1 pending tx in SQLite, 0 UTXOs, 0 vaults, 0 balance |
| Root cause | transaction submitted to Esmeralda testnet via remote indexer but never mined |

**This is the universal blocker** — ALL 4 competitors face the same problem. Nobody has successfully confirmed a testnet transaction. The `create_free_test_coins` faucet submits transactions that stay pending indefinitely.

---

## Competitive Landscape

| Dimension | Us (enok1111) PR#3 | roadhero PR#6 | sanrishi PR#4 | mkcash PR#5 |
|-----------|-------------------|---------------|---------------|-------------|
| Language | Rust | Rust | Rust | **Python** |
| Tests | **58** | **192** | unknown | unknown |
| Warnings | **0** ✅ | **0** ✅ | unknown | unknown |
| Funding pre-flight | ✅ | ✅ | ❌ | ❌ |
| Resource sampler | ✅ | ✅ | ❌ | ❌ |
| Wallet daemon running | ✅ (port 5100) | ❌ | ❌ | ❌ |
| Faucet claim | ✅ (Pending) | ❌ | ❌ | ❌ |
| Auth token | ✅ | ❌ | ❌ | ❌ |
| Maintainer review | ✅ **(addressed!)** | ❌ (none) | ❌ (none) | ❌ (none) |
| Live baseline | ❌ | ❌ | ❌ | ❌ |
| PR age | May 18 (6 days) | May 23 (1 day) | May 18 (6 days) | May 22 (2 days) |

### Seriousness of Each Competitor

**roadhero (PR #6) — MOST SERIOUS**
- 192 tests, feature-complete codebase
- Extremely well-engineered: ScenarioCtx, S4Dispatcher, cross-platform sampler
- Comprehensive documentation: ANALYSIS.md, API_DRIFT.md, RESULT_PROFILE_SCHEMA.md
- 5 analysis documents vs our 2
- **BUT: No maintainer review at all.** No reviewer assigned.
- **BUT: Identically funding-blocked.** Asks for 33,000 tXTM with addresses.
- **BUT: PR is only 1 day old.** Lower trust/visibility.

**sanrishi (PR #4) — MODERATE**
- Rust, feature-complete
- 10 comments on the PR (community engagement)
- Offline signing approach via subprocess
- Funding-blocked (asks maintainer to fund directly)
- No reviewer assigned, no maintainer review

**mkcash (PR #5) — LOWER**
- Python implementation (Rust codebase mismatch)
- Feature-complete code
- No maintainer review
- Submitted May 22 (newest after roadhero)

---

## Why Faucet Transactions Don't Confirm

The Ootle wallet's `accounts.create_free_test_coins` method:
1. Builds a transaction calling the faucet component (`component_010203...`)
2. Submits it to the Esmeralda testnet via remote indexer (`ootle-indexer-a.tari.com`)
3. The wallet polls every 5s via the indexer for the transaction result
4. The transaction sits in `Pending` indefinitely

**Possible causes:**
- Esmeralda testnet may have low mining activity (block time ~3 min)
- The faucet component may be depleted or offline
- The remote indexer may not properly relay the transaction
- The transaction may need a proper base node connection to be mined

**Our advantage:** We at least understand the flow and have the wallet running. Other competitors haven't even gotten this far.

---

## Action Plan

### Phase A: Fund the Wallet (Current Phase)
```
[~] Wait for faucet tx to confirm (may need alternate funding path)
[ ] Use local base node + miner to self-fund
[ ] Or get testnet XTM from maintainer/faucet directly
[ ] Once funded → check balance, verify UTXOs
```

### Phase B: Execute Baseline
```
[ ] Fund 2 more wallet instances (3 total for 3 modes)
[ ] Run the benchmark harness: `cargo run --release`
[ ] Capture baseline_profile.json
[ ] Benchmark all 3 modes × 9 scenarios
```

### Phase C: Win the Bounty
```
[ ] Commit baseline_profile.json to PR #3
[ ] Update PR body with AC verification table
[ ] Push to enok1111/wallet-benchmarks branch
[ ] Request re-review from SWvheerden
[ ] Highlight that we're the FIRST team with a live baseline run
```

---

## Wallet Access Info

| Credential | Value |
|------------|-------|
| Seed words | `able lift photo weasel side deputy vehicle purchase minimum victory what retire barrel insect lumber custom panther pole glide cereal woman innocent lazy uncover` |
| Account address | `otl_esm_1wjjqkaaqamkfsgqp5e6r50jpklkevy3tg5c8lz3nm9hfsd9ysdzva4am3vw0efvuet43vj4fxc0wneyq8xapcwujvg76hu07cyq9x4cprwq4x` |
| Component address | `component_889736f1eb45d452d5c2ba77bd54d0ef2217623a206f54ce1198a19b34e5a382` |
| Owner public key | `74a40b77a0eeec982001a6743a3e41b7ed96122b45307f8a33d96e9834a48344` |
| Auth token | Obtain via `auth.request(["Admin"], "None")` — 137-char JWT, valid for session |
| RPC endpoint | `http://127.0.0.1:5100/json_rpc` |
| Auth header | `Authorization: Bearer <token>` |

### Useful Commands
```bash
# Start wallet daemon (already running)
tari_ootle_walletd --base-path /tmp/wallet_benchmark_test --authentication none --network esme

# Get auth token
TOKEN=$(curl -s http://127.0.0.1:5100/json_rpc \
  -d '{"jsonrpc":"2.0","id":"1","method":"auth.request","params":{"permissions":["Admin"],"credentials":"None"}}' \
  -H 'Content-Type: application/json' | python3 -c "import sys,json; print(json.load(sys.stdin)['result']['token'])")

# Check balance
curl -s http://127.0.0.1:5100/json_rpc \
  -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":"2","method":"accounts.get_balances","params":{"account":{"ComponentAddress":"component_889736f1eb45d452d5c2ba77bd54d0ef2217623a206f54ce1198a19b34e5a382"},"refresh":true}}'

# Request test coins
curl -s http://127.0.0.1:5100/json_rpc \
  -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":"3","method":"accounts.create_free_test_coins","params":{"account":{"ComponentAddress":"component_889736f1eb45d452d5c2ba77bd54d0ef2217623a206f54ce1198a19b34e5a382"},"max_fee":1000000}}'
```
