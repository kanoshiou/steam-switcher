//! `loginusers.vdf` parsing, mutation and lossless-enough write-back.
//!
//! Parse uses `keyvalues-parser`; write-back re-serializes from our own model.
//! Per AGENTS.md D1, the write-back does not preserve comments or original
//! formatting — Steam only reads key/value pairs, which is our acceptance bar.
//! Unknown per-user fields are carried through [`SteamUser::extra_fields`] so
//! nothing Steam wrote is silently dropped.

use std::path::Path;

use thiserror::Error;

use crate::steam::users::SteamUser;

/// Fixtures used by the unit tests at the bottom of this module.
#[cfg(test)]
const FIXTURE: &str = include_str!("../../tests/fixtures/loginusers.vdf");

/// Errors produced while reading or writing `loginusers.vdf`.
#[derive(Debug, Error)]
pub enum VdfError {
    /// The file is not valid VDF text. Boxed: pest errors are large, and this
    /// error type rides inside every fallible vdf call's return value.
    #[error("failed to parse loginusers.vdf: {0}")]
    Parse(#[from] Box<keyvalues_parser::error::Error>),
    /// The file parsed but did not have the expected `users { <id> { ... } }` shape.
    #[error("loginusers.vdf has unexpected structure: {0}")]
    Shape(&'static str),
    /// Filesystem failure (read, backup or write).
    #[error("loginusers.vdf io failure: {0}")]
    Io(#[from] std::io::Error),
}

/// Parse the contents of a `loginusers.vdf` file into account models.
pub fn parse_users(text: &str) -> Result<Vec<SteamUser>, VdfError> {
    let partial = keyvalues_parser::parse(text).map_err(Box::new)?;
    let obj = partial
        .value
        .get_obj()
        .ok_or(VdfError::Shape("top level value is not an object"))?;

    let mut users = Vec::new();
    for (steam_id64, values) in &obj.0 {
        // Every key maps to a Vec (the parser's multi-map); account blocks have
        // exactly one value, an object of field pairs.
        let Some(fields) = values.first().and_then(|v| v.get_obj()) else {
            tracing::warn!(%steam_id64, "skipping non-object user block");
            continue;
        };

        let mut user = SteamUser {
            steam_id64: steam_id64.to_string(),
            account_name: String::new(),
            persona_name: String::new(),
            most_recent: false,
            remember_password: false,
            timestamp: 0,
            avatar: None,
            extra_fields: Vec::new(),
        };

        for (key, field_values) in &fields.0 {
            let Some(value) = field_values.first().and_then(|v| v.get_str()) else {
                continue;
            };
            match key.as_ref() {
                "AccountName" => user.account_name = value.to_string(),
                "PersonaName" => user.persona_name = value.to_string(),
                "MostRecent" => user.most_recent = value == "1",
                "RememberPassword" => user.remember_password = value == "1",
                "Timestamp" => user.timestamp = value.parse().unwrap_or(0),
                // Preserve anything we don't model so write-back keeps it.
                _ => user.extra_fields.push((key.to_string(), value.to_string())),
            }
        }
        users.push(user);
    }
    Ok(users)
}

/// Serialize users back to VDF text in the canonical Steam layout (tabs, quoted strings).
pub fn serialize_users(users: &[SteamUser]) -> String {
    let mut out = String::from("\"users\"\n{\n");
    for user in users {
        out.push_str(&format!("\t\"{}\"\n\t{{\n", user.steam_id64));
        let mut field = |k: &str, v: &str| {
            out.push_str(&format!("\t\t\"{}\"\t\"{}\"\n", k, escape(v)));
        };
        field("AccountName", &user.account_name);
        field("PersonaName", &user.persona_name);
        field("RememberPassword", if user.remember_password { "1" } else { "0" });
        field("MostRecent", if user.most_recent { "1" } else { "0" });
        field("Timestamp", &user.timestamp.to_string());
        for (k, v) in &user.extra_fields {
            field(k, v);
        }
        out.push_str("\t}\n");
    }
    out.push_str("}\n");
    out
}

/// Escape a value for inclusion in a quoted VDF string (inverse of the parser's unescaping).
fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out
}

/// Mark `steam_id64` as the active account: `MostRecent=1` + `RememberPassword=1`
/// on the target, `MostRecent=0` everywhere else.
///
/// Returns `false` — leaving the list untouched — if no user with that id exists.
pub fn set_active(users: &mut [SteamUser], steam_id64: &str) -> bool {
    if !users.iter().any(|u| u.steam_id64 == steam_id64) {
        return false;
    }
    for user in users.iter_mut() {
        if user.steam_id64 == steam_id64 {
            user.most_recent = true;
            user.remember_password = true;
        } else {
            user.most_recent = false;
        }
    }
    true
}

/// Back up `path` (keeping the latest [`MAX_BACKUPS`]) and write `text` to it.
pub fn write_users(path: &Path, users: &[SteamUser]) -> Result<(), VdfError> {
    backup_existing(path)?;
    std::fs::write(path, serialize_users(users))?;
    Ok(())
}

/// How many timestamped `.bak` files to keep around the live vdf.
const MAX_BACKUPS: usize = 3;

/// Copy the current file to `loginusers.vdf.bak-YYYYMMDD-HHMMSS`, then prune
/// old backups beyond [`MAX_BACKUPS`]. Missing file is a no-op.
fn backup_existing(path: &Path) -> Result<(), VdfError> {
    if !path.is_file() {
        return Ok(());
    }
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let backup = path.with_file_name(format!(
        "{}.bak-{}",
        path.file_name()
            .unwrap_or_default()
            .to_string_lossy(),
        stamp
    ));
    std::fs::copy(path, &backup)?;
    tracing::info!(backup = %backup.display(), "vdf backed up");
    prune_backups(path)?;
    Ok(())
}

/// Delete the oldest `*.bak-*` siblings of `path`, keeping [`MAX_BACKUPS`].
fn prune_backups(path: &Path) -> Result<(), VdfError> {
    let dir = path.parent().ok_or(VdfError::Shape("vdf path has no parent"))?;
    let stem = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();

    let mut backups: Vec<_> = std::fs::read_dir(dir)?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.strip_prefix(&format!("{stem}.bak-")).map(|stamp| (stamp.to_string(), entry.path()))
        })
        .collect();
    // Newest stamp first (zero-padded YYYYMMDD-HHMMSS sorts lexicographically).
    backups.sort_by(|a, b| b.0.cmp(&a.0));

