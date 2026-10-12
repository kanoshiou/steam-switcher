//! steam-switcher library root.
//!
//! Module map (see docs/plan.md for the phased roadmap):
//! - [`steam`]: Steam interaction core (discovery, loginusers.vdf, switching)
//! - [`app`]:   application bootstrap and the tray/hotkey event loop
//! - [`tray`]:  tray icon and context menu
//! - [`singleton`], [`autostart`], [`winutil`], [`hotkey`], [`notify`], [`logging`]:
//!   OS-integration helpers

pub mod autostart;
pub mod config;
pub mod hotkey;
pub mod logging;
pub mod notify;
pub mod singleton;
pub mod steam;
pub mod tray;

/// Windows-specific helpers (message box, win32 message pump).
#[cfg(windows)]
pub mod winutil;

/// GUI application bootstrap and event loop (gpui + tray bridging).
#[cfg(windows)]
pub mod app;
#[cfg(windows)]
pub mod settings;

/// Non-Windows builds have no tray/GUI yet (Windows-first; see docs/plan.md).
#[cfg(not(windows))]
pub mod app {
    /// Stub entry point so the crate still checks on other platforms.
    pub fn run() -> anyhow::Result<()> {
        eprintln!("steam-switcher: tray GUI is Windows-only in this phase");
        Ok(())
    }
}
