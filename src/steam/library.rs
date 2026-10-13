//! Read the locally installed game list (no network, no Steam Web API —
//! see docs/research-game-list.md):
//!
//! 1. `steamapps/libraryfolders.vdf` → every library partition path
//! 2. `<library>/steamapps/appmanifest_<AppID>.acf` → appid, name, install state
//! 3. Cover art from the client's local caches (`resolve_cover`)

use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::steam::discover::SteamInstall;

/// One installed game as shown in the games page grid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteamGame {
    /// Steam AppID (`steam://rungameid/<appid>`).
    pub appid: u32,
    /// Display name straight from the app manifest.
    pub name: String,
    /// Client-cached cover art, when present.
    pub grid_image: Option<PathBuf>,
    /// Unix timestamp of the last play session (0 = never played).
    pub last_played: u64,
}

/// Errors while reading the local game library.
#[derive(Debug, Error)]
pub enum LibraryError {
    /// `libraryfolders.vdf` could not be parsed.
    #[error("failed to parse libraryfolders.vdf: {0}")]
    Parse(#[from] Box<keyvalues_parser::error::Error>),
    /// Filesystem access failed.
    #[error("library io failure: {0}")]
    Io(#[from] std::io::Error),
}

/// List every installed game across all library partitions, sorted by name.
pub fn installed_games(steam: &SteamInstall) -> Result<Vec<SteamGame>, LibraryError> {
    let mut games = Vec::new();
    for library in library_paths(steam)? {
        let steamapps = library.join("steamapps");
        let Ok(entries) = std::fs::read_dir(&steamapps) else {
            continue;
        };
        for entry in entries.flatten() {
            let file_name = entry.file_name().to_string_lossy().into_owned();
            let Some(appid) = file_name
                .strip_prefix("appmanifest_")
                .and_then(|rest| rest.strip_suffix(".acf"))
                .and_then(|digits| digits.parse::<u32>().ok())
            else {
                continue;
            };
            let Ok(text) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            if let Some(game) = parse_app_manifest(&text, appid, steam) {
                games.push(game);
            }
        }
    }
    games.sort_by_key(|game| game.name.to_lowercase());
    games.dedup_by(|a, b| a.appid == b.appid);
    Ok(games)
}

/// Library partition roots from `libraryfolders.vdf` (the main install included).
fn library_paths(steam: &SteamInstall) -> Result<Vec<PathBuf>, LibraryError> {
    let vdf_path = steam.root.join("steamapps").join("libraryfolders.vdf");
    let text = std::fs::read_to_string(&vdf_path).unwrap_or_default();
    let mut paths = vec![steam.root.clone()];

    if !text.is_empty() {
        let partial = keyvalues_parser::parse(&text).map_err(Box::new)?;
        if let Some(folders) = partial.value.get_obj() {
            for values in folders.0.values() {
                let Some(folder) = values.first().and_then(|v| v.get_obj()) else {
                    continue;
                };
                if let Some(path) = folder
                    .0
                    .get("path")
                    .and_then(|v| v.first())
                    .and_then(|v| v.get_str())
                {
                    paths.push(PathBuf::from(path.replace("\\\\", "\\")));
                }
            }
        }
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

/// Parse one `appmanifest_<AppID>.acf` body into a game entry.
///
/// Returns `None` for manifests without a usable name (e.g. pending install
/// stubs) so the grid only shows real entries.
fn parse_app_manifest(text: &str, appid: u32, steam: &SteamInstall) -> Option<SteamGame> {
    let partial = keyvalues_parser::parse(text).ok()?;
    let fields = partial.value.get_obj()?;

    let name = fields
        .0
        .get("name")
        .and_then(|v| v.first())
        .and_then(|v| v.get_str())?
        .to_string();
    // StateFlags bit 4 = fully installed; show anything with a name either way,
    // Steam itself launches/updates via the rungameid protocol.
    let last_played = fields
        .0
        .get("LastPlayed")
        .and_then(|v| v.first())
        .and_then(|v| v.get_str())
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);

    let grid_image = resolve_cover(steam, appid);

    Some(SteamGame {
        appid,
        name,
        grid_image,
        last_played,
    })
}

/// Resolve the best locally cached cover art for `appid`, first hit wins.
///
/// The modern client (2023 UI) keeps library art in `<Steam>/appcache/
/// librarycache/<AppID>/`: `header*.jpg` (460×215 landscape, matching the
/// games grid card), `library_600x900*.jpg` (portrait box art), plus
/// `library_header` / `library_hero` / `library_capsule` variants — often as
/// locale-suffixed files (e.g. `header_schinese.jpg`) or inside content-hash
/// subdirectories. Older clients and non-Steam shortcuts use
/// `<Steam>/config/grid/<AppID>.png` and
/// `<Steam>/steamapps/librarycache/<AppID>_*.jpg`.
fn resolve_cover(steam: &SteamInstall, appid: u32) -> Option<PathBuf> {
    let root = &steam.root;

    // Legacy grid cache; still where non-Steam shortcut art lives.
    let grid = root.join("config").join("grid").join(format!("{appid}.png"));
    if grid.is_file() {
        return Some(grid);
    }

    let cache_dir = root
        .join("appcache")
        .join("librarycache")
        .join(appid.to_string());
    if let Some(cover) = cover_from_cache_dir(&cache_dir) {
        return Some(cover);
    }

    // Pre-2023 client layout: flat files under steamapps/librarycache.
    let legacy = root.join("steamapps").join("librarycache");
    for name in [
        format!("{appid}_header.jpg"),
        format!("{appid}_library_600x900.jpg"),
    ] {
        let path = legacy.join(name);
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

/// Search one `appcache/librarycache/<AppID>` directory — its own files plus
/// one level of content-hash subdirectories — and return the best cover.
/// Asset type decides first (landscape header, then portrait box art, then
/// hero/capsule images), name length second so the exact international name
/// beats locale variants; blurred heroes are a deliberate last resort.
fn cover_from_cache_dir(dir: &Path) -> Option<PathBuf> {
    /// Sort key of a cover file name: asset-type rank, then name length.
    fn rank(name: &str) -> Option<(u8, usize)> {
        let lower = name.to_lowercase();
        if !(lower.ends_with(".jpg") || lower.ends_with(".png")) {
            return None;
        }
        let rank = if lower.starts_with("header") {
            0
        } else if lower.starts_with("library_600x900") {
            1
        } else if lower.starts_with("library_header") {
            2
        } else if lower.starts_with("library_hero_blur") {
            4
        } else if lower.starts_with("library_hero") {
            3
        } else if lower.starts_with("library_capsule") {
            5
        } else {
            return None;
        };
        Some((rank, lower.len()))
    }

    let mut best: Option<((u8, usize), PathBuf)> = None;
    let consider = |path: PathBuf, best: &mut Option<((u8, usize), PathBuf)>| {
        let Some(key) = path.file_name().and_then(|n| n.to_str()).and_then(rank) else {
            return;
        };
        if best.as_ref().is_none_or(|(known, _)| key < *known) {
            *best = Some((key, path));
        }
    };

    let Ok(entries) = std::fs::read_dir(dir) else {
        return None;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if file_type.is_file() {
            consider(path, &mut best);
        } else if file_type.is_dir()
            && let Ok(inner) = std::fs::read_dir(&path)
        {
            for file in inner.flatten() {
                if file.file_type().map(|t| t.is_file()).unwrap_or(false) {
                    consider(file.path(), &mut best);
                }
            }
        }
    }
    best.map(|(_, path)| path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"
"AppState"
{
	"appid"		"1091500"
	"Universe"		"1"
	"name"		"Cyberpunk 2077"
	"StateFlags"		"4"
	"installdir"		"Cyberpunk 2077"
	"LastPlayed"		"1717000000"
	"SizeOnDisk"		"70352336128"
}
"#;

    #[test]
    fn parse_manifest_fields() {
        let steam = SteamInstall { root: PathBuf::from("C:/Steam") };
        let game = parse_app_manifest(MANIFEST, 1091500, &steam).expect("manifest parses");
        assert_eq!(game.appid, 1091500);
        assert_eq!(game.name, "Cyberpunk 2077");
        assert_eq!(game.last_played, 1_717_000_000);
        // The grid cache file does not exist in the test environment.
        assert_eq!(game.grid_image, None);
    }

    #[test]
    fn manifest_without_name_is_skipped() {
        let steam = SteamInstall { root: PathBuf::from("C:/Steam") };
        let broken = r#""AppState" { "appid" "1" "StateFlags" "4" }"#;
        assert!(parse_app_manifest(broken, 1, &steam).is_none());
    }

    /// Build a fake Steam root under the OS temp dir with the given files.
    fn fake_steam(files: &[(&str, &str)]) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "steam-switcher-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        for (rel, _) in files {
            std::fs::create_dir_all(root.join(rel).parent().unwrap()).unwrap();
        }
        for (rel, content) in files {
            std::fs::write(root.join(rel), content).unwrap();
        }
        root
    }

    #[test]
    fn cover_prefers_header_over_locale_and_boxart() {
        let appid = 730u32;
        let root = fake_steam(&[
            (
                &format!("appcache/librarycache/{appid}/header_schinese.jpg"),
                "junk",
            ),
            (
                &format!("appcache/librarycache/{appid}/library_600x900_schinese.jpg"),
                "junk",
            ),
        ]);
        let cover = resolve_cover(&SteamInstall { root: root.clone() }, appid).unwrap();
        assert_eq!(
            cover,
            root.join(format!("appcache/librarycache/{appid}/header_schinese.jpg"))
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn cover_falls_back_to_hashed_subdir_boxart() {
        let appid = 578080u32;
        let root = fake_steam(&[(
            &format!("appcache/librarycache/{appid}/abc/library_600x900.jpg"),
            "junk",
        )]);
        let cover = resolve_cover(&SteamInstall { root: root.clone() }, appid).unwrap();
        assert_eq!(
            cover,
            root.join(format!("appcache/librarycache/{appid}/abc/library_600x900.jpg"))
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn cover_falls_back_to_legacy_cache_and_grid() {
        let appid = 431960u32;
        let root = fake_steam(&[(&format!("steamapps/librarycache/{appid}_header.jpg"), "junk")]);
        let cover = resolve_cover(&SteamInstall { root: root.clone() }, appid).unwrap();
        assert_eq!(
            cover,
            root.join(format!("steamapps/librarycache/{appid}_header.jpg"))
        );
        std::fs::remove_dir_all(root).ok();

        let root = fake_steam(&[(&format!("config/grid/{appid}.png"), "junk")]);
        let cover = resolve_cover(&SteamInstall { root: root.clone() }, appid).unwrap();
        assert_eq!(cover, root.join(format!("config/grid/{appid}.png")));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn cover_picks_subdir_header_over_top_level_blur() {
        let appid = 1172470u32;
        let root = fake_steam(&[
            (&format!("appcache/librarycache/{appid}/library_hero_blur.jpg"), "junk"),
            (
                &format!("appcache/librarycache/{appid}/abc/library_header.jpg"),
                "junk",
            ),
        ]);
        let cover = resolve_cover(&SteamInstall { root: root.clone() }, appid).unwrap();
        assert_eq!(
            cover,
            root.join(format!("appcache/librarycache/{appid}/abc/library_header.jpg"))
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn cover_none_when_missing() {
        let root = fake_steam(&[]);
        assert!(resolve_cover(&SteamInstall { root: root.clone() }, 1).is_none());
        std::fs::remove_dir_all(root).ok();
    }
}
