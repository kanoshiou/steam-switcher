//! Game launching via the `steam://rungameid/<AppID>` URL scheme.
//!
//! Handing the request to Steam lets the client deal with not-running-yet,
//! updates and login queuing itself (docs/research-game-list.md).

use std::process::Command;

/// Ask Steam to launch the game with the given AppID.
///
/// Uses `explorer <url>` — no FFI, and the URL scheme is opened by the shell
/// even when Steam is mid-restart (it queues the request internally).
pub fn launch_game(appid: u32) -> std::io::Result<()> {
    let url = format!("steam://rungameid/{appid}");
    tracing::info!(%url, "launching game via steam protocol");
    Command::new("explorer").arg(&url).spawn()?;
    Ok(())
}
