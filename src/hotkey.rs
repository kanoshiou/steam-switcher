//! Global hotkey registration (Phase 1: fixed default `Ctrl+Alt+S`).
//!
//! The manager must be created on the thread that runs the win32 message pump;
//! see [`crate::app`].

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyManager, GlobalHotKeyEvent, HotKeyState};

/// The Phase 1 default global hotkey: `Ctrl+Alt+S`.
///
/// `HotKey::new` is not const, so this is a function; the id it derives is
/// deterministic, so comparisons against event ids stay stable.
pub fn default_hotkey() -> HotKey {
    HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyS)
}

/// Human-readable form of [`default_hotkey`] for menus and toasts.
pub const DEFAULT_HOTKEY_LABEL: &str = "Ctrl+Alt+S";

/// Errors from registering the global hotkey.
#[derive(Debug, thiserror::Error)]
#[error("failed to register global hotkey {label}: {source}")]
pub struct HotkeyError {
    /// Label of the hotkey we tried to register.
    pub label: &'static str,
    /// Underlying global-hotkey error (e.g. the combo is taken by another app).
    #[source]
    source: global_hotkey::Error,
}

/// Create the manager on the current thread and register [`default_hotkey`].
pub fn register_default(manager: &GlobalHotKeyManager) -> Result<(), HotkeyError> {
    manager
        .register(default_hotkey())
        .map_err(|source| HotkeyError { label: DEFAULT_HOTKEY_LABEL, source })
}

/// Whether this event is the initial press of [`default_hotkey`].
///
/// global-hotkey delivers both press and release; we act on press only.
pub fn is_our_press(event: &GlobalHotKeyEvent) -> bool {
    event.id() == default_hotkey().id() && event.state() == HotKeyState::Pressed
}
