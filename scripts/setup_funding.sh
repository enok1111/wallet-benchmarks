#!/usr/bin/env bash
# Set up local mining on Esmeralda testnet and fund wallet benchmarks.
#
# What it does:
#   1. Finds or builds a Tari base node binary
#   2. Creates mining config for Esmeralda testnet
#   3. Starts the base node with CPU mining in the background
#   4. Waits for sync, then mines blocks to generate tXTM
#   5. Outputs wallet addresses for manual funding (or auto-funds if CLI supports it)
#
# Usage: ./scripts/setup_funding.sh [--build] [--threads N] [--target-blocks N]
#   --build       Build minotari_base_node from source if not found
#   --threads N   CPU mining threads (default: 4)
#   --target-blocks N  Stop after mining N blocks (default: 5)
#   --status      Check mining status without starting

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
DATA_DIR="${HOME}/.tari/esmeralda"
CONFIG_FILE="${DATA_DIR}/config.toml"
LOG_FILE="${DATA_DIR}/base_node.log"
PID_FILE="${DATA_DIR}/base_node.pid"

MINING_THREADS=4
TARGET_BLOCKS=5
DO_BUILD=false
CHECK_STATUS=false

# Parse arguments
while [[ $# -gt 0 ]]; do
  case "$1" in
    --build) DO_BUILD=true; shift ;;
    --threads) MINING_THREADS="$2"; shift 2 ;;
    --target-blocks) TARGET_BLOCKS="$2"; shift 2 ;;
    --status) CHECK_STATUS=true; shift ;;
    *) echo "Unknown option: $1"; exit 1 ;;
  esac
done

# Find base node binary
find_base_node() {
  local candidates=(
    "/usr/local/bin/minotari_base_node"
    "/usr/local/bin/minotari_node"
    "${HOME}/.cargo/bin/minotari_base_node"
    "${HOME}/.cargo/bin/minotari_node"
    "${HOME}/Development/tari/target/release/minotari_base_node"
    "${HOME}/Development/tari/target/release/minotari_node"
    "${HOME}/Development/minotari/target/release/minotari_base_node"
    "${HOME}/Development/minotari/target/release/minotari_node"
  )

  for candidate in "${candidates[@]}"; do
    if [[ -x "$candidate" ]]; then
      echo "$candidate"
      return 0
    fi
  done
  return 1
}

# Build base node from source
build_base_node() {
  echo "Building minotari_node from source..."
  local tari_dir="${HOME}/Development/tari"

  if [[ ! -d "$tari_dir" ]]; then
    echo "Cloning Tari repository..."
    git clone --depth 1 https://github.com/tari-project/tari.git "$tari_dir"
  fi

  cd "$tari_dir"
  cargo build --release -p minotari_node
  echo "Built: ${tari_dir}/target/release/minotari_node"
}

# Create mining config
create_config() {
  mkdir -p "$DATA_DIR/config/base_node"

  cat > "$CONFIG_FILE" << EOF
[base_node]
mining_enabled = true
grpc_enabled = true
grpc_address = "/ip4/127.0.0.1/tcp/18142"

[esmeralda.base_node]
identity_file = "config/base_node_id_esmeralda.json"

[miner]
num_mining_threads = ${MINING_THREADS}
proof_of_work_algo = "Sha3X"
mine_on_tip_only = true

[database]
path = "${DATA_DIR}/db"

[logging]
path = "${DATA_DIR}/logs"
EOF

  echo "Config written to ${CONFIG_FILE}"
}

# Check if node is already running
is_running() {
  if [[ -f "$PID_FILE" ]]; then
    local pid
    pid=$(cat "$PID_FILE")
    if kill -0 "$pid" 2>/dev/null; then
      return 0
    fi
    rm -f "$PID_FILE"
  fi
  return 1
}

