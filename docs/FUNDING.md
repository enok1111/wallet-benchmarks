# Funding Setup for Wallet Benchmarks

## Requirements

Each wallet mode needs:
- **Minimum balance:** 10,000 tXTM (10,000,000,000 uT)
- **Minimum UTXOs:** 1 (more is better for S4 concurrent tests)
- **Network:** Esmeralda testnet

## Quick Start

```bash
# Set up local mining (finds or builds base node, starts mining)
./scripts/setup_funding.sh

# Check mining status
./scripts/setup_funding.sh --status

# Build from source if binary not found
./scripts/setup_funding.sh --build
```

## How It Works

The script:
1. Finds `minotari_base_node` binary (or builds from source with `--build`)
2. Creates Esmeralda config with CPU mining at `~/.tari/esmeralda/config.toml`
3. Starts the base node in background, logs to `~/.tari/esmeralda/base_node.log`
4. Waits for the node to be ready (HTTP API on port 18143)

## Manual Steps

If the script doesn't work for your setup:

### 1. Install base node

```bash
# Option A: Build from source
git clone --depth 1 https://github.com/tari-project/tari.git
cd tari && cargo build --release -p minotari_base_node

# Option B: Install pre-built (if available)
cargo install minotari_base_node
```

### 2. Start mining

```bash
minotari_base_node --network esmeralda
```

### 3. Fund wallets

Mining rewards go to the base node's mining address. Use the Tari CLI:

```bash
# Scan for your transactions
minotari --network esmeralda scan

# Check balance
minotari --network esmeralda balance
```

### 4. Verify funding

Run the harness with just S0 (funding check):

```bash
cargo run -- --scenarios S0 --modes new
```

This polls until the target balance is reached or times out after 600s.