    for (_, stale) in backups.into_iter().skip(MAX_BACKUPS) {
        tracing::debug!(backup = %stale.display(), "pruning old vdf backup");
        std::fs::remove_file(stale)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_users() -> Vec<SteamUser> {
        parse_users(FIXTURE).expect("fixture must parse")
    }

    #[test]
    fn parse_fixture_yields_both_accounts() {
        let users = fixture_users();
        assert_eq!(users.len(), 2);

        let a = users.iter().find(|u| u.steam_id64 == "76561198000000001").unwrap();
        assert_eq!(a.account_name, "player_a");
        assert_eq!(a.persona_name, "PlayerA");
        assert!(a.most_recent);
        assert!(a.remember_password);
        assert_eq!(a.timestamp, 1_710_000_000);
        // Fields we don't model must survive in extra_fields.
        assert!(a.extra_fields.iter().any(|(k, v)| k == "WantsOfflineMode" && v == "0"));

        let b = users.iter().find(|u| u.steam_id64 == "76561198000000002").unwrap();
        assert_eq!(b.persona_name, "玩家\"B\"");
        assert!(!b.most_recent);
        assert!(!b.remember_password);
    }

    #[test]
    fn set_active_flips_most_recent() {
        let mut users = fixture_users();
        assert!(set_active(&mut users, "76561198000000002"));
        assert!(!set_active(&mut users, "999"));

        let a = users.iter().find(|u| u.steam_id64 == "76561198000000001").unwrap();
        let b = users.iter().find(|u| u.steam_id64 == "76561198000000002").unwrap();
        assert!(!a.most_recent);
        assert!(b.most_recent);
        // Activating also forces RememberPassword so the switch stays passwordless.
        assert!(b.remember_password);
    }

    #[test]
    fn serialize_round_trip_preserves_users() {
        let mut users = fixture_users();
        set_active(&mut users, "76561198000000002");
        let reparsed = parse_users(&serialize_users(&users)).expect("our own output must parse");
        assert_eq!(users, reparsed);
    }

    #[test]
    fn escaping_survives_round_trip() {
        let user = SteamUser {
            steam_id64: "76561198000000009".into(),
            account_name: "back\\slash".into(),
            persona_name: "quote\" and 空格".into(),
            most_recent: false,
            remember_password: true,
            timestamp: 42,
            avatar: None,
            extra_fields: vec![("Custom".into(), "va\"l\\ue".into())],
        };
        let text = serialize_users(std::slice::from_ref(&user));
        assert!(text.contains("\"quote\\\" and 空格\""));
        let parsed = parse_users(&text).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0], user);
    }

    #[test]
    fn garbage_input_is_an_error() {
        assert!(parse_users("definitely { not vdf").is_err());
    }
}
