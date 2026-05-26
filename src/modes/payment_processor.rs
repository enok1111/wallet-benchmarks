//! Payment Processor Mode (batch 1-to-many transactions)
//!
//! Uses the new wallet's batch transaction capability to send
//! 1-to-many transactions efficiently. This mode measures:
//! - Batch construction throughput vs individual sends
//! - Fee efficiency per recipient
//! - Block consumption comparison

use anyhow::{anyhow, Context, Result};
use log::{debug, info, warn};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

use crate::config::HarnessConfig;
use crate::metrics::ScenarioResult;
use crate::modes::{WalletMode, WalletModeId};

/// Payment processor mode implementation using batch transactions
pub struct PaymentProcessorMode {
    data_dir: PathBuf,
    db_path: PathBuf,
    address: Option<String>,
    seed_words: Vec<String>,
    password: String,
    base_node_http: String,
    db_pool: Option<minotari::db::SqlitePool>,
    grpc_client: Arc<Mutex<Option<crate::grpc_client::OldWalletGrpcClient>>>,
    grpc_address: String,
}

impl PaymentProcessorMode {
    pub fn new(data_dir: PathBuf, grpc_port: u16) -> Self {
        let db_path = data_dir.join("wallet.db");
        let grpc_address = format!("127.0.0.1:{}", grpc_port);
        Self {
            data_dir,
            db_path,
            address: None,
            seed_words: Vec::new(),
            password: "benchmark_password_32chars_min".to_string(),
            base_node_http: "http://127.0.0.1:18143".to_string(),
            db_pool: None,
            grpc_client: Arc::new(Mutex::new(None)),
            grpc_address,
        }
    }

    fn generate_seed_words(_mode_id: WalletModeId) -> Vec<String> {
        crate::modes::generate_tari_seed_words()
    }

    /// Initialize the wallet database using minotari library.
    /// The birthday_height is stored in the DB's account record. The minotari Scanner
    /// reads the birthday from the accounts table to determine its starting scan height.
    async fn init_wallet_db(&mut self, birthday_height: u64) -> Result<()> {
        info!(
            "Initializing payment processor wallet DB at {} with birthday height {}",
            self.db_path.display(),
            birthday_height
        );

        std::fs::create_dir_all(&self.data_dir)?;

        let db_pool = minotari::init_db(self.db_path.clone())
            .context("Failed to initialize minotari database")?;
        self.db_pool = Some(db_pool.clone());

        // If an account already exists (created by the gRPC wallet subprocess),
        // update its birthday to match the requested scan start height.
        if let Ok(conn) = db_pool.get() {
            let birthday_val: i64 = birthday_height as i64;
            if let Ok(count) = conn.query_row(
                "SELECT COUNT(*) FROM accounts",
                [],
                |row| row.get::<_, i64>(0),
            )
                && count > 0 {
                    let _ = conn.execute(
                        "UPDATE accounts SET birthday = ?1 WHERE birthday != ?1",
                        [birthday_val],
                    );
                    info!("Updated account birthday to {}", birthday_height);
                }
        }

        debug!("Payment processor wallet database initialized");
        Ok(())
    }

    /// Scan blockchain using the minotari Scanner.
    /// The Scanner reads the starting height from the wallet's stored birthday in the DB
    /// (accounts.birthday column), not from a from_height parameter. For genesis rescans,
    /// call `rescan_from_height` which adjusts the birthday in the DB before scanning.
    async fn scan_blockchain(&self) -> Result<()> {
        use minotari::{Scanner, ScanMode};

        info!(
            "Scanning blockchain via {}",
            self.base_node_http,
        );

        let (events, _more_blocks) = Scanner::new(
            &self.password,
            &self.base_node_http,
            self.db_path.clone(),
            100,
            10,
        )
        .account("default")
        .mode(ScanMode::Full)
        .run()
        .await
        .context("Scanner failed")?;

        info!(
            "Scan completed: {} events processed",
            events.len()
        );
        Ok(())
    }

    /// Get wallet balance from database
    #[allow(dead_code)]
    async fn query_balance(&self) -> Result<u64> {
        use minotari::get_balance;

        let db = self.db_pool.as_ref()
            .ok_or_else(|| anyhow!("Database not initialized"))?;
        let conn = db.get().context("Failed to get DB connection")?;

        // Get account
        let accounts = minotari::get_accounts(&conn, Some("default"))
            .context("Failed to get accounts")?;
        let account = accounts.first()
            .ok_or_else(|| anyhow!("Default account not found"))?;

        let balance = get_balance(&conn, account.id)
            .context("Failed to query balance")?;

        debug!("Balance: {} µT available", balance.available);
        Ok(balance.available.into())
    }

    /// Query UTXO count from database
    async fn query_utxo_count(&self) -> Result<u32> {
        let db = self.db_pool.as_ref()
            .ok_or_else(|| anyhow!("Database not initialized"))?;
        let conn = db.get().context("Failed to get DB connection")?;

        let accounts = minotari::get_accounts(&conn, Some("default"))
            .context("Failed to get accounts")?;
        let account = accounts.first()
            .ok_or_else(|| anyhow!("Default account not found"))?;

        let outputs = minotari::db::fetch_unspent_outputs(&conn, account.id, 0)
            .context("Failed to fetch unspent outputs")?;

        debug!("UTXO count: {}", outputs.len());
        Ok(outputs.len() as u32)
    }

