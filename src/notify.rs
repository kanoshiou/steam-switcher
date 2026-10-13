//! Best-effort toast notifications. Failures are logged, never fatal — a
//! missing toast must not abort an in-progress switch.

/// AppUserModelID toasts are sent under. Registered with the shell at startup
/// by [`register_app_id`] so toasts display our own name and icon.
#[cfg(windows)]
const APP_ID: &str = "Kanoshiou.SteamSwitcher";

/// Show a toast notification. The message is the toast's (bold) content line;
/// the source name shown by the shell comes from the registered AppUserModelID,
/// so callers should not repeat the app name in the message.
pub fn show(message: &str) {
    let result = send(message);
    if let Err(err) = result {
        tracing::warn!("toast notification failed: {err}");
    }
}

fn send(message: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut notification = notify_rust::Notification::new();
    notification.summary(message);

    #[cfg(windows)]
    notification.app_id(APP_ID);

    notification.show().map(|_| ())?;
    Ok(())
}

/// Register the AppUserModelID toasts are sent under, so the shell shows our
/// display name and icon as the toast source (instead of falling back to some
/// borrowed identity, e.g. PowerShell's).
///
/// Unpackaged desktop apps must register their AUMID with the shell before
/// Windows will render their toasts. Writing `DisplayName` + `IconUri` under
/// `HKCU\Software\Classes\AppUserModelId\<AUMID>` is the installer-free way
/// (the same mechanism tauri-winrt-notification ships). Best-effort: on
/// failure toasts are simply dropped and the error is logged.
#[cfg(windows)]
pub fn register_app_id() {
    if let Err(err) = try_register_app_id() {
        tracing::warn!("toast app-id registration failed: {err:#}");
    }
}

#[cfg(windows)]
fn try_register_app_id() -> anyhow::Result<()> {
    use winreg::enums::HKEY_CURRENT_USER;

    let icon = extract_icon()?;
    let hkcu = winreg::RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu.create_subkey(format!(r"Software\Classes\AppUserModelId\{APP_ID}"))?;
    key.set_value("DisplayName", &rust_i18n::t!("app.title").to_string())?;
    key.set_value("IconUri", &icon.to_string_lossy().to_string())?;
    tracing::debug!(app_id = APP_ID, icon = %icon.display(), "toast app-id registered");
    Ok(())
}

/// Materialize the toast icon under the app config dir and return its path
/// (the AUMID registry key needs a real file path, and the exe is portable).
#[cfg(windows)]
fn extract_icon() -> anyhow::Result<std::path::PathBuf> {
    const ICON: &[u8] = include_bytes!("../assets/icon.ico");

    let dir = crate::config::config_path()
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("icon.ico");
    std::fs::write(&path, ICON)?;
    Ok(path)
}
