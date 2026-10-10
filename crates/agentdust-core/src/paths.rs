use std::ffi::OsStr;
use std::io;
use std::path::PathBuf;

pub const DATA_DIR_ENV: &str = "AGENTDUST_DATA_DIR";
pub const CLAUDE_CONFIG_DIR_ENV: &str = "CLAUDE_CONFIG_DIR";

pub fn data_dir() -> io::Result<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        resolve_linux_data_dir(
            std::env::var_os(DATA_DIR_ENV).as_deref(),
            std::env::var_os("XDG_DATA_HOME").as_deref(),
            std::env::var_os("HOME").as_deref(),
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        resolve_data_dir(
            std::env::var_os(DATA_DIR_ENV).as_deref(),
            std::env::var_os("HOME").as_deref(),
        )
    }
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
    #[cfg(target_os = "linux")]
    return Ok(absolute(home, "HOME")?.join(".local/share/agentdust"));
    #[cfg(not(target_os = "linux"))]
    Ok(absolute(home, "HOME")?.join("Library/Application Support/agentdust"))
}

#[cfg(target_os = "linux")]
fn resolve_linux_data_dir(
    override_dir: Option<&OsStr>,
    xdg_data_home: Option<&OsStr>,
    home: Option<&OsStr>,
) -> io::Result<PathBuf> {
    if let Some(dir) = override_dir.filter(|dir| !dir.is_empty()) {
        return absolute(dir, DATA_DIR_ENV);
    }
    if let Some(dir) = xdg_data_home.filter(|dir| !dir.is_empty()) {
        return Ok(absolute(dir, "XDG_DATA_HOME")?.join("agentdust"));
    }
    let home = home
        .filter(|home| !home.is_empty())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
    Ok(absolute(home, "HOME")?.join(".local/share/agentdust"))
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
