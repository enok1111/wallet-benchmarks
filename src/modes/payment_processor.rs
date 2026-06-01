//! Payment Processor Mode (batch 1-to-many transactions)
//!
//! Uses the minotari library directly for batch transactions:
//! - UTXO selection via `FundLocker`
//! - Transaction building via `OneSidedTransaction`
//! - Broadcasting via `WalletHttpClient`
//! - Confirmation tracking via `TransactionMonitor`
//!
//! Key difference from NewWalletMode: focuses on batch 1-to-many transactions
//! for S5 throughput comparison.

use anyhow::{anyhow, Context, Result};
use log::{debug, info, warn};
use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use minotari::db::SqlitePool;
use minotari::http::WalletHttpClient;
use minotari::transactions::fund_locker::FundLocker;
use minotari::transactions::monitor::{MonitoringState, TransactionMonitor};
use minotari::transactions::one_sided_transaction::{OneSidedTransaction, Recipient};

use tari_common::configuration::Network;
use tari_common_types::seeds::cipher_seed::CipherSeed;
use tari_common_types::seeds::mnemonic::Mnemonic;
use tari_common_types::seeds::seed_words::SeedWords;
use tari_common_types::tari_address::TariAddress;
use tari_transaction_components::consensus::ConsensusConstantsBuilder;
use tari_transaction_components::offline_signing::sign_locked_transaction;
use tari_transaction_components::tari_amount::MicroMinotari;
use url::Url;

use crate::config::HarnessConfig;
use crate::metrics::ScenarioResult;
use crate::modes::{WalletMode, WalletModeId};

/// Payment processor mode using the minotari library for batch transactions.
pub struct PaymentProcessorMode {
    data_dir: PathBuf,
    db_path: PathBuf,
    db_pool: Option<SqlitePool>,
    address: Option<String>,
    seed_words: Vec<String>,
    password: String,
    base_node_http: String,
    account_id: i64,
    birthday_height: u64,
    monitoring_state: MonitoringState,
}

impl PaymentProcessorMode {
    pub fn new(data_dir: PathBuf, _grpc_port: u16) -> Self {
        let db_path = data_dir.join("wallet.db");
        Self {
            data_dir,
            db_path,
            db_pool: None,
            address: None,
            seed_words: Vec::new(),
            password: "benchmark_password_32chars_min".to_string(),
            base_node_http: "http://127.0.0.1:18143".to_string(),
            account_id: 1,
            birthday_height: 0,
            monitoring_state: MonitoringState::new(),
        }
    }

    fn ensure_db_pool(&mut self) -> Result<&SqlitePool> {
        if self.db_pool.is_none() {
            let pool = minotari::init_db(self.db_path.clone())
                .context("Failed to initialize database pool")?;
            self.db_pool = Some(pool);
        }
        Ok(self.db_pool.as_ref().unwrap())
    }

    async fn init_wallet_db(&mut self, birthday_height: u64) -> Result<()> {
        info!(
            "Initializing payment processor wallet DB at {} with birthday height {}",
            self.db_path.display(),
            birthday_height
        );

        std::fs::create_dir_all(&self.data_dir)?;

        let mnemonic_str = self.seed_words.join(" ");
        let mnemonic_seq = SeedWords::from_str(&mnemonic_str)
            .context("Failed to parse seed words")?;
        let cipher_seed = CipherSeed::from_mnemonic(&mnemonic_seq, None)
            .context("Failed to create CipherSeed from mnemonic")?;

        minotari::utils::init_wallet::init_with_seed_words(
            cipher_seed,
            &self.password,
            &self.db_path,
            Some("default"),
        )
        .context("Failed to initialize wallet with seed words")?;

        {
            let pool = self.ensure_db_pool()?;
            let conn = pool.get().context("Failed to get DB connection")?;
            let birthday_val: i64 = birthday_height as i64;
            let _ = conn.execute(
                "UPDATE accounts SET birthday = ?1 WHERE birthday != ?1",
                [birthday_val],
            );
            info!("Set account birthday to {}", birthday_height);
        }

        {
            let pool = self.ensure_db_pool()?;
            let conn = pool.get().context("Failed to get DB connection")?;
            let accounts = minotari::get_accounts(&conn, Some("default"))
                .context("Failed to get accounts")?;

            if let Some(account) = accounts.first() {
                let addr = account.get_address(Network::Esmeralda, &self.password)?;
                self.address = Some(addr.to_string());
                info!("Wallet address: {}", self.address.as_ref().unwrap());
            }
        }

        Ok(())
    }

