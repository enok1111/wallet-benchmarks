# Wallet Benchmarks - Progress Tracker

## Bounty Status: IN PROGRESS (L-tier, 150,000 XTM)
**Issue**: https://github.com/tari-project/wallet-benchmarks/issues/1
**Branch**: `bounty/issue-1-wallet-benchmark-harness`

---

## Architecture Overview

```
wallet-benchmarks/
├── src/
│   ├── main.rs              # CLI entry point, orchestrator
│   ├── config.rs            # TOML config with all bounty parameters
│   ├── metrics.rs           # Metrics collection, result profile generation
│   ├── modes/               # Wallet mode implementations
│   │   ├── mod.rs           # WalletMode trait definition
│   │   ├── old_wallet.rs    # minotari_console_wallet via gRPC (subprocess)
│   │   ├── new_wallet.rs    # minotari crate library integration
│   │   └── payment_processor.rs  # Batch 1-to-many transactions
│   └── scenarios/           # Scenario execution orchestrator
│       └── mod.rs           # Run all scenarios per mode
├── config.example.toml      # Reference configuration
├── Cargo.toml               # Dependencies + Tari crate bindings
├── README.md                # Project documentation
└── PROGRESS.md              # This file
```

## Implementation Plan

### Phase 1: Foundation ✅ (DONE)
- [x] Project structure and Cargo.toml with all dependencies
- [x] Configuration system (config.rs) - all bounty parameters exposed
- [x] Metrics collection framework (metrics.rs) - result profile generation
- [x] WalletMode trait with async_trait
- [x] Three wallet mode skeletons (old, new, payment_processor)
- [x] Scenario orchestrator skeleton
- [x] Example config file
- [x] README.md
- [x] Compiles cleanly (`cargo check` passes)

### Phase 2: Core Scenario Logic 🔄 (IN PROGRESS)
- [ ] B0 - Baseline Scan (empty wallet, floor cost of block-walk + view-key check)
- [ ] S0 - Funding Baseline (init wallet, receive funding UTXO, wait for C_min confirmation)
- [ ] S1 - UTXO Build-up (doubling rounds + fan-out → 512 UTXOs)
- [ ] S2 - Scan from Genesis (checkpoint 1, post-S1 state)
- [ ] S3 - Scan from Birthday (checkpoint 1, wallet birthday = genesis height + 1)
- [ ] S4 - Concurrent Construction (ramp: 8,16,32,64,128 concurrent txs)
- [ ] S5 - Payment Processor Throughput (batch arm vs individual arm)
- [ ] S6 - Scan from Genesis (checkpoint 2, post-S5 state)
- [ ] S7 - Scan from Birthday (checkpoint 2, wallet birthday = genesis height + 1)

### Phase 3: Wallet Mode Integration
- [ ] Old wallet: tonic gRPC client with minotari_app_grpc protos
- [ ] New wallet: minotari crate Scanner + TransactionSender integration
- [ ] Payment processor: batch transaction builder

### Phase 4: Testing & Validation
- [ ] Unit tests for config parsing and validation
- [ ] Mock base node for local testing
- [ ] Result profile JSON schema validation

### Phase 5: Draft PR & Polish
- [ ] Open draft PR on tari-project/wallet-benchmarks
- [ ] Troubleshooting guide in README
- [ ] CI configuration (GitHub Actions)

---

## API Research Notes

### Old Wallet (minotari_console_wallet) - gRPC Interface
**Proto**: `minotari_app_grpc` crate (v5.3.1-pre.0)
**Key services/methods**:
- `WalletGrpcService::GetBalance` → `GetBalanceResponse { balance, locked_balance }`
- `WalletGrpcService::Transfer` → `TransferRequest { destination, amount, fee_per_gram }`
- `WalletGrpcService::GetTransactions` → filter by status
- `WalletGrpcService::GetAddress` → wallet address
- `WalletGrpcService::GetTipHeight` → chain tip height
- `WalletGrpcService::ScanBlockchain` → trigger scan

**Startup command**:
```bash
minotari_console_wallet --base-path /tmp/wallet_old \
  --network esmeralda \
  --grpc-enabled --grpc-address 127.0.0.1:18200 \
  -n  # non-interactive
```

### New Wallet (minotari crate) - Library Integration
**Scanner API**:
```rust
use minotari::wallet2::scanner::Scanner;
let scanner = Scanner::new(
    password,
    base_url,           // "http://127.0.0.1:18142"
    db_path,            // SQLite wallet database path
    batch_size,         // blocks per HTTP request
)
.mode(ScanMode::Full)  // or ScanMode::Birthday(height)
.run();
```

**Transaction flow**:
```rust
use minotari::wallet2::transaction_builder::TransactionSender;
let sender = TransactionSender::new(db_conn, account_id);
// Build unsigned transaction
let unsigned_tx = sender.build_unsigned_transaction(
    recipients,         // Vec<(address, amount)>
    fee_per_gram,
)?;
// Sign and broadcast
let signed_tx = sender.sign_and_broadcast(unsigned_tx)?;
```

**Balance query**:
```rust
use minotari::wallet2::db::get_balance;
let balance = get_balance(db_conn, account_id)?;
```

### Payment Processor - Batch Transactions
Uses same minotari crate but with multiple recipients per transaction:
```rust
let recipients = vec![
    (recipient_addr_1, amount_1),
    (recipient_addr_2, amount_2),
    // ... up to K outputs per batch tx
];
let batch_tx = sender.build_unsigned_transaction(recipients, fee_per_gram)?;
```

### Base Node HTTP RPC API (Esmeralda testnet)
- `POST /v1/transactions` - Submit transaction
- `GET /v1/blocks/{height}` - Get block by height
- `GET /v1/tip_height` - Current chain tip
- `GET /v1/block_outputs/{height}` - Block outputs for scanning

---

## Computed Deltas (Required by Bounty)

| Delta | Formula | Purpose |
|-------|---------|---------|
| `T_scan_genesis_B0` | `S2.genesis_scan_time - B0.scan_time` | Overhead from UTXO storage |
| `T_scan_birthday_B0` | `S3.birthday_scan_time - B0.scan_time` | Birthday scan overhead |
| `T_scan_genesis_delta` | `S6.genesis_scan_time - S2.genesis_scan_time` | Growth impact (checkpoint 1→2) |
| `T_scan_birthday_delta` | `S7.birthday_scan_time - S3.birthday_scan_time` | Growth impact (checkpoint 1→2) |
| `throughput_multiplier` | `S5.individual_total_time / S5.batch_total_time` | Batch efficiency gain |
| `fee_per_recipient_batch` | `S5.batch_fees / S5_m` | Batch fee efficiency |
| `fee_per_recipient_individual` | `S5.individual_fees / S5_m` | Individual fee baseline |

---

## Commits

| Hash | Date | Message |
|------|------|---------|
| 96b2b1d | 2025-07-14 | feat: initial wallet benchmark harness architecture |

---

## Next Steps (Current Sprint)

1. **Implement B0 baseline scan** - foundation for all scan scenarios
2. **Implement S0 funding baseline** - wallet init + receive UTXO + wait for confirmation
3. **Build old wallet gRPC client** - tonic connection to minotari_console_wallet
4. **Build new wallet library integration** - Scanner + TransactionSender wrappers
5. **Commit and push draft PR** - get early feedback from Tari team
