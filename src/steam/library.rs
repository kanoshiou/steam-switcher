//! Read the locally installed game list (no network, no Steam Web API —
//! see docs/research-game-list.md):
//!
//! 1. `steamapps/libraryfolders.vdf` → every library partition path
//! 2. `<library>/steamapps/appmanifest_<AppID>.acf` → appid, name, install state
//! 3. Cover art from the client grid cache: `<Steam>/config/grid/<AppID>.png`

use std::path::PathBuf;

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

    let grid_image = steam.root.join("config").join("grid").join(format!("{appid}.png"));
    let grid_image = grid_image.is_file().then_some(grid_image);

    Some(SteamGame {
        appid,
        name,
        grid_image,
        last_played,
    })
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
}