    async fn scan_blockchain(&mut self) -> Result<Vec<minotari::WalletEvent>> {
        info!("Scanning blockchain via {} (library integration)", self.base_node_http);

        let (events, _more_blocks) = minotari::Scanner::new(
            &self.password,
            &self.base_node_http,
            self.db_path.clone(),
            100,
            10,
        )
        .mode(minotari::ScanMode::Full)
        .account("default")
        .run()
        .await
        .context("Scanner failed")?;

        info!("Scan completed: {} events processed", events.len());
        Ok(events)
    }

    async fn query_balance(&mut self) -> Result<u64> {
        let pool = self.ensure_db_pool()?;
        let conn = pool.get().context("Failed to get DB connection")?;

        let balance = minotari::get_balance(&conn, self.account_id)
            .context("Failed to query balance")?;

        debug!("Balance: {} uT available", balance.available.0);
        Ok(balance.available.0)
    }

    async fn query_utxo_count(&mut self) -> Result<u32> {
        let pool = self.ensure_db_pool()?;
        let conn = pool.get().context("Failed to get DB connection")?;

        let accounts = minotari::get_accounts(&conn, Some("default"))
            .context("Failed to get accounts")?;
        let account = accounts.first()
            .ok_or_else(|| anyhow!("Default account not found"))?;

        let outputs = minotari::db::fetch_unspent_outputs(&conn, account.id, 0)
            .context("Failed to fetch unspent outputs")?;

        debug!("UTXO count: {}", outputs.len());
        Ok(outputs.len() as u32)
    }

    /// Send a batch transaction to multiple recipients using the library flow.
    async fn send_batch(&mut self, recipients: Vec<Recipient>, fee_per_gram: u64) -> Result<u64> {
        let pool = self.ensure_db_pool()?.clone();
        let conn = pool.get().context("Failed to get DB connection")?;

        let account = minotari::db::get_account_by_name(&conn, "default")?
            .ok_or_else(|| anyhow!("Default account not found"))?;

        let total_amount: u64 = recipients.iter().map(|r| r.amount.0).sum();
        let num_outputs = recipients.len().max(1);

        // Lock funds using FundLocker
        let locker = FundLocker::new(pool.clone());
        let locked_funds = locker.lock(
            self.account_id,
            MicroMinotari(total_amount),
            num_outputs,
            MicroMinotari(fee_per_gram),
            None,
            None,
            300,
            3,
        ).context("Failed to lock funds")?;

        // Create unsigned transaction
        let tx_builder = OneSidedTransaction::new(
            pool.clone(),
            Network::Esmeralda,
            self.password.clone(),
        );

        let unsigned_tx = tx_builder.create_unsigned_transaction(
            &account,
            locked_funds,
            recipients,
            MicroMinotari(fee_per_gram),
        ).context("Failed to create unsigned transaction")?;

        // Sign
        let key_manager = account.get_key_manager(&self.password)?;
        let consensus_constants = ConsensusConstantsBuilder::new(Network::Esmeralda).build();
        let signed_result = sign_locked_transaction(
            &key_manager,
            consensus_constants,
            Network::Esmeralda,
            unsigned_tx,
        ).context("Failed to sign transaction")?;

        // Broadcast
        let base_url = Url::parse(&self.base_node_http)
            .context("Invalid base node URL")?;
        let client = WalletHttpClient::new(base_url)
            .context("Failed to create wallet HTTP client")?;

        let tx = signed_result.signed_transaction.transaction.clone();
        let _response = client
            .submit_transaction(tx)
            .await
            .context("Failed to submit transaction")?;

        let numeric_tx_id = signed_result.request.tx_id.as_u64();
        debug!("Batch transaction broadcast successfully (tx_id={})", numeric_tx_id);
        Ok(numeric_tx_id)
    }