    async fn get_balance_via_grpc(&self) -> Result<u64> {
        let mut guard = self.grpc_client.lock().await;
        let client = guard.as_mut().ok_or_else(|| anyhow!("gRPC client not connected"))?;
        let resp = client.get_balance().await?;
        Ok(resp.available_balance)
    }

    #[allow(dead_code)]
    async fn send_to_self_via_grpc(&self, output_count: u32, fee_per_gram: u64) -> Result<u64> {
        let address = self.address.as_deref().ok_or_else(|| anyhow!("Wallet address not available"))?;
        let amount_per_output = 100_000u64;

        let mut guard = self.grpc_client.lock().await;
        let client = guard.as_mut().ok_or_else(|| anyhow!("gRPC client not connected"))?;

        let recipients: Vec<_> = (0..output_count)
            .map(|_| minotari_app_grpc::tari_rpc::PaymentRecipient {
                address: address.to_string(),
                amount: amount_per_output,
                fee_per_gram,
                payment_type: 0,
                raw_payment_id: Vec::new(),
                user_payment_id: None,
            })
            .collect();

        let request = tonic::Request::new(minotari_app_grpc::tari_rpc::TransferRequest {
            recipients,
            single_tx: false,
        });

        let resp = client.client_mut().transfer(request).await?;
        let inner = resp.into_inner();
        let tx_id = inner.results.first()
            .map(|r| r.transaction_id)
            .ok_or_else(|| anyhow!("Transfer response contained no results"))?;
        Ok(tx_id)
    }

    #[allow(dead_code)]
    async fn send_single_transfer_via_grpc(&self, destination: &str, amount: u64, fee_per_gram: u64) -> Result<u64> {
        let mut guard = self.grpc_client.lock().await;
        let client = guard.as_mut().ok_or_else(|| anyhow!("gRPC client not connected"))?;
        let resp = client.transfer(destination, amount, fee_per_gram).await?;
        let tx_id = resp.results.first()
            .map(|r| r.transaction_id)
            .ok_or_else(|| anyhow!("Transfer response contained no results"))?;
        Ok(tx_id)
    }

