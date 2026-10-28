//! Signed release metadata and downloads, independent of application windows.
pub mod download;
pub mod manifest;
pub mod windows;

use anyhow::{Context as _, Result};
use std::path::PathBuf;

pub const FEED_URL: &str =
    "https://github.com/zephyraluco/alacrterm/releases/latest/download/update.json";
pub const DOWNLOAD_PAGE: &str = "https://github.com/zephyraluco/alacrterm/releases/latest";
pub const APPCAST_URL: &str =
    "https://github.com/zephyraluco/alacrterm/releases/latest/download/appcast.xml";

pub fn platform_key() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Some("windows-x64"),
        ("linux", "x86_64") => Some("linux-x64"),
        _ => None,
    }
}

pub fn trusted_keys() -> Vec<String> {
    option_env!("ALACRTERM_UPDATE_PUBLIC_KEYS")
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

pub fn cache_dir() -> Result<PathBuf> {
    Ok(dirs::cache_dir()
        .context("找不到用户缓存目录")?
        .join("Alacrterm/updates"))
}

pub fn data_dir() -> Result<PathBuf> {
    Ok(dirs::data_local_dir()
        .context("找不到用户数据目录")?
        .join("Alacrterm/updates"))
}
