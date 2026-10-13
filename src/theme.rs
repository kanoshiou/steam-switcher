//! The app's Steam-flavored palette, stamped onto the active gpui-component
//! theme after every mode switch.
//!
//! gpui-component ships a neutral default theme; `apply` loads the requested
//! mode (dark/light/system) and then overwrites the semantic color tokens we
//! care about with the Steam palette from `steam_switcher.html` (dark:
//! `#171a21` background, `#1b2838` cards, `#66c0f4` primary; light: slate
//! surfaces with a stronger blue). Only the tokens listed here deviate from
//! the stock theme, so unlisted component colors stay coherent per mode.

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{px, rgb, rgba, App, Background, Hsla};

/// Steam light blue — the brand/primary accent (`#66c0f4`).
pub fn blue() -> Hsla {
    rgb(0x66c0f4).into()
}
/// Darker primary hover (`#4199c7`).
pub fn blue_hover() -> Hsla {
    rgb(0x4199c7).into()
}
/// Steam navy card color (`#1b2838`).
pub fn navy_card() -> Hsla {
    rgb(0x1b2838).into()
}
/// Selected nav/segment surface (`#223447`).
pub fn navy_active() -> Hsla {
    rgb(0x223447).into()
}

/// The green gradient of the "one-click switch" button (90deg, hover end first).
pub fn switch_button_gradient(hover: bool) -> Background {
    let (from, to): (Hsla, Hsla) = if hover {
        (rgb(0x8cb214).into(), rgb(0x688e14).into())
    } else {
        (rgb(0x79990f).into(), rgb(0x5c7e10).into())
    };
    gpui_kit::linear_gradient(
        90.0,
        gpui_kit::linear_color_stop(from, 0.0),
        gpui_kit::linear_color_stop(to, 1.0),
    )
}

/// Load `mode` and stamp the matching Steam palette onto the active theme.
pub fn apply(mode: &str, cx: &mut App) {
    match mode {
        "dark" => Theme::change(ThemeMode::Dark, None, cx),
        "light" => Theme::change(ThemeMode::Light, None, cx),
        // Follow the OS appearance.
        _ => Theme::sync_system_appearance(None, cx),
    }
    Theme::update(cx, |theme| {
        if theme.mode.is_dark() {
            stamp_dark(&mut theme.colors);
        } else {
            stamp_light(&mut theme.colors);
        }
        // Slightly rounder than stock to match the prototype's rounded-xl/2xl.
        theme.radius = px(10.0);
        theme.radius_lg = px(14.0);
    });
}

/// Dark palette: the classic immersive Steam client look.
fn stamp_dark(c: &mut gpui_kit::component::ThemeColor) {
    c.background = rgb(0x171a21).into();
    c.foreground = rgb(0xdce3ea).into();
    c.title_bar = rgb(0x101216).into();
    c.title_bar_border = rgb(0x2a475e).into();
    c.border = rgb(0x2a475e).into();
    c.input = rgba(0x101216b3).into();
    c.muted = rgb(0x1f2733).into();
    c.muted_foreground = rgb(0x8f98a0).into();

    c.primary = rgb(0x66c0f4).into();
    c.primary_foreground = rgb(0x0f141a).into();
    c.primary_hover = rgb(0x4199c7).into();
    c.primary_active = rgb(0x3a89b1).into();

    c.secondary = rgb(0x1b2838).into();
    c.secondary_foreground = rgb(0xdce3ea).into();
    c.secondary_hover = rgb(0x233348).into();
    c.secondary_active = rgb(0x223447).into();

    c.accent = rgb(0x223447).into();
    c.accent_foreground = rgb(0x66c0f4).into();

    c.sidebar = rgb(0x12151c).into();
    c.sidebar_border = rgb(0x2a475e).into();
    c.sidebar_foreground = rgb(0x8f98a0).into();
    c.sidebar_accent = rgb(0x1b2838).into();
    c.sidebar_accent_foreground = rgb(0x66c0f4).into();
    c.sidebar_primary = rgb(0x66c0f4).into();

    c.success = rgb(0x10b981).into();
    c.success_foreground = rgb(0x0f141a).into();
    c.success_hover = rgb(0x059669).into();
    c.danger = rgb(0xe11d48).into();
    c.danger_foreground = rgb(0xffffff).into();
    c.danger_hover = rgb(0xf43f5e).into();
    c.warning = rgb(0xf59e0b).into();
    c.warning_foreground = rgb(0x0f141a).into();

    c.link = rgb(0x66c0f4).into();
    c.link_hover = rgb(0x8fd3f7).into();
    c.selection = rgba(0x66c0f44d).into();
    c.overlay = rgba(0x000000c0).into();
    c.popover = rgb(0x1b2838).into();
    c.popover_foreground = rgb(0xdce3ea).into();
    c.progress_bar = rgb(0x66c0f4).into();
    c.window_border = rgb(0x2a475e).into();
}

/// Light palette: crisp slate surfaces with a stronger blue accent.
fn stamp_light(c: &mut gpui_kit::component::ThemeColor) {
    c.background = rgb(0xf2f4f8).into();
    c.foreground = rgb(0x1b2838).into();
    c.title_bar = rgb(0xf6f8fa).into();
    c.title_bar_border = rgb(0xd9e0e8).into();
    c.border = rgb(0xd3dae3).into();
    c.input = rgb(0xffffff).into();
    c.muted = rgb(0xe8edf3).into();
    c.muted_foreground = rgb(0x64748b).into();

    c.primary = rgb(0x2470b3).into();
    c.primary_foreground = rgb(0xffffff).into();
    c.primary_hover = rgb(0x1d5c92).into();
    c.primary_active = rgb(0x184e7c).into();

    c.secondary = rgb(0xffffff).into();
    c.secondary_foreground = rgb(0x1b2838).into();
    c.secondary_hover = rgb(0xeaf0f6).into();
    c.secondary_active = rgb(0xdde7f0).into();

    c.accent = rgb(0xdde7f0).into();
    c.accent_foreground = rgb(0x2470b3).into();

    c.sidebar = rgb(0xeef1f6).into();
    c.sidebar_border = rgb(0xd9e0e8).into();
    c.sidebar_foreground = rgb(0x64748b).into();
    c.sidebar_accent = rgb(0xe2e9f1).into();
    c.sidebar_accent_foreground = rgb(0x2470b3).into();
    c.sidebar_primary = rgb(0x2470b3).into();

    c.success = rgb(0x15803d).into();
    c.success_foreground = rgb(0xffffff).into();
    c.success_hover = rgb(0x166534).into();
    c.danger = rgb(0xe11d48).into();
    c.danger_foreground = rgb(0xffffff).into();
    c.danger_hover = rgb(0xbe123c).into();
    c.warning = rgb(0xd97706).into();
    c.warning_foreground = rgb(0xffffff).into();

    c.link = rgb(0x2470b3).into();
    c.link_hover = rgb(0x1d5c92).into();
    c.selection = rgba(0x66c0f466).into();
    c.overlay = rgba(0x0f172ab0).into();
    c.popover = rgb(0xffffff).into();
    c.popover_foreground = rgb(0x1b2838).into();
    c.progress_bar = rgb(0x2470b3).into();
    c.window_border = rgb(0xd3dae3).into();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steam_blue_round_trips() {
        // #66c0f4 in HSL: h≈0.551, s≈0.874, l≈0.678.
        let blue = blue();
        assert!((blue.h - 0.5514).abs() < 0.01);
        assert!((blue.s - 0.8742).abs() < 0.01);
        assert!((blue.l - 0.6784).abs() < 0.01);
    }
}