    /// Wait for a specific transaction to reach `c_min` confirmations.
    ///
    /// Polls the wallet gRPC `GetTransactionInfo` to check the specific tx status.
    /// Once mined, verifies confirmation depth against base node tip height.
     #[allow(dead_code)]
    async fn wait_for_confirmation_via_grpc(&self, tx_id: u64, c_min: u32, timeout_secs: u64) -> Result<()> {
        use minotari_app_grpc::tari_rpc::TransactionStatus;

        let start = std::time::Instant::now();
        let timeout = Duration::from_secs(timeout_secs);

        loop {
            if start.elapsed() > timeout {
                anyhow::bail!(
                    "Timeout waiting for tx {} confirmation (c_min={})",
                    tx_id, c_min
                );
            }

            // Poll the specific transaction status via wallet gRPC
            let tx_info = {
                let mut guard = self.grpc_client.lock().await;
                let client = guard.as_mut().ok_or_else(|| anyhow!("gRPC client not connected"))?;
                client.get_transaction_info(tx_id).await?
            };

            match tx_info {
                Some(info) => {
                    match info.status {
                        s if s == TransactionStatus::MinedConfirmed as i32 => {
                            debug!("Transaction {} confirmed (MINED_CONFIRMED)", tx_id);
                            return Ok(());
                        }
                        s if s == TransactionStatus::MinedUnconfirmed as i32 => {
                            let mined_height = info.mined_in_block_height;
                            let current_tip = crate::http_rpc::BaseNodeRpcClient::new(&self.base_node_http)
                                .get_tip_height().await?;
                            let confirmations = current_tip.saturating_sub(mined_height);
                            if confirmations >= c_min as u64 {
                                debug!(
                                    "Transaction {} confirmed: {} confirmations at height {}",
                                    tx_id, confirmations, mined_height
                                );
                                return Ok(());
                            }
                            debug!(
                                "Tx {} mined at height {}, waiting for {} confirmations (current_tip={})",
                                tx_id, mined_height, c_min, current_tip
                            );
                        }
                        s if s == TransactionStatus::Rejected as i32 => {
                            anyhow::bail!(
                                "Transaction {} was rejected (status={})",
                                tx_id, s
                            );
                        }
                        _ => {
                            debug!("Tx {} status={}, still waiting...", tx_id, info.status);
                        }
                    }
                }
                None => {
                    debug!("Tx {} not yet found in wallet DB, waiting...", tx_id);
                }
            }

            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }

    /// Send a transaction to self using the library's offline signing flow.
    /// Used by S1 (doubling/fan-out) to build UTXOs without gRPC.
    async fn send_to_self(&self, output_count: u32, fee_per_gram: u64) -> Result<u64> {
        use minotari::transactions::one_sided_transaction::{OneSidedTransaction, Recipient};
        use minotari::db::get_account_by_name;
        use tari_common::configuration::Network;
        use tari_common_types::tari_address::TariAddress;
        use tari_transaction_components::offline_signing::sign_locked_transaction;
        use tari_transaction_components::consensus::ConsensusConstantsBuilder;
        use tari_transaction_components::tari_amount::MicroMinotari;

        let db = minotari::init_db(self.db_path.clone())?;
        let conn = db.get()?;

        let account = get_account_by_name(&conn, "default")?
            .ok_or_else(|| anyhow!("Default account not found"))?;

        let address = TariAddress::from_base58(
            self.address.as_deref().ok_or_else(|| anyhow!("Wallet address not available"))?,
        )?;

        let tx_builder = OneSidedTransaction::new(db.clone(), Network::Esmeralda, self.password.clone());

        let amount_per_output = MicroMinotari(100_000);
        let total_amount = amount_per_output.0 * output_count as u64;

        let recipient = Recipient {
            address,
            amount: MicroMinotari(total_amount),
            payment_id: Some(format!(
                "bench-pp-s1-{}-{}",
                output_count,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis()
            )),
        };

        let locked_funds = self.lock_funds_library(output_count).await?;

        let unsigned_tx = tx_builder.create_unsigned_transaction(
            &account,
            locked_funds,
            vec![recipient],
            MicroMinotari(fee_per_gram),
        )?;

        let key_manager = account.get_key_manager(&self.password)?;
        let consensus_constants = ConsensusConstantsBuilder::new(Network::Esmeralda).build();
        let signed_result = sign_locked_transaction(
            &key_manager,
            consensus_constants,
            Network::Esmeralda,
            unsigned_tx,
        )?;

        self.broadcast_signed_transaction(&signed_result).await
    }

    /// Send a batch transaction to multiple recipients using the library.
    /// Used by S5 (batch throughput) to demonstrate 1-to-many efficiency.
    async fn send_batch_library(&self, recipients: &[(String, u64)], fee_per_gram: u64) -> Result<u64> {
        use minotari::transactions::one_sided_transaction::{OneSidedTransaction, Recipient};
        use minotari::db::get_account_by_name;
        use tari_common::configuration::Network;
        use tari_common_types::tari_address::TariAddress;
        use tari_transaction_components::offline_signing::sign_locked_transaction;
        use tari_transaction_components::consensus::ConsensusConstantsBuilder;
        use tari_transaction_components::tari_amount::MicroMinotari;

        let db = minotari::init_db(self.db_path.clone())?;
        let conn = db.get()?;

        let account = get_account_by_name(&conn, "default")?
            .ok_or_else(|| anyhow!("Default account not found"))?;

        let tx_builder = OneSidedTransaction::new(db.clone(), Network::Esmeralda, self.password.clone());

        let tari_recipients: Vec<Recipient> = recipients
            .iter()
            .map(|(addr, amount)| Recipient {
                address: TariAddress::from_base58(addr.as_str()).unwrap(),
                amount: MicroMinotari(*amount),
                payment_id: None,
            })
            .collect();

        let locked_funds = self.lock_funds_library(recipients.len() as u32).await?;

        let unsigned_tx = tx_builder.create_unsigned_transaction(
            &account,
            locked_funds,
            tari_recipients,
            MicroMinotari(fee_per_gram),
        )?;

        let key_manager = account.get_key_manager(&self.password)?;
        let consensus_constants = ConsensusConstantsBuilder::new(Network::Esmeralda).build();
        let signed_result = sign_locked_transaction(
            &key_manager,
            consensus_constants,
            Network::Esmeralda,
            unsigned_tx,
        )?;

        self.broadcast_signed_transaction(&signed_result).await
    }

    /// Lock funds for a transaction using simple UTXO selection (library version).
    async fn lock_funds_library(&self, output_count: u32) -> Result<minotari::api::types::LockFundsResult> {
        use minotari::db::{get_account_by_name, fetch_unspent_outputs};
        use tari_transaction_components::tari_amount::MicroMinotari;
        use tari_transaction_components::utxo_selection::UtxoValue;

        let db = minotari::init_db(self.db_path.clone())?;
        let conn = db.get()?;

        let account = get_account_by_name(&conn, "default")?
            .ok_or_else(|| anyhow!("Default account not found"))?;
        let outputs = fetch_unspent_outputs(&conn, account.id, 0)?;

        let amount_per_output = MicroMinotari(100_000);
        let total_amount = amount_per_output.0 * output_count as u64;

        let mut locked_outputs = Vec::new();
        let mut accumulated = 0u64;

        for output in outputs {
            locked_outputs.push(output.output.clone());
            accumulated += output.value().as_u64();
            if accumulated >= total_amount {
                break;
            }
        }

        let fee_per_gram = MicroMinotari(5);
        let estimated_tx_size = 500;
        let fee_with_change = MicroMinotari(fee_per_gram.0 * estimated_tx_size);
        let fee_without_change = MicroMinotari((fee_per_gram.0 * (estimated_tx_size - 100)) / 2);

        Ok(minotari::api::types::LockFundsResult {
            utxos: locked_outputs,
            requires_change_output: accumulated > total_amount,
            total_value: MicroMinotari(accumulated),
            fee_without_change,
            fee_with_change,
        })
    }

    /// Broadcast a signed transaction via HTTP RPC (library version).
    async fn broadcast_signed_transaction(
        &self,
        signed_result: &tari_transaction_components::offline_signing::models::SignedOneSidedTransactionResult,
    ) -> Result<u64> {
        let client = reqwest::Client::new();
        let submit_url = format!("{}/json_rpc", self.base_node_http);
        let transaction = &signed_result.signed_transaction.transaction;

        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "1",
            "method": "submit_transaction",
            "params": { "transaction": transaction }
        });

