//! Best-effort toast notifications. Failures are logged, never fatal — a
//! missing toast must not abort an in-progress switch.

/// Show a toast notification.
pub fn show(summary: &str, body: &str) {
    let result = send(summary, body);
    if let Err(err) = result {
        tracing::warn!(summary, "toast notification failed: {err}");
    }
}

fn send(summary: &str, body: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut notification = notify_rust::Notification::new();
    notification.summary(summary).body(body);

    // Windows toasts require an AppUserModelID that is registered with the
    // shell. We ship no installer, so borrow PowerShell's well-known AUMID —
    // the standard workaround for unpackaged apps (see tauri-winrt-notification
    // docs). Toasts show under PowerShell's identity purely as a display name.
    #[cfg(windows)]
    notification.app_id("{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\\WindowsPowerShell\\v1.0\\powershell.exe");

    notification.show().map(|_| ())?;
    Ok(())
}
