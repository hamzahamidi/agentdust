pub mod probe;
#[cfg(target_os = "macos")]
mod tools;

#[cfg(target_os = "macos")]
use rmcp::ServiceExt;

pub use probe::ProbeServer;
#[cfg(target_os = "macos")]
use tools::AgentDustServer;

#[cfg(target_os = "macos")]
pub async fn serve_stdio() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let service = AgentDustServer::new()?.serve(rmcp::transport::stdio()).await?;
    service.waiting().await?;
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub async fn serve_stdio() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "the AgentDust server requires macOS",
    )
    .into())
}
