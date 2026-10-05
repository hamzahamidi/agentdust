use std::ffi::OsStr;
use std::io;
use std::path::PathBuf;

pub const DATA_DIR_ENV: &str = "AGENTDUST_DATA_DIR";
pub const CLAUDE_CONFIG_DIR_ENV: &str = "CLAUDE_CONFIG_DIR";

pub fn data_dir() -> io::Result<PathBuf> {
    resolve_data_dir(
        std::env::var_os(DATA_DIR_ENV).as_deref(),
        std::env::var_os("HOME").as_deref(),
    )
}

pub fn claude_config_dir() -> io::Result<PathBuf> {
    resolve_claude_config_dir(
        std::env::var_os(CLAUDE_CONFIG_DIR_ENV).as_deref(),
        std::env::var_os("HOME").as_deref(),
    )
}

pub fn resolve_claude_config_dir(override_dir: Option<&OsStr>, home: Option<&OsStr>) -> io::Result<PathBuf> {
    if let Some(dir) = override_dir.filter(|dir| !dir.is_empty()) {
        return absolute(dir, CLAUDE_CONFIG_DIR_ENV);
    }
    let home = home
        .filter(|home| !home.is_empty())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
    Ok(absolute(home, "HOME")?.join(".claude"))
}

pub fn resolve_data_dir(override_dir: Option<&OsStr>, home: Option<&OsStr>) -> io::Result<PathBuf> {
    if let Some(dir) = override_dir.filter(|dir| !dir.is_empty()) {
        return absolute(dir, DATA_DIR_ENV);
    }
    let home = home
        .filter(|home| !home.is_empty())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
    Ok(absolute(home, "HOME")?.join("Library/Application Support/agentdust"))
}

fn absolute(value: &OsStr, name: &str) -> io::Result<PathBuf> {
    let path = PathBuf::from(value);
    if path.is_relative() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{name} must be an absolute path"),
        ));
    }
    Ok(path)
}
