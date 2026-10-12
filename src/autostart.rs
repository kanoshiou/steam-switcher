//! "Start with Windows" toggle backed by the per-user `Run` registry key
//! (HKCU — no elevation required).

/// Registry value name this app claims under the Run key.
#[cfg(windows)]
const VALUE_NAME: &str = "steam-switcher";

/// Whether autostart is currently enabled.
pub fn is_enabled() -> bool {
    #[cfg(windows)]
    {
        read_current_value().is_some()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Enable autostart: point the Run value at the current executable.
pub fn enable() -> Result<(), AutostartError> {
    #[cfg(windows)]
    {
        let exe = std::env::current_exe().map_err(AutostartError::Io)?;
        write_run_value(&format!("\"{}\"", exe.display()))?;
        tracing::info!(exe = %exe.display(), "autostart enabled");
        Ok(())
    }
    #[cfg(not(windows))]
    {
        Err(AutostartError::Unsupported)
    }
}

/// Disable autostart by deleting the Run value.
pub fn disable() -> Result<(), AutostartError> {
    #[cfg(windows)]
    {
        let hkcu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
        let key = hkcu
            .open_subkey_with_flags(RUN_KEY_PATH, winreg::enums::KEY_SET_VALUE)
            .map_err(AutostartError::Io)?;
        key.delete_value(VALUE_NAME).map_err(AutostartError::Io)?;
        tracing::info!("autostart disabled");
        Ok(())
    }
    #[cfg(not(windows))]
    {
        Err(AutostartError::Unsupported)
    }
}

/// Flip autostart, returning the new state.
pub fn toggle() -> Result<bool, AutostartError> {
    if is_enabled() {
        disable()?;
        Ok(false)
    } else {
        enable()?;
        Ok(true)
    }
}

/// Errors from toggling autostart.
#[derive(Debug, thiserror::Error)]
pub enum AutostartError {
    /// Registry or filesystem access failed.
    #[error("autostart registry access failed: {0}")]
    Io(#[from] std::io::Error),
    /// The current platform has no autostart implementation.
    #[error("autostart is not supported on this platform")]
    Unsupported,
}

#[cfg(windows)]
const RUN_KEY_PATH: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";

/// The current Run value content, or `None` when autostart is off.
#[cfg(windows)]
fn read_current_value() -> Option<String> {
    let hkcu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
    let key = hkcu.open_subkey(RUN_KEY_PATH).ok()?;
    key.get_value(VALUE_NAME).ok()
}

#[cfg(windows)]
fn write_run_value(command: &str) -> Result<(), AutostartError> {
    let hkcu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
    let key = hkcu
        .open_subkey_with_flags(RUN_KEY_PATH, winreg::enums::KEY_SET_VALUE)
        .map_err(AutostartError::Io)?;
    key.set_value(VALUE_NAME, &command).map_err(AutostartError::Io)?;
    Ok(())
}
