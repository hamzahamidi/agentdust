use std::io;
use std::path::PathBuf;

pub const DATA_DIR_ENV: &str = "AGENTDUST_DATA_DIR";

pub fn data_dir() -> io::Result<PathBuf> {
    if let Some(dir) = std::env::var_os(DATA_DIR_ENV) {
        return Ok(PathBuf::from(dir));
    }
    let home =
        std::env::var_os("HOME").ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
    Ok(PathBuf::from(home).join("Library/Application Support/agentdust"))
}
