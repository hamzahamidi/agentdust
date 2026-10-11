use std::error::Error;

use agentdust_mcp::ProbeServer;
use rmcp::ServiceExt;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let service = ProbeServer::default().serve(rmcp::transport::stdio()).await?;
    service.waiting().await?;
    Ok(())
}
