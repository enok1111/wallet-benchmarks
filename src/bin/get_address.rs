use wallet_benchmarks::grpc_client::OldWalletGrpcClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = OldWalletGrpcClient::connect("127.0.0.1:18143").await?;
    let addr = client.get_address().await?;
    println!("Interactive address: {}", hex::encode(&addr.interactive_address));
    
    let balance = client.get_balance().await?;
    println!("Available balance: {}", balance.available_balance);
    
    let tip = client.get_tip_height().await?;
    println!("Scanned height: {}", tip);
    
    Ok(())
}
