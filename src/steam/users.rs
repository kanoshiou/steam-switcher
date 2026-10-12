//! The `SteamUser` model and account listing against a live Steam install.

use std::path::PathBuf;

use crate::steam::discover::SteamInstall;
use crate::steam::vdf::{self, VdfError};

/// One account remembered by Steam on this machine (one block in `loginusers.vdf`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteamUser {
    /// SteamID64 of the account, e.g. `76561198000000001`.
    pub steam_id64: String,
    /// Login name (what the registry `AutoLoginUser` must be set to).
    pub account_name: String,
    /// Nickname shown in Steam (what the tray menu displays).
    pub persona_name: String,
    /// Whether this was the last account logged in (vdf `MostRecent`).
    pub most_recent: bool,
    /// Whether Steam stores a password credential for passwordless login.
    pub remember_password: bool,
    /// Unix timestamp of the last login; used for stable menu ordering.
    pub timestamp: u64,
    /// Path of the locally cached avatar PNG (`config/avatarcache/<id64>.png`),
    /// if Steam has one on disk. Rendering falls back to a letter tile otherwise.
    pub avatar: Option<PathBuf>,
    /// Fields we don't model, preserved verbatim so write-back keeps them.
    pub extra_fields: Vec<(String, String)>,
}

impl SteamUser {
    /// Human-facing label for the tray menu: persona name, falling back to the
    /// account name when Steam never learned a nickname.
    pub fn display_name(&self) -> &str {
        if self.persona_name.is_empty() {
            &self.account_name
        } else {
            &self.persona_name
        }
    }

    /// Path of the locally cached avatar PNG for this account, if Steam has one.
    pub fn avatar_path(&self, steam: &SteamInstall) -> Option<PathBuf> {
        let path = steam.avatarcache_dir().join(format!("{}.png", self.steam_id64));
        path.is_file().then_some(path)
    }
}

/// Load all accounts from the live `config/loginusers.vdf`, resolving each
/// account's locally cached avatar PNG (if Steam has one on disk).
pub fn load_users(steam: &SteamInstall) -> Result<Vec<SteamUser>, VdfError> {
    let path = loginusers_path(steam);
    let text = std::fs::read_to_string(&path)?;
    let mut users = vdf::parse_users(&text)?;
    let cache_dir = steam.avatarcache_dir();
    for user in &mut users {
        let candidate = cache_dir.join(format!("{}.png", user.steam_id64));
        if candidate.is_file() {
            user.avatar = Some(candidate);
        }
    }
    Ok(users)
}

/// Path of `loginusers.vdf` under the given Steam install.
pub fn loginusers_path(steam: &SteamInstall) -> PathBuf {
    steam.loginusers_vdf()
}

/// The currently active account (`MostRecent=1`), if any.
pub fn active_user(users: &[SteamUser]) -> Option<&SteamUser> {
    users.iter().find(|u| u.most_recent)
}

/// Accounts ordered the way Steam shows them: most recently used first.
pub fn sorted_by_recency(mut users: Vec<SteamUser>) -> Vec<SteamUser> {
    users.sort_by(|a, b| b.timestamp.cmp(&a.timestamp).then(a.steam_id64.cmp(&b.steam_id64)));
    users
}
