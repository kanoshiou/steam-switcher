//! steam-switcher library root.
//!
//! Module map (see docs/plan.md for the phased roadmap):
//! - [`steam`]: Steam interaction core (discovery, loginusers.vdf, switching)
//! - [`app`]:   application bootstrap and the tray/hotkey event loop
//! - [`tray`]:  tray icon and context menu
//! - [`singleton`], [`autostart`], [`winutil`], [`hotkey`], [`notify`], [`logging`]:
//!   OS-integration helpers

pub mod app;
pub mod autostart;
pub mod hotkey;
pub mod logging;
pub mod notify;
pub mod singleton;
pub mod steam;
pub mod tray;

/// Windows-specific helpers (message box, win32 message pump).
#[cfg(windows)]
pub mod winutil;
