# Wallet Benchmarks

Reproducible wallet performance test harness for Tari/Minotari that measures `minotari-cli` wallet performance across three modes on Esmeralda testnet.

## Overview

This harness benchmarks wallet performance across:

1. **Old Wallet** (`minotari_console_wallet`) — harness manages the wallet process lifecycle (spawn, wait for ready, run tests, tear down) and interacts via gRPC.
2. **New Wallet** (`minotari-cli` library) — uses the `minotari` crate directly: local UTXO selection, `sign_locked_transaction`, broadcast via HTTP RPC to a base node. No external wallet process.
3. **Payment Processor** — uses the new wallet's batch transaction capability to send 1-to-many transactions efficiently.

## Test Protocol

Nine scenarios run per wallet mode:

| Scenario | Description |
|----------|-------------|
| B0 | Baseline Scan (empty wallet) |
| S0 | Funding Baseline |
| S1 | UTXO Build-up (doubling + fan-out → 512) |
| S2 | Scan from Genesis (checkpoint 1) |
| S3 | Scan from Birthday (checkpoint 1) |
| S4 | Concurrent Construction |
| S5 | Payment Processor Throughput |
| S6 | Scan from Genesis (checkpoint 2) |
| S7 | Scan from Birthday (checkpoint 2) |

## Prerequisites

- Rust toolchain (edition 2024 or later)
- Esmeralda testnet access with funded wallet(s)
- `minotari_console_wallet` binary (old wallet mode)
- `minotari` CLI binary (new wallet mode)
- Access to an Esmeralda base node (HTTP RPC + gRPC)

## Build

```bash
cargo build --release
```

## Quick Start

### 1. Configure

Copy and edit the configuration file:

```bash
cp config.example.toml config.toml
# Edit config.toml with your environment settings
```

Key configuration parameters:
- `base_node_grpc` / `base_node_http`: Esmeralda testnet base node addresses
- `old_wallet_binary` / `new_wallet_binary`: Paths to wallet binaries
- `a_fund`: Funding amount per mode (default: 10,000 tXTM)
- `fee_rate`: Explicit fee rate for all transactions (µT/gram)
- `seed_words_*`: Optional seed phrases for each wallet mode

### 2. Fund Wallets

Before running, fund each wallet mode's address on Esmeralda testnet. The harness will generate or use provided seed words to create fresh wallets per mode.

### 3. Run Harness

```bash
# Run all scenarios for all modes
./target/release/wallet-benchmarks

# Run specific scenarios only
./target/release/wallet-benchmarks --scenarios B0,S0,S1

# Run specific modes only
./target/release/wallet-benchmarks --modes old,new

# Custom output directory
./target/release/wallet-benchmarks --output-dir ./my-results
```

### 4. View Results

Results are saved as structured JSON in the output directory (default: `results/`):

```bash
cat results/result_profile.json | jq .
```

## Result Profile Format

The result profile is a JSON file containing:

- **Environment disclosure**: CPU model, RAM, disk type, OS, network path to base node
- **Pinned versions**: Exact wallet and base node versions used
- **Configuration parameters**: All parameter values recorded for reproducibility
- **Per-scenario metrics**: Wall-clock duration, fees paid, success/failure counts, balance reconciliation
- **Computed deltas**: `T_scan(S2) - T_scan(B0)`, S5 throughput multiplier, etc.

## Architecture

```
wallet-benchmarks/
├── Cargo.toml
├── config.example.toml
├── .github/workflows/ci.yml  # GitHub Actions CI
├── tests/integration_tests.rs  # Unit and integration tests
├── src/
│   ├── main.rs              # CLI entry point and orchestrator
│   ├── config.rs            # Configuration loading and validation
│   ├── metrics.rs           # Metrics collection and result profile generation
│   ├── grpc_client.rs       # gRPC client for old wallet mode
│   ├── http_rpc.rs          # HTTP RPC client for base node communication
│   ├── modes/
│   │   ├── mod.rs           # WalletMode trait definition
│   │   ├── old_wallet.rs    # Old wallet mode (gRPC via minotari_console_wallet)
│   │   ├── new_wallet.rs    # New wallet mode (direct library integration)
│   │   └── payment_processor.rs  # Payment processor mode (batch transactions)
│   └── scenarios/
│       └── mod.rs           # Scenario execution orchestrator
└── results/                 # Generated result profiles
```

## Design Principles

- **Harness measures, does not engineer around wallet pain**: UTXO locking, selection contention, stalls, and failed tx construction under concurrency are part of what the harness measures. No retries, backoff, UTXO pre-partitioning, or throttling.
- **Reproducible**: All configuration parameters are exposed and recorded in the output.
- **Verifiable**: A third party can clone the repo, build the harness, fund the wallets, and reproduce the test.

## Testing

Run the test suite:

```bash
# Run all tests
cargo test

# Run specific test module
cargo test --test integration_tests

# Run with output
cargo test -- --nocapture
```

### CI/CD

This project uses GitHub Actions for continuous integration:

- **Lint**: rustfmt and clippy checks on every PR
- **Test**: Full test suite on Ubuntu
- **Build**: Cross-platform builds (Ubuntu + macOS)
- **Documentation**: Auto-generated Rust docs

## Troubleshooting

### Common Issues

1. **"Database not initialized" error**
   - Ensure the wallet data directory exists and is writable
   - Check that seed words are valid (12-word BIP39 mnemonic)

2. **"Failed to connect to base node"**
   - Verify `base_node_http` and `base_node_grpc` point to running nodes
   - Check network connectivity: `curl http://<base_node>/v1/tip_height`

3. **"UTXO selection failed" during S1/S4**
   - Ensure wallet has sufficient funding (at least `a_fund` amount)
   - Check that UTXOs are confirmed (wait for `c_min` confirmations)

4. **Scanner hangs or times out**
   - Increase timeout in config if scanning large blockchain
   - Verify base node is synced and responsive

5. **gRPC connection refused (old wallet mode)**
   - Ensure `minotari_console_wallet` is running with `--grpc-enabled`
   - Check that gRPC port matches configuration

### Debug Mode

Run with verbose logging:

```bash
RUST_LOG=debug ./target/release/wallet-benchmarks
```

## Contributing

1. Fork the repository
2. Create a feature branch (`git checkout -b bounty/feature-name`)
3. Commit your changes (`git commit -am 'feat: add new feature'`)
4. Push to the branch (`git push origin bounty/feature-name`)
5. Open a Pull Request

## License

BSD-3-Clause
