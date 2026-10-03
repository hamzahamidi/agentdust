pub mod probe;

use rmcp::ServiceExt;

pub use probe::ProbeServer;

pub async fn serve_stdio() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let service = ProbeServer::default().serve(rmcp::transport::stdio()).await?;
    service.waiting().await?;
    Ok(())
}
