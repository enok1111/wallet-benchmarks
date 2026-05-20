//! gRPC client wrapper for minotari_console_wallet communication
//!
//! Uses tonic + minotari_app_grpc proto definitions to communicate with
//! the old wallet's gRPC server.

use anyhow::{Context, Result};
use log::debug;
use minotari_app_grpc::tari_rpc::{
    wallet_client::WalletClient,
    GetBalanceRequest, GetBalanceResponse,
    GetAddressResponse,
    PaymentRecipient, TransferRequest, TransferResponse,
    Empty, UserPaymentId,
};
use std::time::Duration;

/// gRPC client wrapper for old wallet communication
pub struct OldWalletGrpcClient {
    /// Tonic client connected to the wallet gRPC server
    client: WalletClient<tonic::transport::Channel>,
}

impl OldWalletGrpcClient {
    /// Create a new gRPC client connected to the wallet at the given address
    pub async fn connect(grpc_address: &str) -> Result<Self> {
        debug!("Connecting to wallet gRPC at {}", grpc_address);
        
        let client = WalletClient::connect(format!("http://{}", grpc_address))
            .await
            .context("Failed to connect to wallet gRPC server")?;
        
        Ok(Self { client })
    }

    /// Get a mutable reference to the inner client for gRPC calls
    pub fn client_mut(&mut self) -> &mut WalletClient<tonic::transport::Channel> {
        &mut self.client
    }

    /// Check if the gRPC connection is alive
    pub async fn ping(&mut self) -> Result<()> {
        debug!("Pinging wallet gRPC server");
        let _ = self.get_address().await?;
        Ok(())
    }

    /// Get wallet address
    pub async fn get_address(&mut self) -> Result<GetAddressResponse> {
        debug!("Calling GetAddress");
        
        let request = tonic::Request::<Empty>::new(Empty {});
        
        let response = self
            .client_mut()
            .get_address(request)
            .await
            .context("GetAddress RPC failed")?;
        
        Ok(response.into_inner())
    }

    /// Get wallet balance
    pub async fn get_balance(&mut self) -> Result<GetBalanceResponse> {
        debug!("Calling GetBalance");
        
        let request = tonic::Request::new(GetBalanceRequest {
            payment_id: None,
        });
        
        let response = self
            .client_mut()
            .get_balance(request)
            .await
            .context("GetBalance RPC failed")?;
        
        Ok(response.into_inner())
    }

    /// Transfer funds to a recipient
    pub async fn transfer(
        &mut self,
        destination: &str,
        amount: u64,
        fee_per_gram: u64,
    ) -> Result<TransferResponse> {
        debug!(
            "Calling Transfer: destination={}, amount={}, fee_per_gram={}",
            destination, amount, fee_per_gram
        );
        
        let recipient = PaymentRecipient {
            address: destination.to_string(),
            amount,
            fee_per_gram,
            payment_type: 0,  // PaymentType::Standard
            raw_payment_id: Vec::new(),
            user_payment_id: None,
        };
        
        let request = tonic::Request::new(TransferRequest {
            recipients: vec![recipient],
            single_tx: false,
        });
        
        let response = self
            .client_mut()
            .transfer(request)
            .await
            .context("Transfer RPC failed")?;
        
        Ok(response.into_inner())
    }

    /// Get chain tip height from wallet via GetState
    pub async fn get_tip_height(&mut self) -> Result<u64> {
        debug!("Getting tip height via GetState");

        let request = tonic::Request::new(minotari_app_grpc::tari_rpc::GetStateRequest {});
        let response = self
            .client_mut()
            .get_state(request)
            .await
            .context("GetState RPC failed")?;

        let state = response.into_inner();
        Ok(state.scanned_height)
    }

    /// Wait for the gRPC server to be ready by attempting a connection
    pub async fn wait_for_ready(address: &str, timeout_secs: u64) -> Result<Option<Self>> {
        let start = std::time::Instant::now();
        let timeout = Duration::from_secs(timeout_secs);
        
        loop {
            if start.elapsed() > timeout {
                return Ok(None);
            }
            
            match Self::connect(address).await {
                Ok(client) => {
                    debug!("Wallet gRPC server ready at {}", address);
                    return Ok(Some(client));
                }
                Err(e) => {
                    debug!("Waiting for wallet gRPC: {}", e);
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            }
        }
    }
}
