pub mod probe;
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod tools;

#[cfg(any(target_os = "macos", target_os = "linux"))]
use rmcp::ServiceExt;

pub use probe::ProbeServer;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use tools::AgentDustServer;

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub async fn serve_stdio() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let service = AgentDustServer::new()?.serve(rmcp::transport::stdio()).await?;
    service.waiting().await?;
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub async fn serve_stdio() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "the AgentDust server requires macOS",
    )
    .into())
}
