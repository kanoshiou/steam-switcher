//! Locate the local Steam installation.
//!
//! Strategy (Windows first): read `SteamPath` from the registry, then fall back
//! to probing well-known install locations. Every candidate must contain the
//! Steam executable to be accepted.

use std::path::{Path, PathBuf};
use thiserror::Error;/// Where Steam is installed, plus derived paths used by the rest of the app.
#[derive(Debug, Clone)]
pub struct SteamInstall {
    /// Steam root directory (registry `SteamPath`, e.g. `C:\Program Files (x86)\Steam`).
    pub root: PathBuf,
}

impl SteamInstall {
    /// Path to the Steam launcher executable.
    pub fn exe(&self) -> PathBuf {
        #[cfg(windows)]
        {
            self.root.join("steam.exe")
        }
        #[cfg(not(windows))]
        {
            self.root.join("steam.sh")
        }
    }

    /// Path to `config/loginusers.vdf` — the account list Steam persists locally.
    pub fn loginusers_vdf(&self) -> PathBuf {
        self.root.join("config").join("loginusers.vdf")
    }

    /// Directory where Steam caches per-account avatars as `<steam_id64>.png`.
    pub fn avatarcache_dir(&self) -> PathBuf {
        self.root.join("config").join("avatarcache")
    }
}

/// Errors while locating Steam.
#[derive(Debug, Error)]
pub enum DiscoverError {
    /// No candidate directory contained a Steam installation.
    #[error("Steam installation not found (registry and common paths all failed)")]
    NotFound,
    /// Unexpected I/O failure during probing.
    #[error("failed while probing for Steam: {0}")]
    Io(#[from] std::io::Error),
}

/// Find the Steam installation, preferring the registry value.
pub fn discover() -> Result<SteamInstall, DiscoverError> {
    for candidate in candidates() {
        let root = PathBuf::from(candidate);
        if is_steam_root(&root) {
            tracing::debug!(?root, "found Steam installation");
            return Ok(SteamInstall { root });
        }
    }
    Err(DiscoverError::NotFound)
}

fn is_steam_root(root: &Path) -> bool {
    // The executable is the only reliable marker across install shapes.
    root.join(if cfg!(windows) { "steam.exe" } else { "steam.sh" })
        .is_file()
}

/// Candidate Steam roots in priority order; the first whose directory contains
/// the Steam executable wins.
fn candidates() -> Vec<String> {
    let mut out = Vec::new();

    #[cfg(windows)]
    if let Some(path) = registry_steam_path() {
        out.push(path);
    }

    if let Some(pf86) = std::env::var_os("ProgramFiles(x86)") {
        out.push(PathBuf::from(pf86).join("Steam").to_string_lossy().into_owned());
    }
    if let Some(pf) = std::env::var_os("ProgramFiles") {
        out.push(PathBuf::from(pf).join("Steam").to_string_lossy().into_owned());
    }
    if let Some(lad) = std::env::var_os("LOCALAPPDATA") {
        out.push(PathBuf::from(lad).join("Steam").to_string_lossy().into_owned());
    }

    #[cfg(not(windows))]
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        out.push(home.join(".steam").join("steam").to_string_lossy().into_owned());
        out.push(home.join(".local/share/Steam").to_string_lossy().into_owned());
        out.push(
            home.join("Library/Application Support/Steam")
                .to_string_lossy()
                .into_owned(),
        );
    }

    out
}

/// Read `SteamPath` from `HKCU\Software\Valve\Steam`.
#[cfg(windows)]
fn registry_steam_path() -> Option<String> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu.open_subkey("Software\\Valve\\Steam").ok()?;
    let path: String = key.get_value("SteamPath").ok()?;
    tracing::debug!(path = %path, "registry SteamPath found");
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_are_non_empty() {
        assert!(!candidates().is_empty());
    }
}
