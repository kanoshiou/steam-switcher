//! Tray icon and the dynamically built context menu.
//!
//! Menu layout (docs/plan.md P1): account list (check mark on the active one),
//! separator, Refresh, Start-with-Windows toggle, Settings placeholder, Quit.
//!
//! Menu item ids are structured strings (`acct:<steam_id64>`, `refresh`, ...)
//! so [`parse_action`] can turn a [`MenuId`] back into an [`Action`] without a
//! side table — the id *is* the mapping.

use tray_icon::menu::{CheckMenuItem, Menu, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use crate::steam::users::{self, SteamUser};

/// Prefix for per-account menu item ids.
pub const ACCOUNT_PREFIX: &str = "acct:";

/// What the user picked from the tray menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Switch to the account with this SteamID64.
    Switch(String),
    /// Re-read `loginusers.vdf` and rebuild the menu.
    Refresh,
    /// Toggle the "start with Windows" Run-key entry.
    ToggleAutostart,
    /// Settings window placeholder (Phase 2).
    Settings,
    /// Exit the app.
    Quit,
}

/// The tray icon handle; rebuilds its menu whenever accounts change.
pub struct Tray {
    icon: TrayIcon,
}

impl Tray {
    /// Create the tray icon with an initial menu.
    pub fn new(users: &[SteamUser], autostart_enabled: bool) -> anyhow::Result<Self> {
        let icon = TrayIconBuilder::new()
            .with_id("steam-switcher")
            .with_icon(default_icon()?)
            .with_tooltip("Steam Switcher")
            .with_menu(Box::new(build_menu(users, autostart_enabled)))
            .build()?;
        Ok(Self { icon })
    }

    /// Replace the context menu (after a switch, refresh or autostart toggle).
    pub fn update_menu(&self, users: &[SteamUser], autostart_enabled: bool) {
        self.icon.set_menu(Some(Box::new(build_menu(users, autostart_enabled))));
    }
}

/// Map a fired menu item id back to the action it represents.
pub fn parse_action(id: &MenuId) -> Option<Action> {
    let raw = id.0.as_str();
    if let Some(steam_id64) = raw.strip_prefix(ACCOUNT_PREFIX) {
        return Some(Action::Switch(steam_id64.to_string()));
    }
    match raw {
        "refresh" => Some(Action::Refresh),
        "autostart" => Some(Action::ToggleAutostart),
        "settings" => Some(Action::Settings),
        "quit" => Some(Action::Quit),
        _ => None,
    }
}

/// Build the context menu from the current account list.
fn build_menu(users: &[SteamUser], autostart_enabled: bool) -> Menu {
    let menu = Menu::new();

    if users.is_empty() {
        let _ = menu.append(&MenuItem::with_id(
            "no-accounts",
            "No Steam accounts found",
            false,
            None,
        ));
    }
    for user in users::sorted_by_recency(users.to_vec()) {
        // Show the persona name; disambiguate with the account name when they differ.
        let label = if !user.account_name.is_empty() && user.account_name != user.persona_name {
            format!("{} ({})", user.display_name(), user.account_name)
        } else {
            user.display_name().to_string()
        };
        let _ = menu.append(&CheckMenuItem::with_id(
            format!("{ACCOUNT_PREFIX}{}", user.steam_id64),
            label,
            true,
            user.most_recent,
            None,
        ));
    }
    if !users.is_empty() {
        let _ = menu.append(&PredefinedMenuItem::separator());
    }

    let _ = menu.append(&MenuItem::with_id("refresh", "Refresh", true, None));
    let _ = menu.append(&CheckMenuItem::with_id(
        "autostart",
        "Start with Windows",
        true,
        autostart_enabled,
        None,
    ));
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&MenuItem::with_id(
        "settings",
        "Settings (coming in Phase 2)",
        false,
        None,
    ));
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&MenuItem::with_id("quit", "Quit", true, None));
    menu
}

/// Draw the app icon as straight-alpha RGBA8 at `size`: a Steam-navy disc with
/// a light-blue ring and center dot.
///
/// This is the single source of truth for the icon design — the tray uses it
/// at runtime, and `examples/gen-icon.rs` renders `assets/icon.ico` from it.
pub fn render_rgba(size: u32) -> Vec<u8> {
    // #1B2838 (Steam navy) and #66C0F4 (Steam light blue).
    const NAVY: [f32; 3] = [27.0, 40.0, 56.0];
    const BLUE: [f32; 3] = [102.0, 192.0, 244.0];
    /// Supersampling grid per pixel edge; 4x4 gives clean edges at 16px.
    const SUB: u32 = 4;

    // Geometry tuned at 32px, scaled to any size.
    let unit = size as f32 / 32.0;
    let (r_disc, r_ring_in, r_ring_out, r_dot) = (15.5 * unit, 10.0 * unit, 13.5 * unit, 5.0 * unit);
    let center = (size as f32 - 1.0) / 2.0;
    let samples = (SUB * SUB) as f32;

    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let mut blue_hits = 0u32;
            let mut navy_hits = 0u32;
            for sy in 0..SUB {
                for sx in 0..SUB {
                    let px = x as f32 + (sx as f32 + 0.5) / SUB as f32;
                    let py = y as f32 + (sy as f32 + 0.5) / SUB as f32;
                    let dx = px - center;
                    let dy = py - center;
                    let dist = (dx * dx + dy * dy).sqrt();
                    if dist <= r_dot || (r_ring_in..=r_ring_out).contains(&dist) {
                        blue_hits += 1;
                    } else if dist <= r_disc {
                        navy_hits += 1;
                    }
                }
            }
            let covered = blue_hits + navy_hits;
            let alpha = covered as f32 / samples * 255.0;
            // Blend the two tones where an edge pixel straddles both.
            let rgb = if covered == 0 {
                NAVY
            } else {
                [
                    (BLUE[0] * blue_hits as f32 + NAVY[0] * navy_hits as f32) / covered as f32,
                    (BLUE[1] * blue_hits as f32 + NAVY[1] * navy_hits as f32) / covered as f32,
                    (BLUE[2] * blue_hits as f32 + NAVY[2] * navy_hits as f32) / covered as f32,
                ]
            };
            rgba.extend_from_slice(&[rgb[0] as u8, rgb[1] as u8, rgb[2] as u8, alpha as u8]);
        }
    }
    rgba
}

/// The tray icon: the shared design rendered at the classic 32px tray size.
fn default_icon() -> Result<Icon, tray_icon::BadIcon> {
    const SIZE: u32 = 32;
    Icon::from_rgba(render_rgba(SIZE), SIZE, SIZE)
}
