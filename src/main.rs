//! steam-switcher — tray-based Steam account quick switcher.
//!
//! Phase roadmap (docs/plan.md):
//! - P0: skeleton + Steam discovery + vdf read/write (done)
//! - P1: tray menu switching + hotkey + single instance + autostart (current)
//! - P2: gpui-kit settings window
//! - P3: local game library + pick-account-to-launch

// Keep a console for logs in dev builds; release builds are windowed so
// launching from Explorer doesn't flash a terminal.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> anyhow::Result<()> {
    steam_switcher::app::run()
}
