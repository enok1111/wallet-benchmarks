# Wallet Benchmarks: Funding & Environment Setup Report

## Status: Code Complete, Blocked on Testnet tXTM Funds

### What's Done

**PR #3** (`feat: complete wallet benchmark harness`) is open, compiles cleanly, and passes 58 tests. All 9 scenarios (B0-S7) across 3 wallet modes are implemented. The one remaining requirement before the baseline run is testnet funding.

### Architecture (Updated May 27)

Each mode now uses its intended interface:

| Mode | Interface | Key Operations |
|------|-----------|----------------|
| Old Wallet | `minotari_console_wallet` subprocess + gRPC | Full gRPC lifecycle (spawn, scan, send, teardown) |
| New Wallet | `minotari` library (pure Rust, NO gRPC) | Scanner, `get_balance()`, `sign_locked_transaction`, HTTP RPC broadcast |
| Payment Processor | `minotari` library + HTTP RPC | Same as New Wallet, with batch 1-to-many support |

### Built Assets (Working)

| Binary | Location | Network | Version |
|--------|----------|---------|---------|
| `minotari_node` (Esmeralda) | `tari/target/release/minotari_node` | Esmeralda testnet | v5.3.1 (built from tag) |
| `minotari_console_wallet` (Esmeralda) | `tari/target/release/minotari_console_wallet` | Esmeralda testnet | v5.3.1 |
| `wallet-benchmarks` | `wallet-benchmarks/target/release/wallet-benchmarks` | — | PR #3 |

### Funding Options Investigated

#### 1. Local Base Node Connecting to Public Esmeralda Testnet ❌

We built `minotari_node` from the `v5.3.1` tag (matching the running testnet version). The node starts but **cannot establish RPC connections to seed peers**. The error is consistent across macOS-native and Docker (Linux arm64) environments:

```
SeedStrap: Failed to connect RPC client to seed peer <id>: Peer connection error: channel closed
```

TCP connectivity to seed peers is confirmed working (`nc -zv` succeeds). The failure is at the RPC handshake layer. The DNS seed resolution has a secondary issue — the first DNS TXT record returns a URL that the code misparses as a peer address.

**Attempted fixes that didn't help:**
- `--network esmeralda` runtime flag
- `-p base_node.p2p.transport.type=tcp` (TCP transport instead of Tor)
- `-p base_node.p2p.transport.tcp.listener_address=...` (explicit bind)
- Docker container with arm64v8/ubuntu:22.04
- Default Tor transport (libtor bundled)

#### 2. Docker with Official Linux arm64 Binary ❌

We downloaded `tari_suite-5.3.1-esme-bc0e4f2-linux-arm64.zip` from the official release, built a Docker image, and ran it. Same "channel closed" RPC error. The issue is not macOS-specific.

#### 3. Public HTTP RPC Endpoint ✅ (Partially Works)

The public Esmeralda RPC at `https://rpc.esmeralda.tari.com` is operational:
- `/health` -> `{"status":"healthy"}`
- `/get_tip_info` -> returns chain metadata (tip height: ~655,710)

**Update:** NewWalletMode can use this HTTP RPC endpoint directly via `reqwest` for broadcasting transactions, since it no longer depends on gRPC. Only OldWalletMode still requires a local base node with gRPC.

#### 4. `test-faucet` Tool ❌

The `tari-project/test-faucet` repo generates 4,000 x 10 T UTXOs for a custom genesis block on a **local devnet**. However, the code depends on `tari_core::transactions` module paths that were refactored out in v5.3.1 (moved to `tari_transaction_components`). The tool is incompatible with the current code structure and would require significant adaptation.

#### 5. Tari Universe Mainnet Binary ❌

Tari Universe (v5.2.1) macOS binary at `~/Library/Caches/com.tari.universe/binaries/tari/mainnet/5.2.1/minotari_node` refuses `--network esmeralda`:
```
Failed to set the network: The network esmeralda is invalid for this binary built for MainNet
```

### Funding Requirements

| Per Mode | x3 Modes | Total |
|----------|----------|-------|
| 10,000 tXTM | 30,000 tXTM | Block reward on Esmeralda is ~14,000 tXTM |

### What We've Learned

- The Tari RPC handshake fails with "channel closed" on both macOS and Linux (Docker)
- DNS seed records at `seeds.esmeralda.tari.com` work correctly (return both peer addresses and a CDN URL)
- The CDN URL (`https://cdn-universe.tari.com/.../seednodes.json`) returns valid peer data but is misparsed
- The test-faucet tool is structurally incompatible with v5.3.1
- A public HTTP RPC is available (`https://rpc.esmeralda.tari.com`) and NewWalletMode can use it directly
- No macOS binaries are published in Tari GitHub releases (only Linux/Windows)
- Tari Universe provides macOS binaries but only for mainnet

### Recommended Next Steps

1. **Ask for tXTM in Tari Discord** — The fastest path. The user joined the Tari Discord and completed onboarding. Asking in `#general` or `#new-user-chat` for ~30,000 tXTM for wallet-benchmarks bounty work. Esmeralda testnet funds have no real value and community members are typically willing to share.

2. **PR review first** — Request a maintainer review of PR #3 as-is (without baseline results). The harness logic is complete, 58 tests pass, and the code compiles cleanly. The baseline JSON can be produced once access to funded wallets is available.

3. **Alternative: Use the `minotari` CLI for manual funding** — If a community member provides tXTM:
   - Create 3 wallets (old, new, payment_processor) using predetermined seed words
   - Send 10,000 tXTM to each
   - Run the harness with `shared_seed_words` configured in `config.toml`

4. **If all else fails: Patch the RPC issue** — The "channel closed" error in the comms layer could be further debugged by enabling `RUST_LOG=debug` on the node and inspecting the full handshake trace. This is a Tari core protocol debugging exercise.