# Start the base node
start_node() {
  local binary="$1"

  if is_running; then
    local pid
    pid=$(cat "$PID_FILE")
    echo "Base node already running (PID: ${pid})"
    return 0
  fi

  create_config

  echo "Starting base node with ${MINING_THREADS} mining threads..."
  echo "  Binary: ${binary}"
  echo "  Config: ${CONFIG_FILE}"
  echo "  Log:    ${LOG_FILE}"

  "$binary" --network esmeralda --config "${CONFIG_FILE}" \
    --mining-enabled --non-interactive-mode --disable-splash-screen \
    >> "$LOG_FILE" 2>&1 &
  local pid=$!
  echo "$pid" > "$PID_FILE"

  echo "Base node started (PID: ${pid})"
  echo ""
  echo "Waiting for initial sync (watch ${LOG_FILE} for progress)..."

  # Wait for the node to start accepting gRPC connections
  local wait=0
  while [[ $wait -lt 120 ]]; do
    if curl -sf "http://127.0.0.1:18142" >/dev/null 2>&1; then
      echo "Base node is ready (took ${wait}s)"
      return 0
    fi
    if ! kill -0 "$pid" 2>/dev/null; then
      echo "ERROR: Base node exited prematurely. Check ${LOG_FILE}"
      return 1
    fi
    sleep 2
    wait=$((wait + 2))
  done

  echo "WARNING: Base node may still be syncing (waited 120s)"
  echo "Check logs: tail -f ${LOG_FILE}"
  return 0
}

# Check mining progress
check_status() {
  if ! is_running; then
    echo "Base node is not running"
    return 1
  fi

  local pid
  pid=$(cat "$PID_FILE")
  echo "Base node running (PID: ${pid})"

  # Try to get status via gRPC
  if curl -sf "http://127.0.0.1:18142" >/dev/null 2>&1; then
    echo "Node is accepting connections on port 18142"
  else
    echo "Node may still be starting up"
  fi

  echo ""
  echo "Recent mining/sync activity:"
  if [[ -f "$LOG_FILE" ]]; then
    grep -i "mine\|block\|reward\|sync\|tip" "$LOG_FILE" | tail -10 || echo "  No activity yet"
  fi

  echo ""
  echo "Full log: ${LOG_FILE}"
}

# Get wallet addresses from config
get_wallet_addresses() {
  echo "Wallet benchmark addresses:"
  echo ""
  echo "Run the harness to generate wallet addresses:"
  echo "  cd ${PROJECT_DIR}"
  echo "  cargo run -- --scenarios B0,S0 --modes new"
  echo ""
  echo "Or check existing wallets:"
  echo "  minotari --network esmeralda --config ${PROJECT_DIR}/config.toml show-seed-words"
}

# Main
main() {
  echo "=== Tari Wallet Benchmark Funding Setup ==="
  echo ""

  if $CHECK_STATUS; then
    check_status
    return
  fi

  # Find or build base node binary
  local binary
  binary=$(find_base_node) || true

  if [[ -z "$binary" ]]; then
    if $DO_BUILD; then
      build_base_node
      binary="${HOME}/Development/tari/target/release/minotari_node"
    else
      echo "minotari_node not found."
      echo "Options:"
      echo "  1. Run with --build to compile from source"
      echo "  2. Install manually: cargo install minotari_node"
      echo "  3. Set MINOTARI_BASE_NODE env var to binary path"
      exit 1
    fi
  fi

  # Start the node
  start_node "$binary"

  echo ""
  echo "=== Mining Setup Complete ==="
  echo ""
  echo "The base node is now mining on Esmeralda testnet."
  echo "Target: ${TARGET_BLOCKS} blocks to generate funding."
  echo ""
  echo "Next steps:"
  echo "  1. Wait for ${TARGET_BLOCKS} blocks (check with: $0 --status)"
  echo "  2. Fund your benchmark wallets from the mining rewards"
  echo "  3. Run the harness: cd ${PROJECT_DIR} && cargo run"
  echo ""
  get_wallet_addresses
}

main "$@"
