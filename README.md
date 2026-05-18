# Wallet Benchmarks

Reproducible wallet performance test harness for Tari/Minotari that measures `minotari-cli` wallet performance across three modes on Esmeralda testnet.

## Overview

This harness benchmarks wallet performance across:

1. **Old Wallet** (`minotari_console_wallet`) — harness manages the wallet process lifecycle (spawn, wait for ready, run tests, tear down) and interacts via gRPC.
2. **New Wallet** (`minotari-cli` library) — uses the `minotari` crate directly: local UTXO selection, `sign_locked_transaction`, broadcast via HTTP RPC to a base node. No external wallet process.
3. **Payment Processor** — uses the new wallet's batch transaction capability to send 1-to-many transactions.

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

- Rust toolchain (edition 2024)
- Esmeralda testnet access with funded wallet(s)
- `minotari_console_wallet` binary (old wallet mode)
- `minotari` CLI binary (new wallet mode)

## Build

```bash
cargo build --release
```

## Run Instructions

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
- `fee_rate`: Explicit fee rate for all transactions
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
├── src/
│   ├── main.rs              # CLI entry point and orchestrator
│   ├── config.rs            # Configuration loading and validation
│   ├── metrics.rs           # Metrics collection and result profile generation
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

## License

BSD-3-Clause
