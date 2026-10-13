//! Minimal win32 interop helpers: message boxes and the message pump that
//! keeps the tray icon alive.

use windows::core::HSTRING;
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, MessageBoxW, PeekMessageW, TranslateMessage, MB_ICONWARNING,
    MB_OKCANCEL, MB_SETFOREGROUND, MB_TOPMOST, MESSAGEBOX_RESULT, MSG, PM_REMOVE,
};

/// Show a modal message box with no owner window.
pub fn message_box(text: &str, caption: &str, style: windows::Win32::UI::WindowsAndMessaging::MESSAGEBOX_STYLE) -> MESSAGEBOX_RESULT {
    let text = HSTRING::from(text);
    let caption = HSTRING::from(caption);
    // SAFETY: MessageBoxW with a null owner and valid wide strings.
    unsafe { MessageBoxW(None, &text, &caption, style) }
}

/// Blocking OK/Cancel confirmation with a warning icon. Used for the
/// game-process confirmation before switching (must not be skippable).
pub fn confirm_dialog(text: &str, caption: &str) -> bool {
    message_box(
        text,
        caption,
        MB_OKCANCEL | MB_ICONWARNING | MB_TOPMOST | MB_SETFOREGROUND,
    ) == windows::Win32::UI::WindowsAndMessaging::IDOK
}

/// Show a plain informational dialog (used for the "already running" notice).
pub fn info_dialog(text: &str, caption: &str) {
    message_box(text, caption, windows::Win32::UI::WindowsAndMessaging::MB_OK);
}

/// Run the win32 message pump on the calling thread until the process exits.
///
/// The tray icon (and the global hotkey hook) only receive their events while
/// this thread pumps messages, so this must be the last thing `main` does.
pub fn run_message_pump() {
    let mut msg = MSG::default();
    loop {
        // SAFETY: `msg` is a valid, default-initialized MSG for GetMessageW to
        // fill; the loop ends when GetMessageW reports WM_QUIT (0) or error (-1).
        let result = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if result.0 <= 0 {
            tracing::debug!("message pump stopping (GetMessageW returned {})", result.0);
            break;
        }
        // SAFETY: msg came straight from GetMessageW and is a queued message.
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Dispatch all currently queued win32 messages without blocking.
///
/// Non-blocking counterpart to [`run_message_pump`]: the tray-only loop calls
/// this between menu-event polls so the tray icon's hidden window keeps
/// receiving its messages while the thread also drains the event channels.
pub fn pump_pending_messages() {
    let mut msg = MSG::default();
    loop {
        // SAFETY: `msg` is a valid, default-initialized MSG; PM_REMOVE takes
        // the message out of the queue, and false means the queue is empty.
        let pending = unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) };
        if !pending.as_bool() {
            break;
        }
        // SAFETY: msg came straight from PeekMessageW and is a queued message.
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
