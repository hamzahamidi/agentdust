pub mod probe;
mod tools;

use rmcp::ServiceExt;

pub use probe::ProbeServer;
use tools::AgentDustServer;

pub async fn serve_stdio() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let service = AgentDustServer::new()?.serve(rmcp::transport::stdio()).await?;
    service.waiting().await?;
    Ok(())
}