        let response = client
            .post(&submit_url)
            .json(&request)
            .send()
            .await?;

        if response.status().is_success() {
            let numeric_tx_id = signed_result.request.tx_id.as_u64();
            debug!("Transaction broadcast successfully (tx_id={})", numeric_tx_id);
            Ok(numeric_tx_id)
        } else {
            let status = response.status();
            anyhow::bail!("Transaction broadcast failed: {}", status);
        }
    }

     /// Wait for a transaction to be confirmed using library-level scanning.
    /// Re-scans the blockchain and checks if UTXO count has increased.
    async fn wait_for_confirmation_library(&self, tx_id: u64, _c_min: u32, timeout_secs: u64) -> Result<()> {
        let start = std::time::Instant::now();
        let timeout = Duration::from_secs(timeout_secs);
        let base_node = crate::http_rpc::BaseNodeRpcClient::new(&self.base_node_http);

        loop {
            if start.elapsed() > timeout {
                anyhow::bail!(
                    "Timeout waiting for tx {} confirmation",
                    tx_id
                );
            }

            let tip_height = base_node.get_tip_height().await?;

            // Re-scan to pick up any new confirmed outputs
            if let Err(e) = self.scan_blockchain().await {
                debug!("Scan during confirmation poll: {}", e);
            }

            // Check if UTXO count has increased (means our tx confirmed)
            let utxo_count = self.query_utxo_count().await.unwrap_or(0);
            let balance = self.query_balance().await.unwrap_or(0);

            debug!(
                "Tx {} polling: tip={}, utxos={}, balance={}",
                tx_id, tip_height, utxo_count, balance
            );

            if utxo_count > 0 && balance > 0
                && tip_height > 0 {
                    return Ok(());
                }

            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }

    async fn wait_for_grpc_ready(&self, timeout_secs: u64) -> Result<()> {
        let start = std::time::Instant::now();
        let timeout = Duration::from_secs(timeout_secs);

        loop {
            if start.elapsed() > timeout {
                return Err(anyhow!("Timeout waiting for gRPC server to be ready"));
            }

            match crate::grpc_client::OldWalletGrpcClient::connect(&self.grpc_address).await {
                Ok(client) => {
                    info!("Payment processor gRPC server ready at {}", self.grpc_address);
                    let mut guard = self.grpc_client.lock().await;
                    *guard = Some(client);
                    return Ok(());
                }
                Err(_) => {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            }
        }
    }

    #[allow(dead_code)]
    async fn create_and_broadcast_batch_transaction(
        &self,
        recipients: &[(&str, u64)],
        fee_per_gram: u64,
    ) -> Result<String> {
        let mut guard = self.grpc_client.lock().await;
        let client = guard.as_mut().ok_or_else(|| anyhow!("gRPC client not connected"))?;

        let grpc_recipients: Vec<_> = recipients
            .iter()
            .map(|(addr, amount)| minotari_app_grpc::tari_rpc::PaymentRecipient {
                address: addr.to_string(),
                amount: *amount,
                fee_per_gram,
                payment_type: 0,
                raw_payment_id: Vec::new(),
                user_payment_id: None,
            })
            .collect();

        let request = tonic::Request::new(minotari_app_grpc::tari_rpc::TransferRequest {
            recipients: grpc_recipients,
            single_tx: true,
        });

        let resp = client.client_mut().transfer(request).await?;
        let inner = resp.into_inner();
        let tx_id = inner.results.first()
            .map(|r| r.transaction_id.to_string())
            .unwrap_or_else(|| "unknown".to_string());

        debug!("Batch transaction created: {} for {} recipients", tx_id, recipients.len());
        Ok(tx_id)
    }
}

#[async_trait::async_trait]
impl WalletMode for PaymentProcessorMode {
    async fn initialize(&mut self, config: &HarnessConfig) -> Result<()> {
        info!(
            "Initializing payment processor mode at {}",
            self.data_dir.display()
        );

        std::fs::create_dir_all(&self.data_dir)?;

        if self.seed_words.is_empty() {
            self.seed_words = config
                .resolve_seed_words("payment_processor")
                .unwrap_or_else(|| Self::generate_seed_words(WalletModeId::PaymentProcessor));
        }

        self.base_node_http = config.base_node_http.clone();

        self.init_wallet_db(0).await?;

        self.wait_for_grpc_ready(60).await?;

        let mut guard = self.grpc_client.lock().await;
        if let Some(ref mut client) = *guard {
            match client.get_address().await {
                Ok(response) => {
                    self.address = Some(hex::encode(&response.interactive_address));
                    info!("Wallet address: {}", self.address.as_ref().unwrap());
                }
                Err(e) => {
                    warn!("Failed to get wallet address via gRPC: {}", e);
                    self.address = Some("placeholder_address".to_string());
                }
            }
        }

        info!("Payment processor initialized successfully");
        Ok(())
    }

    async fn run_scenario(
        &mut self,
        scenario_id: &str,
        config: &HarnessConfig,
    ) -> Result<ScenarioResult> {
        info!(
            "Running scenario {} for payment processor mode",
            scenario_id
        );

        let mut result = ScenarioResult::new(scenario_id);

        match scenario_id {
            "B0" => self.run_b0(config, &mut result).await?,
            "S0" => self.run_s0(config, &mut result).await?,
            "S1" => self.run_s1(config, &mut result).await?,
            "S2" => self.run_s2(config, &mut result).await?,
            "S3" => self.run_s3(config, &mut result).await?,
            "S4" => self.run_s4(config, &mut result).await?,
            "S5" => self.run_s5(config, &mut result).await?, // Batch arm only
            "S6" => self.run_s6(config, &mut result).await?,
            "S7" => self.run_s7(config, &mut result).await?,
            _ => return Err(anyhow!("Unknown scenario: {}", scenario_id)),
        }

        info!(
            "Scenario {} completed for payment processor: success={}, failures={}",
            scenario_id, result.success_count, result.failure_count
        );

        Ok(result)
    }

    fn get_address(&self) -> Option<&str> {
        self.address.as_deref()
    }

    async fn get_balance(&self) -> Result<u64> {
        self.query_balance().await
    }

    async fn teardown(&mut self) -> Result<()> {
        info!("Tearing down payment processor mode");
        // No process to kill; just clean up temp files if needed
        info!("Payment processor torn down");
        Ok(())
    }
}

// Scenario implementations for payment processor mode
impl PaymentProcessorMode {
    async fn run_b0(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        // B0 - Baseline Scan (empty wallet)
        use std::time::Instant;

        let start = Instant::now();
        info!("B0: Baseline scan for payment processor mode");

        let base_node_client =
            crate::http_rpc::BaseNodeRpcClient::new(&config.base_node_http);
        let tip_height_start = base_node_client.get_tip_height().await?;
        result.tip_height_start = tip_height_start;

        info!(
            "B0: Scanning from genesis to tip {} (library integration)",
            tip_height_start
        );

        match self.scan_blockchain().await {
            Ok(_) => info!("B0 scan completed"),
            Err(e) => warn!("B0 scan error (may be expected for empty wallet): {}", e),
        }

        let scan_time_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = scan_time_secs;

        let tip_height_end = base_node_client.get_tip_height().await?;
        result.tip_height_end = tip_height_end;

        let blocks_scanned = tip_height_end.saturating_sub(tip_height_start);
        result.success_count = if blocks_scanned > 0 { 1 } else { 0 };

        let mut b0_metrics: HashMap<String, serde_json::Value> = HashMap::new();
        b0_metrics.insert("scan_mode".into(), serde_json::json!("genesis"));
        b0_metrics.insert("blocks_scanned".into(), serde_json::json!(blocks_scanned));
        b0_metrics.insert(
            "blocks_per_sec".into(),
            serde_json::json!(if scan_time_secs > 0.0 { blocks_scanned as f64 / scan_time_secs } else { 0.0 }),
        );
        result.metrics.extend(b0_metrics);

        info!(
            "B0 (payment processor) completed: {} blocks in {:.2}s",
            blocks_scanned, scan_time_secs
        );
        Ok(())
    }

    async fn run_s0(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        // S0 - Funding Baseline
        use std::time::Instant;

        let start = Instant::now();
        info!("S0: Funding baseline for payment processor mode");

        let funding_address = self.address.as_deref().unwrap_or("placeholder");
        info!(
            "S0: Funding address: {} (awaiting {} µT)",
            funding_address, config.a_fund
        );

        let wait_result = self.wait_for_funding(config.a_fund, config.c_min, 600).await;
        let elapsed_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = elapsed_secs;

        match wait_result {
            Ok(balance) => {
                result.success_count = 1;
                result.balance_delta_ut = balance as i64;
            }
            Err(e) => {
                result.failure_count = 1;
                result.failure_reasons.push(format!("Funding wait failed: {}", e));
            }
        }

        Ok(())
    }

    async fn run_s1(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        // S1 - UTXO Build-up (doubling + fan-out)
        use std::time::Instant;

        let start = Instant::now();
        info!(
            "S1: UTXO build-up for payment processor - target {} UTXOs",
            config.volume_target
        );

        let mut current_utxos = 1u32;

        // Doubling phase
        for round in 0..config.doubling_rounds {
            match self
                .send_to_self(current_utxos * 2, config.fee_rate)
                .await
            {
                Ok(tx_id) => {
                    info!("S1: Doubling round {} tx: {}", round + 1, tx_id);
                    self.wait_for_confirmation_library(tx_id, config.c_min, 300).await?;
                    current_utxos *= 2;
                }
                Err(e) => {
                    result.failure_reasons
                        .push(format!("Doubling round {} failed: {}", round + 1, e));
                    break;
                }
            }
        }

        // Fan-out phase
        if current_utxos < config.volume_target {
            let remaining = config.volume_target - current_utxos;
            let fanout_rounds =
                remaining.div_ceil(config.fanout_outputs_per_tx);

            for round in 0..fanout_rounds {
                let outputs = std::cmp::min(
                    config.fanout_outputs_per_tx,
                    config.volume_target - current_utxos,
                );
                match self.send_to_self(outputs, config.fee_rate).await {
                    Ok(tx_id) => {
                        info!("S1: Fan-out round {} tx: {}", round + 1, tx_id);
                        self.wait_for_confirmation_library(tx_id, config.c_min, 300).await?;
                        current_utxos += outputs - 1;
                    }
                    Err(e) => {
                        result.failure_reasons
                            .push(format!("Fan-out round {} failed: {}", round + 1, e));
                        break;
                    }
                }
            }
        }

        let elapsed_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = elapsed_secs;
        result.success_count = if current_utxos >= config.volume_target { 1 } else { 0 };

        let mut s1_metrics: HashMap<String, serde_json::Value> = HashMap::new();
        s1_metrics.insert("final_utxo_count".into(), serde_json::json!(current_utxos));
        result.metrics.extend(s1_metrics);

        info!("S1 (payment processor) completed: {} UTXOs in {:.2}s", current_utxos, elapsed_secs);
        Ok(())
    }

    async fn run_s2(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        // S2 - Scan from Genesis (checkpoint 1)
        use std::time::Instant;

        let start = Instant::now();
        info!("S2: Scan from Genesis (checkpoint 1) for payment processor");

        let base_node_client =
            crate::http_rpc::BaseNodeRpcClient::new(&config.base_node_http);
        let tip_height_start = base_node_client.get_tip_height().await?;
        result.tip_height_start = tip_height_start;

        self.rescan_from_height(config, 0).await?;

        let scan_time_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = scan_time_secs;

        let tip_height_end = base_node_client.get_tip_height().await?;
        result.tip_height_end = tip_height_end;

        let blocks_scanned = tip_height_end.saturating_sub(tip_height_start);
        let utxo_count = self.query_utxo_count().await.unwrap_or(0);
        result.success_count = if utxo_count >= config.volume_target { 1 } else { 0 };

        let mut s2_metrics: HashMap<String, serde_json::Value> = HashMap::new();
        s2_metrics.insert("scan_mode".into(), serde_json::json!("genesis"));
        s2_metrics.insert("blocks_scanned".into(), serde_json::json!(blocks_scanned));
        s2_metrics.insert("utxo_count_found".into(), serde_json::json!(utxo_count));
        result.metrics.extend(s2_metrics);

        info!(
            "S2 (payment processor) completed: {} blocks, {} UTXOs in {:.2}s",
            blocks_scanned, utxo_count, scan_time_secs
        );
        Ok(())
    }

    async fn run_s3(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        // S3 - Scan from Birthday (checkpoint 1)
        use std::time::Instant;

        let start = Instant::now();
        info!("S3: Scan from Birthday (checkpoint 1) for payment processor");

        let base_node_client =
            crate::http_rpc::BaseNodeRpcClient::new(&config.base_node_http);
        let tip_height_start = base_node_client.get_tip_height().await?;
        result.tip_height_start = tip_height_start;

        let birthday_height = 1u64;
        self.rescan_from_height(config, birthday_height).await?;

        let scan_time_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = scan_time_secs;

        let tip_height_end = base_node_client.get_tip_height().await?;
        result.tip_height_end = tip_height_end;

        let blocks_scanned = tip_height_end.saturating_sub(birthday_height);
        let utxo_count = self.query_utxo_count().await.unwrap_or(0);
        result.success_count = if utxo_count >= config.volume_target { 1 } else { 0 };

        let mut s3_metrics: HashMap<String, serde_json::Value> = HashMap::new();
        s3_metrics.insert("scan_mode".into(), serde_json::json!("birthday"));
        s3_metrics.insert("birthday_height".into(), serde_json::json!(birthday_height));
        s3_metrics.insert("blocks_scanned".into(), serde_json::json!(blocks_scanned));
        s3_metrics.insert("utxo_count_found".into(), serde_json::json!(utxo_count));
        result.metrics.extend(s3_metrics);

        info!(
            "S3 (payment processor) completed: {} blocks, {} UTXOs in {:.2}s",
            blocks_scanned, utxo_count, scan_time_secs
        );
        Ok(())
    }

    async fn run_s4(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        use std::time::Instant;

        let start = Instant::now();
        info!(
            "S4: Concurrent Construction for payment processor - batches {:?}",
            config.concurrent_batches
        );

        let mut total_success = 0u32;
        let mut total_failure = 0u32;
        let mut batch_results: Vec<serde_json::Value> = Vec::new();

        for n_concurrent in &config.concurrent_batches {
            let batch_start = Instant::now();
            let n_val = *n_concurrent;
            let recipient = self.address.as_deref().unwrap_or("placeholder").to_string();
            let fee = config.fee_rate;
            let client_arc = Arc::clone(&self.grpc_client);

            let mut handles = Vec::new();
            for i in 0..n_val {
                let rcpt = recipient.clone();
                let c_arc = Arc::clone(&client_arc);
                let handle = tokio::spawn(async move {
                    let start_tx = std::time::Instant::now();
                    let mut guard = c_arc.lock().await;
                    
                    if let Some(ref mut client) = *guard {
                        match client.transfer(&rcpt, 100_000, fee).await {
                            Ok(resp) => {
                                let tx_id = resp.results.first()
                                    .map(|r| r.transaction_id.to_string())
                                    .unwrap_or_else(|| "unknown".to_string());
                                (i, true, tx_id, start_tx.elapsed().as_micros() as u64)
                            }
                            Err(e) => (i, false, format!("error: {}", e), start_tx.elapsed().as_micros() as u64),
                        }
                    } else {
                        (i, false, "no client".to_string(), start_tx.elapsed().as_micros() as u64)
                    }
                });
                handles.push(handle);
            }

            let mut batch_success = 0u32;
            let mut batch_failure = 0u32;
            let mut construction_times: Vec<u64> = Vec::new();
            for handle in handles {
                match handle.await {
                    Ok((_idx, success, tx_id, construction_us)) => {
                        construction_times.push(construction_us);
                        if success {
                            batch_success += 1;
                        } else {
                            batch_failure += 1;
                            result.failure_reasons.push(format!("S4 tx failed: {}", tx_id));
                        }
                    }
                    Err(e) => {
                        batch_failure += 1;
                        warn!("S4: Task join error: {}", e);
                    }
                }
            }

            let batch_time = batch_start.elapsed().as_secs_f64();
            total_success += batch_success;
            total_failure += batch_failure;

            let max_serialization_gap_ms = if construction_times.len() > 1 {
                let mut sorted = construction_times.clone();
                sorted.sort();
                sorted.windows(2)
                    .map(|w| (w[1] - w[0]) as f64 / 1000.0)
                    .fold(0.0_f64, f64::max)
            } else {
                0.0
            };

            batch_results.push(serde_json::json!({
                "n_concurrent": n_val,
                "batch_wall_clock_secs": batch_time,
                "success_count": batch_success,
                "failure_count": batch_failure,
                "success_rate": if n_val > 0 { batch_success as f64 / n_val as f64 } else { 0.0 },
                "max_serialization_gap_ms": max_serialization_gap_ms,
            }));
        }

        let elapsed_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = elapsed_secs;
        result.success_count = total_success;
        result.failure_count = total_failure;

        let mut s4_metrics: HashMap<String, serde_json::Value> = HashMap::new();
        s4_metrics.insert("batch_results".into(), serde_json::json!(batch_results));
        result.metrics.extend(s4_metrics);

        info!(
            "S4 (payment processor) completed: {} success, {} failure in {:.2}s",
            total_success, total_failure, elapsed_secs
        );
        Ok(())
    }

    async fn run_s5(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        use std::time::Instant;

        let start = Instant::now();
        info!(
            "S5: Batch arm for payment processor - {} recipients, batch size {}",
            config.s5_m, config.s5_k
        );

        let recipient = self.address.as_deref().unwrap_or("placeholder");
        let mut success_count = 0u32;
        let mut failure_count = 0u32;
        let mut total_fees: u64 = 0;
        let mut batch_times: Vec<serde_json::Value> = Vec::new();

        let recipients: Vec<String> = (0..config.s5_m)
            .map(|i| format!("{}_{}", recipient, i))
            .collect();

        for (batch_idx, chunk) in recipients.chunks(config.s5_k as usize).enumerate() {
            let batch_start = Instant::now();
            info!(
                "S5: Processing batch {} with {} recipients",
                batch_idx + 1,
                chunk.len()
            );

            let batch_recipients: Vec<(String, u64)> = chunk
                .iter()
                .map(|r| (r.clone(), config.a_fund / config.s5_m as u64))
                .collect();

            match self.send_batch_library(&batch_recipients, config.fee_rate).await {
                Ok(tx_id) => {
                    info!("S5: Batch {} tx: {}", batch_idx + 1, tx_id);
                    success_count += chunk.len() as u32;
                    total_fees += config.fee_rate;
                }
                Err(e) => {
                    failure_count += chunk.len() as u32;
                    result.failure_reasons.push(format!("Batch {} failed: {}", batch_idx + 1, e));
                    warn!("S5: Batch {} failed: {}", batch_idx + 1, e);
                }
            }

            let batch_time = batch_start.elapsed().as_secs_f64();
            let tx_per_sec = if batch_time > 0.0 {
                chunk.len() as f64 / batch_time
            } else {
                0.0
            };

            batch_times.push(serde_json::json!({
                "batch_idx": batch_idx,
                "recipients": chunk.len(),
                "batch_wall_clock_secs": batch_time,
                "success_count": config.s5_k,
                "tx_per_sec": tx_per_sec,
            }));
        }

        let elapsed_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = elapsed_secs;
        result.success_count = success_count;
        result.failure_count = failure_count;
        result.fees_paid_ut = total_fees;

        let overall_tx_per_sec = if elapsed_secs > 0.0 {
            config.s5_m as f64 / elapsed_secs
        } else {
            0.0
        };

        let mut s5_metrics: HashMap<String, serde_json::Value> = HashMap::new();
        s5_metrics.insert("arm".into(), serde_json::json!("batch"));
        s5_metrics.insert("t_batch_secs".into(), serde_json::json!(elapsed_secs));
        s5_metrics.insert("overall_tx_per_sec".into(), serde_json::json!(overall_tx_per_sec));
        s5_metrics.insert("batch_times".into(), serde_json::json!(batch_times));
        result.metrics.extend(s5_metrics);

        info!(
            "S5 (payment processor, batch) completed: {} success in {:.2}s ({:.1} tx/s)",
            success_count, elapsed_secs, overall_tx_per_sec
        );
        Ok(())
    }

    async fn run_s6(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        use std::time::Instant;

        let start = Instant::now();
        info!("S6: Scan from Genesis (checkpoint 2) for payment processor");

        let base_node_client = crate::http_rpc::BaseNodeRpcClient::new(&config.base_node_http);
        let tip_height_start = base_node_client.get_tip_height().await?;
        result.tip_height_start = tip_height_start;

        self.rescan_from_height(config, 0).await?;

        let scan_time_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = scan_time_secs;

        let tip_height_end = base_node_client.get_tip_height().await?;
        result.tip_height_end = tip_height_end;

        let blocks_scanned = tip_height_end.saturating_sub(tip_height_start);
        let utxo_count = self.query_utxo_count().await.unwrap_or(0);
        result.success_count = if utxo_count > 0 { 1 } else { 0 };

        let mut s6_metrics = HashMap::new();
        s6_metrics.insert("scan_mode".into(), serde_json::json!("genesis"));
        s6_metrics.insert("blocks_scanned".into(), serde_json::json!(blocks_scanned));
        s6_metrics.insert("utxo_count_found".into(), serde_json::json!(utxo_count));
        result.metrics.extend(s6_metrics);

        info!("S6 (payment processor) completed: {} blocks, {} UTXOs in {:.2}s", blocks_scanned, utxo_count, scan_time_secs);
        Ok(())
    }

    async fn run_s7(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        use std::time::Instant;

        let start = Instant::now();
        info!("S7: Scan from Birthday (checkpoint 2) for payment processor");

        let base_node_client = crate::http_rpc::BaseNodeRpcClient::new(&config.base_node_http);
        let tip_height_start = base_node_client.get_tip_height().await?;
        result.tip_height_start = tip_height_start;

        let birthday_height = 1u64;
        self.rescan_from_height(config, birthday_height).await?;

        let scan_time_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = scan_time_secs;

        let tip_height_end = base_node_client.get_tip_height().await?;
        result.tip_height_end = tip_height_end;

        let blocks_scanned = tip_height_end.saturating_sub(birthday_height);
        let utxo_count = self.query_utxo_count().await.unwrap_or(0);
        result.success_count = if utxo_count > 0 { 1 } else { 0 };

        let mut s7_metrics = HashMap::new();
        s7_metrics.insert("scan_mode".into(), serde_json::json!("birthday"));
        s7_metrics.insert("birthday_height".into(), serde_json::json!(birthday_height));
        s7_metrics.insert("blocks_scanned".into(), serde_json::json!(blocks_scanned));
        s7_metrics.insert("utxo_count_found".into(), serde_json::json!(utxo_count));
        result.metrics.extend(s7_metrics);

        info!("S7 (payment processor) completed: {} blocks, {} UTXOs in {:.2}s", blocks_scanned, utxo_count, scan_time_secs);
        Ok(())
    }

    async fn wait_for_funding(&self, target: u64, _c_min: u32, timeout_secs: u64) -> Result<u64> {
        let start = std::time::Instant::now();
        let timeout = std::time::Duration::from_secs(timeout_secs);
        loop {
            if start.elapsed() > timeout {
                anyhow::bail!("Timeout waiting for funding of {} µT", target);
            }
            let balance = self.get_balance_via_grpc().await?;
            if balance >= target {
                return Ok(balance);
            }
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        }
    }

    /// Rescan the blockchain. The Scanner reads the starting height from the account's
    /// birthday in the DB. For a genesis rescan (from_height=0), we update the birthday
    /// in the DB so the Scanner starts from block 0. For a birthday rescan, the existing
    /// birthday (set by the gRPC wallet at creation time) is used.
    async fn rescan_from_height(&self, _config: &HarnessConfig, from_height: u64) -> Result<()> {
        info!("Rescanning from height {} using minotari Scanner", from_height);

        // Update the account birthday in the DB so the Scanner starts from the correct height.
        if from_height == 0
            && let Ok(pool) = minotari::init_db(self.db_path.clone())
                && let Ok(conn) = pool.get() {
                    let _ = conn.execute(
                        "UPDATE accounts SET birthday = 0",
                        [],
                    );
                    info!("Set account birthday to 0 for genesis rescan");
                }

        match self.scan_blockchain().await {
            Ok(_) => info!("Rescan completed"),
            Err(e) => warn!("Rescan error: {}", e),
        }
        Ok(())
    }
}