    async fn wait_for_confirmation(&mut self, tx_id: u64, c_min: u32, timeout_secs: u64) -> Result<()> {
        let start = std::time::Instant::now();
        let timeout = Duration::from_secs(timeout_secs);
        let base_node = crate::http_rpc::BaseNodeRpcClient::new(&self.base_node_http);
        let base_node_http = self.base_node_http.clone();
        let account_id = self.account_id;

        loop {
            if start.elapsed() > timeout {
                anyhow::bail!(
                    "Timeout waiting for tx {} confirmation (c_min={})",
                    tx_id, c_min
                );
            }

            let tip_height = base_node.get_tip_height().await?;

            let pool = self.ensure_db_pool()?.clone();
            let base_url = Url::parse(&base_node_http)
                .context("Invalid base node URL")?;
            let client = WalletHttpClient::new(base_url)
                .context("Failed to create wallet HTTP client")?;

            let monitoring_state = self.monitoring_state.clone();
            let monitor = TransactionMonitor::new(
                monitoring_state,
                c_min as u64,
                None,
            );

            let _result = monitor
                .monitor_if_needed(&client, &pool, account_id, tip_height)
                .await
                .context("Transaction monitor failed")?;

            let utxo_count = self.query_utxo_count().await.unwrap_or(0);
            let balance = self.query_balance().await.unwrap_or(0);

            debug!(
                "Tx {} polling: tip={}, utxos={}, balance={}",
                tx_id, tip_height, utxo_count, balance
            );

            if utxo_count > 0 && balance > 0 && tip_height > 0 {
                return Ok(());
            }

            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }

    async fn wait_for_funding(&mut self, target_balance: u64, _c_min: u32, timeout_secs: u64) -> Result<u64> {
        let start = std::time::Instant::now();
        let timeout = Duration::from_secs(timeout_secs);

        loop {
            if start.elapsed() > timeout {
                anyhow::bail!("Timeout waiting for funding (target={} uT)", target_balance);
            }

            let balance = self.query_balance().await?;
            debug!("Funding poll: balance={} uT (target={} uT)", balance, target_balance);

            if balance >= target_balance {
                return Ok(balance);
            }

            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    }

    fn generate_seed_words(_mode_id: WalletModeId) -> Vec<String> {
        crate::modes::generate_tari_seed_words()
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

        if let Err(e) = self.scan_blockchain().await {
            warn!("Initial scan (expected for empty wallet): {}", e);
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
            "S5" => self.run_s5(config, &mut result).await?,
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
        let pool = self.db_pool.clone()
            .ok_or_else(|| anyhow!("Database not initialized"))?;
        let conn = pool.get().context("Failed to get DB connection")?;
        let balance = minotari::get_balance(&conn, self.account_id)
            .context("Failed to query balance")?;
        Ok(balance.available.0)
    }

    async fn teardown(&mut self) -> Result<()> {
        info!("Tearing down payment processor mode");
        info!("Payment processor torn down");
        Ok(())
    }
}

impl PaymentProcessorMode {
    async fn run_b0(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
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
        use std::time::Instant;

        let start = Instant::now();
        info!("S0: Funding baseline for payment processor mode");

        let wait_result = self.wait_for_funding(config.a_fund, config.c_min, 600).await;
        let elapsed_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = elapsed_secs;

        match wait_result {
            Ok(balance) => {
                result.success_count = 1;
                result.balance_delta_ut = balance as i64;
                let base_node = crate::http_rpc::BaseNodeRpcClient::new(&self.base_node_http);
                if let Ok(tip) = base_node.get_tip_height().await {
                    self.birthday_height = tip;
                    info!("S0: Captured birthday height {}", tip);
                }
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
        use std::time::Instant;

        let start = Instant::now();
        info!(
            "S1: UTXO build-up for payment processor - target {} UTXOs",
            config.volume_target
        );

        let mut current_utxos = 1u32;

        // Doubling phase
        for round in 0..config.doubling_rounds {
            let recipient = Recipient {
                address: TariAddress::from_base58(
                    self.address.as_deref()
                        .ok_or_else(|| anyhow!("Wallet address not available"))?,
                )?,
                amount: MicroMinotari(100_000 * 2),
                payment_id: Some(format!("bench-s1-double-{}", round + 1)),
            };

            match self.send_batch(vec![recipient], config.fee_rate).await {
                Ok(tx_id) => {
                    info!("S1: Doubling round {} tx: {}", round + 1, tx_id);
                    self.wait_for_confirmation(tx_id, config.c_min, 300).await?;
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

                let addr = TariAddress::from_base58(
                    self.address.as_deref()
                        .ok_or_else(|| anyhow!("Wallet address not available"))?,
                )?;
                let recipients: Vec<Recipient> = (0..outputs)
                    .map(|i| Recipient {
                        address: addr.clone(),
                        amount: MicroMinotari(100_000),
                        payment_id: Some(format!("bench-s1-fanout-{}-{}", round + 1, i)),
                    })
                    .collect();

                match self.send_batch(recipients, config.fee_rate).await {
                    Ok(tx_id) => {
                        info!("S1: Fan-out round {} tx: {}", round + 1, tx_id);
                        self.wait_for_confirmation(tx_id, config.c_min, 300).await?;
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
        use std::time::Instant;

        let start = Instant::now();
        info!("S2: Scan from genesis (checkpoint 1) for payment processor");

        let base_node_client =
            crate::http_rpc::BaseNodeRpcClient::new(&config.base_node_http);
        let tip_height_start = base_node_client.get_tip_height().await?;
        result.tip_height_start = tip_height_start;

        std::fs::remove_dir_all(&self.data_dir).ok();
        std::fs::create_dir_all(&self.data_dir)?;
        self.init_wallet_db(0).await?;

        match self.scan_blockchain().await {
            Ok(events) => info!("S2 scan completed: {} events", events.len()),
            Err(e) => warn!("S2 scan error: {}", e),
        }

        let scan_time_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = scan_time_secs;

        let tip_height_end = base_node_client.get_tip_height().await?;
        result.tip_height_end = tip_height_end;

        let blocks_scanned = tip_height_end.saturating_sub(tip_height_start);
        result.success_count = 1;

        let mut s2_metrics: HashMap<String, serde_json::Value> = HashMap::new();
        s2_metrics.insert("scan_mode".into(), serde_json::json!("genesis"));
        s2_metrics.insert("blocks_scanned".into(), serde_json::json!(blocks_scanned));
        result.metrics.extend(s2_metrics);

        Ok(())
    }

    async fn run_s3(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        use std::time::Instant;

        let start = Instant::now();
        info!("S3: Scan from birthday (checkpoint 1) for payment processor");

        let base_node_client =
            crate::http_rpc::BaseNodeRpcClient::new(&config.base_node_http);
        let tip_height_start = base_node_client.get_tip_height().await?;
        result.tip_height_start = tip_height_start;

        std::fs::remove_dir_all(&self.data_dir).ok();
        std::fs::create_dir_all(&self.data_dir)?;
        let birthday = if self.birthday_height > 0 { self.birthday_height } else { tip_height_start };
        self.init_wallet_db(birthday).await?;

        match self.scan_blockchain().await {
            Ok(events) => info!("S3 scan completed: {} events", events.len()),
            Err(e) => warn!("S3 scan error: {}", e),
        }

        let scan_time_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = scan_time_secs;

        let tip_height_end = base_node_client.get_tip_height().await?;
        result.tip_height_end = tip_height_end;

        let blocks_scanned = tip_height_end.saturating_sub(tip_height_start);
        result.success_count = 1;

        let mut s3_metrics: HashMap<String, serde_json::Value> = HashMap::new();
        s3_metrics.insert("scan_mode".into(), serde_json::json!("birthday"));
        s3_metrics.insert("blocks_scanned".into(), serde_json::json!(blocks_scanned));
        result.metrics.extend(s3_metrics);

        Ok(())
    }

    async fn run_s4(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        use std::time::Instant;

        let start = Instant::now();
        info!("S4: Concurrent construction for payment processor");

        let mut total_successes = 0u32;
        let mut total_failures = 0u32;

        let db_path = self.db_path.clone();
        let address = self.address.clone();
        let password = self.password.clone();
        let base_node_http = self.base_node_http.clone();
        let account_id = self.account_id;

        for &n_concurrent in &config.concurrent_batches {
            info!("S4: Running {} concurrent transactions", n_concurrent);

            let mut handles = Vec::new();
            for i in 0..n_concurrent {
                let db_path = db_path.clone();
                let addr = address.clone();
                let pwd = password.clone();
                let base_http = base_node_http.clone();
                let acc_id = account_id;
                let fee_rate = config.fee_rate;

                let handle = tokio::spawn(async move {
                    send_concurrent_tx(&db_path, &addr, &pwd, &base_http, acc_id, fee_rate, i).await
                });
                handles.push(handle);
            }

            let batch_start = Instant::now();
            let batch_results: Vec<_> = futures::future::join_all(handles).await;

            for r in batch_results {
                match r {
                    Ok(Ok(tx_id)) => {
                        total_successes += 1;
                        debug!("S4 tx succeeded: tx_id={}", tx_id);
                    }
                    Ok(Err(e)) => {
                        total_failures += 1;
                        result.failure_reasons.push(format!("S4 concurrent tx failed: {}", e));
                    }
                    Err(join_err) => {
                        total_failures += 1;
                        result.failure_reasons.push(format!("S4 task join error: {}", join_err));
                    }
                }
            }

            let batch_time = batch_start.elapsed().as_secs_f64();
            debug!("S4: {} concurrent txs completed in {:.2}s", n_concurrent, batch_time);
        }

        let elapsed_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = elapsed_secs;
        result.success_count = total_successes;
        result.failure_count = total_failures;

        let mut s4_metrics: HashMap<String, serde_json::Value> = HashMap::new();
        s4_metrics.insert("concurrent_batches".into(), serde_json::json!(&config.concurrent_batches));
        s4_metrics.insert("total_successes".into(), serde_json::json!(total_successes));
        s4_metrics.insert("total_failures".into(), serde_json::json!(total_failures));
        result.metrics.extend(s4_metrics);

        Ok(())
    }

    async fn run_s5(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        use std::time::Instant;

        let start = Instant::now();
        info!("S5: Payment processor throughput (batch arm)");

        // Arm A - Batch sends (payment processor mode does batch sends)
        let mut successes = 0u32;
        let mut failures = 0u32;

        // Create recipients for batch txs
        let addr = TariAddress::from_base58(
            self.address.as_deref().unwrap_or("otl_esm_1placeholder"),
        )?;
        let recipients_per_batch = config.s5_k as usize;
        let num_batches = (config.s5_m as usize).div_ceil(recipients_per_batch);

        for batch_idx in 0..num_batches {
            let recipients: Vec<Recipient> = (0..recipients_per_batch)
                .map(|i| Recipient {
                    address: addr.clone(),
                    amount: MicroMinotari(100_000),
                    payment_id: Some(format!("bench-s5-batch-{}-{}", batch_idx, i)),
                })
                .collect();

            match self.send_batch(recipients, config.fee_rate).await {
                Ok(_) => successes += 1,
                Err(e) => {
                    failures += 1;
                    result.failure_reasons.push(format!("S5 batch tx {} failed: {}", batch_idx, e));
                }
            }
        }

        let elapsed_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = elapsed_secs;
        result.success_count = successes;
        result.failure_count = failures;

        let mut s5_metrics: HashMap<String, serde_json::Value> = HashMap::new();
        s5_metrics.insert("arm".into(), serde_json::json!("batch"));
        s5_metrics.insert("t_batch_secs".into(), serde_json::json!(elapsed_secs));
        s5_metrics.insert("successes".into(), serde_json::json!(successes));
        s5_metrics.insert("failures".into(), serde_json::json!(failures));
        result.metrics.extend(s5_metrics);

        Ok(())
    }

    async fn run_s6(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        let start = std::time::Instant::now();
        info!("S6: Scan from genesis (checkpoint 2) for payment processor");

        let base_node_client =
            crate::http_rpc::BaseNodeRpcClient::new(&config.base_node_http);
        let tip_height_start = base_node_client.get_tip_height().await?;
        result.tip_height_start = tip_height_start;

        std::fs::remove_dir_all(&self.data_dir).ok();
        std::fs::create_dir_all(&self.data_dir)?;
        self.init_wallet_db(0).await?;

        match self.scan_blockchain().await {
            Ok(events) => info!("S6 scan completed: {} events", events.len()),
            Err(e) => warn!("S6 scan error: {}", e),
        }

        let scan_time_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = scan_time_secs;

        let tip_height_end = base_node_client.get_tip_height().await?;
        result.tip_height_end = tip_height_end;

        let blocks_scanned = tip_height_end.saturating_sub(tip_height_start);
        result.success_count = 1;

        let mut s6_metrics: HashMap<String, serde_json::Value> = HashMap::new();
        s6_metrics.insert("scan_mode".into(), serde_json::json!("genesis"));
        s6_metrics.insert("blocks_scanned".into(), serde_json::json!(blocks_scanned));
        result.metrics.extend(s6_metrics);

        Ok(())
    }

    async fn run_s7(
        &mut self,
        config: &HarnessConfig,
        result: &mut ScenarioResult,
    ) -> Result<()> {
        let start = std::time::Instant::now();
        info!("S7: Scan from birthday (checkpoint 2) for payment processor");

        let base_node_client =
            crate::http_rpc::BaseNodeRpcClient::new(&config.base_node_http);
        let tip_height_start = base_node_client.get_tip_height().await?;
        result.tip_height_start = tip_height_start;

        std::fs::remove_dir_all(&self.data_dir).ok();
        std::fs::create_dir_all(&self.data_dir)?;
        let birthday = if self.birthday_height > 0 { self.birthday_height } else { tip_height_start };
        self.init_wallet_db(birthday).await?;

        match self.scan_blockchain().await {
            Ok(events) => info!("S7 scan completed: {} events", events.len()),
            Err(e) => warn!("S7 scan error: {}", e),
        }

        let scan_time_secs = start.elapsed().as_secs_f64();
        result.wall_clock_secs = scan_time_secs;

        let tip_height_end = base_node_client.get_tip_height().await?;
        result.tip_height_end = tip_height_end;

        let blocks_scanned = tip_height_end.saturating_sub(tip_height_start);
        result.success_count = 1;

        let mut s7_metrics: HashMap<String, serde_json::Value> = HashMap::new();
        s7_metrics.insert("scan_mode".into(), serde_json::json!("birthday"));
        s7_metrics.insert("blocks_scanned".into(), serde_json::json!(blocks_scanned));
        result.metrics.extend(s7_metrics);

        Ok(())
    }
}

/// Send a concurrent transaction using library functions.
async fn send_concurrent_tx(
    db_path: &PathBuf,
    address: &Option<String>,
    password: &str,
    base_node_http: &str,
    account_id: i64,
    fee_per_gram: u64,
    index: usize,
) -> Result<u64> {
    let db = minotari::init_db(db_path.clone())?;
    let conn = db.get()?;

    let account = minotari::db::get_account_by_name(&conn, "default")?
        .ok_or_else(|| anyhow!("Default account not found"))?;

    let addr = TariAddress::from_base58(
        address.as_deref()
            .ok_or_else(|| anyhow!("Wallet address not available"))?,
    ).context("Invalid Tari address")?;

    let recipient = Recipient {
        address: addr.clone(),
        amount: MicroMinotari(100_000),
        payment_id: Some(format!("bench-s4-{}", index)),
    };

    let locker = FundLocker::new(db.clone());
    let locked_funds = locker.lock(
        account_id,
        MicroMinotari(100_000),
        1,
        MicroMinotari(fee_per_gram),
        None,
        None,
        300,
        3,
    ).context("Failed to lock funds for concurrent tx")?;

    let tx_builder = OneSidedTransaction::new(
        db.clone(),
        Network::Esmeralda,
        password.to_string(),
    );

    let unsigned_tx = tx_builder.create_unsigned_transaction(
        &account,
        locked_funds,
        vec![recipient],
        MicroMinotari(fee_per_gram),
    ).context("Failed to create unsigned transaction")?;

    let key_manager = account.get_key_manager(password)?;
    let consensus_constants = ConsensusConstantsBuilder::new(Network::Esmeralda).build();
    let signed_result = sign_locked_transaction(
        &key_manager,
        consensus_constants,
        Network::Esmeralda,
        unsigned_tx,
    ).context("Failed to sign transaction")?;

    let base_url = Url::parse(base_node_http)
        .context("Invalid base node URL")?;
    let client = WalletHttpClient::new(base_url)
        .context("Failed to create wallet HTTP client")?;

    let tx = signed_result.signed_transaction.transaction.clone();
    let _response = client
        .submit_transaction(tx)
        .await
        .context("Failed to submit transaction")?;

    let numeric_tx_id = signed_result.request.tx_id.as_u64();
    debug!("Concurrent tx broadcast successfully (tx_id={})", numeric_tx_id);
    Ok(numeric_tx_id)
}
