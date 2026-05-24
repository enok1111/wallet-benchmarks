# Next Session — Quick Start

## 1. Check Wallet Funds
Open http://localhost:5100 → check balance at top of page
- If balance > 0: proceed to step 2
- If balance = 0: tx still pending, wait or re-claim from "Claim Testnet Funds" button

## 2. Verify Wallet Daemon
```bash
curl -s http://localhost:5100/ | head -5
lsof -i :5100  # should show tari_ootle_walletd listening
```
If dead: `tari_ootle_walletd --network esme -b /tmp/wallet_benchmark_test`

## 3. Fix Maintainer Comments (before pushing)
- `src/modes/new_wallet.rs:231` — `generate_seed_words` needs CipherSeed
- `src/modes/mod.rs` (or wherever `wait_for_confirmation` lives) — poll tx by hash, not tip height

## 4. Run the Baseline
```bash
cargo check          # should be 0 warnings
cargo test           # should pass 26 tests
cargo run -- run --config config/esmeralda.toml  # (or whatever the run command is)
```

## 5. Commit & Push
```bash
git add -A && git commit -m "fix: ..." && git push
```

## Key Info
| Item | Value |
|------|-------|
| Working dir | `/Users/benya9/Development/wallet-benchmarks` |
| Branch | `bounty/issue-1-wallet-benchmark-harness` |
| Wallet binary | `~/.local/bin/tari_ootle_walletd` |
| Wallet account | "Benchmark Wallet" → `component_892938b59f4ad7739bdd9420512129f4ea33b1a082f85964424656d09ec0ef3c` |
| Address | `otl_esm_1pf2x2nvtlxae6hp9rm24h4tgafh76t04vl4jw5glfdefuf4r5v35dnupalzn0slkp4qkhcreulwgs8nny2nmnjgefuhrrlygxthuqfc55gazf` |
| Pending tx | `569d2a7986f1742913d55257e088b52a18e5dcfca9cfdab0f80e698014d121c7` |
| PR #3 | https://github.com/tari-project/wallet-benchmarks/pull/3 |
| Roadhero PR #6 | https://github.com/tari-project/wallet-benchmarks/pull/6 |
