//! HTTP RPC client for Tari base node and wallet communication
//!
//! Uses reqwest to make HTTP calls to the base node REST API.
//! Provides methods for balance queries, transaction submission, and chain state.

use anyhow::{anyhow, Context, Result};
use log::debug;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Base node HTTP RPC client
pub struct BaseNodeRpcClient {
    /// Base node HTTP endpoint
    base_url: String,
    /// HTTP client instance
    http: reqwest::Client,
}

impl BaseNodeRpcClient {
    /// Create a new RPC client for the given base node URL
    pub fn new(base_url: &str) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .expect("Failed to build HTTP client");

        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            http,
        }
    }

    /// Get the current chain tip height
    pub async fn get_tip_height(&self) -> Result<u64> {
        let url = format!("{}/get_tip_info", self.base_url);
        debug!("GET {}", url);

        let response: TipInfoResponse = self
            .http
            .get(&url)
            .send()
            .await
            .context("Failed to send request")?
            .json()
            .await
            .context("Failed to parse response")?;

        Ok(response.metadata.best_block_height)
    }

    /// Submit a transaction to the base node via JSON-RPC
    #[allow(dead_code)]
    pub async fn submit_transaction(&self, tx_json: &serde_json::Value) -> Result<String> {
        let url = format!("{}/json_rpc", self.base_url);
        debug!("POST {} (JSON-RPC submit_transaction)", url);

        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "1",
            "method": "submit_transaction",
            "params": {
                "transaction": tx_json,
                "version": 2
            }
        });

        let response: serde_json::Value = self
            .http
            .post(&url)
            .json(&request)
            .send()
            .await
            .context("Failed to send transaction")?
            .json()
            .await
            .context("Failed to parse response")?;

        if let Some(error) = response.get("error") {
            return Err(anyhow!("Transaction submission failed: {}", error));
        }

        let result = response.get("result").context("Missing result in response")?;
        let accepted = result.get("accepted").and_then(|v| v.as_bool()).unwrap_or(false);

        if !accepted {
            let reason = result.get("rejection_reason").and_then(|v| v.as_str()).unwrap_or("unknown");
            return Err(anyhow!("Transaction rejected: {}", reason));
        }

        Ok("accepted".to_string())
    }

    /// Get block outputs at a specific height (for scanning)
    #[allow(dead_code)]
    pub async fn get_block_outputs(&self, height: u64) -> Result<Vec<BlockOutput>> {
        let url = format!("{}/get_utxos_by_block?block_height={}", self.base_url, height);
        debug!("GET {}", url);

        let response: BlockOutputsResponse = self
            .http
            .get(&url)
            .send()
            .await
            .context("Failed to send request")?
            .json()
            .await
            .context("Failed to parse response")?;

        Ok(response.outputs)
    }

    /// Check if the base node is reachable
    #[allow(dead_code)]
    pub async fn check_connectivity(&self) -> Result<bool> {
        let url = format!("{}/get_tip_info", self.base_url);
        debug!("GET {} (connectivity check)", url);

        match self.http.get(&url).send().await {
            Ok(response) => Ok(response.status().is_success()),
            Err(_) => Ok(false),
        }
    }

    /// Wait for the base node to be ready
    #[allow(dead_code)]
    pub async fn wait_for_ready(&self, timeout_secs: u64) -> Result<()> {
        let start = std::time::Instant::now();
        let timeout = Duration::from_secs(timeout_secs);

        loop {
            if start.elapsed() > timeout {
                return Err(anyhow!("Timeout waiting for base node to be ready"));
            }

            if self.check_connectivity().await? {
                debug!("Base node is ready at {}", self.base_url);
                return Ok(());
            }

            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct BalanceResponse {
    pub available_balance: u64,
    pub pending_incoming_balance: u64,
    pub pending_outgoing_balance: u64,
    #[serde(default)]
    pub timelocked_balance: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TipInfoMetadata {
    pub best_block_height: u64,
    #[serde(default, deserialize_with = "deserialize_hex_array")]
    pub best_block_hash: Vec<u8>,
    #[serde(default)]
    pub pruning_horizon: u64,
    #[serde(default)]
    pub pruned_height: u64,
    #[serde(default)]
    pub accumulated_difficulty: String,
    #[serde(default)]
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TipInfoResponse {
    pub metadata: TipInfoMetadata,
    #[serde(default)]
    pub is_synced: bool,
}

fn deserialize_hex_array<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let arr: Vec<u64> = serde::Deserialize::deserialize(deserializer)?;
    let mut bytes = Vec::with_capacity(arr.len());
    for v in arr {
        bytes.push((v & 0xFF) as u8);
    }
    Ok(bytes)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct BlockOutputsResponse {
    pub outputs: Vec<BlockOutput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct BlockOutput {
    #[serde(default)]
    pub commit: String,
    #[serde(default)]
    pub feature: u32,
    #[serde(default)]
    pub value: u64,
    #[serde(default)]
    pub offset: String,
}

/// Old wallet HTTP RPC client (for wallets that expose HTTP endpoints)
#[allow(dead_code)]
pub struct WalletRpcClient {
    /// Wallet HTTP endpoint
    base_url: String,
    /// HTTP client instance
    http: reqwest::Client,
}

impl WalletRpcClient {
    /// Create a new wallet RPC client
    #[allow(dead_code)]
    pub fn new(base_url: &str) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .expect("Failed to build HTTP client");

        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            http,
        }
    }

    /// Get wallet balance
    #[allow(dead_code)]
    pub async fn get_balance(&self) -> Result<BalanceResponse> {
        let url = format!("{}/v1/balance", self.base_url);
        debug!("GET {}", url);

        let response: BalanceResponse = self
            .http
            .get(&url)
            .send()
            .await
            .context("Failed to send request")?
            .json()
            .await
            .context("Failed to parse response")?;

        Ok(response)
    }

    /// Get wallet address
    #[allow(dead_code)]
    pub async fn get_address(&self) -> Result<String> {
        let url = format!("{}/v1/address", self.base_url);
        debug!("GET {}", url);

        let response: serde_json::Value = self
            .http
            .get(&url)
            .send()
            .await
            .context("Failed to send request")?
            .json()
            .await
            .context("Failed to parse response")?;

        // Try different response formats
        if let Some(addr) = response.get("interactive_address").and_then(|v| v.as_str()) {
            return Ok(addr.to_string());
        }
        if let Some(addr) = response.get("address").and_then(|v| v.as_str()) {
            return Ok(addr.to_string());
        }

        Err(anyhow!("Could not parse address from response"))
    }

    /// Transfer funds to a recipient
    #[allow(dead_code)]
    pub async fn transfer(
        &self,
        destination: &str,
        amount: u64,
        fee_per_gram: u64,
    ) -> Result<String> {
        let url = format!("{}/v1/transfer", self.base_url);
        debug!("POST {} to={} amount={}", url, destination, amount);

        let request = serde_json::json!({
            "destination": destination,
            "amount": amount,
            "fee_per_gram": fee_per_gram,
        });

        let response: serde_json::Value = self
            .http
            .post(&url)
            .json(&request)
            .send()
            .await
            .context("Failed to send transfer request")?
            .json()
            .await
            .context("Failed to parse response")?;

        if let Some(tx_id) = response.get("transaction_id").and_then(|v| v.as_str()) {
            return Ok(tx_id.to_string());
        }
        if let Some(result) = response.get("result").and_then(|v| v.as_str()) {
            return Ok(result.to_string());
        }

        Err(anyhow!("Could not parse transaction ID from response"))
    }

    /// Get wallet state (scanned height, balances, etc.)
    #[allow(dead_code)]
    pub async fn get_state(&self) -> Result<serde_json::Value> {
        let url = format!("{}/v1/state", self.base_url);
        debug!("GET {}", url);

        let response: serde_json::Value = self
            .http
            .get(&url)
            .send()
            .await
            .context("Failed to send request")?
            .json()
            .await
            .context("Failed to parse response")?;

        Ok(response)
    }

    /// Wait for balance to reach minimum threshold
    #[allow(dead_code)]
    pub async fn wait_for_balance(&self, min_balance: u64, timeout_secs: u64) -> Result<()> {
        let start = std::time::Instant::now();
        let timeout = Duration::from_secs(timeout_secs);

        loop {
            if start.elapsed() > timeout {
                return Err(anyhow!(
                    "Timeout waiting for balance >= {} µT",
                    min_balance
                ));
            }

            let balance = self.get_balance().await?;
            if balance.available_balance >= min_balance {
                debug!(
                    "Balance reached: {} µT >= {} µT",
                    balance.available_balance, min_balance
                );
                return Ok(());
            }

            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }
}
