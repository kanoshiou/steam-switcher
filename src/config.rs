//! Application configuration persisted to `%APPDATA%\steam-switcher\config.json`.
//!
//! The file on disk is the source of truth: switch flows re-read it so a
//! settings-window edit takes effect on the very next switch without any
//! in-memory synchronization.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Persisted user preferences.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// Per-account display aliases keyed by SteamID64. An empty value (or a
    /// missing entry) means "show the PersonaName".
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub aliases: BTreeMap<String, String>,
    /// Ask before quitting Steam while games are running (default on; the
    /// dialog itself must not be skippable when enabled).
    #[serde(default = "default_true")]
    pub confirm_before_switch: bool,
    /// UI language: `en`, `zh-CN`, `ja`, `ko`. `None` follows the system locale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// UI theme: `system`, `dark` or `light`. `None` follows the system.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
}

fn default_true() -> bool {
    true
}

impl Config {
    /// Load the config, falling back to defaults when the file is missing,
    /// unreadable or invalid — a corrupt config must never block startup.
    pub fn load() -> Config {
        let path = config_path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Config::default();
        };
        match serde_json::from_str(&text) {
            Ok(config) => config,
            Err(err) => {
                tracing::warn!(%err, path = %path.display(), "invalid config.json, using defaults");
                Config::default()
            }
        }
    }

    /// Persist the config atomically (write to a sibling temp file, then rename).
    pub fn save(&self) -> anyhow::Result<()> {
        let path = config_path();
        std::fs::create_dir_all(path.parent().expect("config path has a parent"))?;
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, serde_json::to_string_pretty(self)?)?;
        std::fs::rename(&temp, &path)?;
        tracing::debug!(path = %path.display(), "config saved");
        Ok(())
    }

    /// The name to show for `user` in menus: alias when set, else the usual
    /// persona/account display name.
    pub fn display_for(&self, user: &crate::steam::users::SteamUser) -> String {
        match self.aliases.get(&user.steam_id64) {
            Some(alias) if !alias.trim().is_empty() => alias.trim().to_string(),
            _ => user.display_name().to_string(),
        }
    }

    /// The UI language to use: configured value, else the system locale mapped
    /// to one of the supported languages (`en`, `zh-CN`, `ja`, `ko`).
    pub fn language(&self) -> &'static str {
        match self.language.as_deref() {
            Some("zh-CN") => "zh-CN",
            Some("ja") => "ja",
            Some("ko") => "ko",
            Some("en") => "en",
            _ => detect_language(),
        }
    }

    /// The UI theme to use: configured value, else `system`.
    pub fn theme(&self) -> &'static str {
        match self.theme.as_deref() {
            Some("dark") => "dark",
            Some("light") => "light",
            _ => "system",
        }
    }
}

/// Map the system locale to the closest supported UI language.
fn detect_language() -> &'static str {
    match sys_locale::get_locale().as_deref() {
        Some(locale) if locale.starts_with("zh") => "zh-CN",
        Some(locale) if locale.starts_with("ja") => "ja",
        Some(locale) if locale.starts_with("ko") => "ko",
        _ => "en",
    }
}

/// `%APPDATA%\steam-switcher\config.json` (falls back to `./config.json` off-Windows).
pub fn config_path() -> PathBuf {
    #[cfg(windows)]
    if let Some(appdata) = std::env::var_os("APPDATA") {
        return PathBuf::from(appdata).join("steam-switcher").join("config.json");
    }
    PathBuf::from("config.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_survive_round_trip() {
        let config = Config::default();
        let text = serde_json::to_string(&config).unwrap();
        assert_eq!(serde_json::from_str::<Config>(&text).unwrap(), config);
    }

    #[test]
    fn display_for_prefers_alias_and_falls_back() {
        let mut config = Config::default();
        config.aliases.insert("765".into(), "小号".into());
        let user = crate::steam::users::SteamUser {
            steam_id64: "765".into(),
            account_name: "acc".into(),
            persona_name: "Persona".into(),
            most_recent: false,
            remember_password: false,
            timestamp: 0,
            extra_fields: vec![],
        };
        assert_eq!(config.display_for(&user), "小号");
        config.aliases.insert("765".into(), "   ".into());
        assert_eq!(config.display_for(&user), "Persona");
    }
}
