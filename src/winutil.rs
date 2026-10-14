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

/// Hide a window from the screen, taskbar and alt-tab (`SW_HIDE`) while its
/// process keeps running — the close-to-tray behavior, which gpui has no safe
/// API for. gpui observes the resulting `WM_SHOWWINDOW` and pauses presenting
/// frames until the window is restored.
///
/// The companion of [`restore_platform_window`].
pub fn hide_platform_window(hwnd: windows::Win32::Foundation::HWND) {
    use windows::Win32::UI::WindowsAndMessaging::{SW_HIDE, ShowWindow};
    // SAFETY: `hwnd` is the live platform window of the caller's gpui window;
    // ShowWindow only changes its placement.
    unsafe {
        let _ = ShowWindow(hwnd, SW_HIDE);
    }
}

/// Bring a hidden or minimized window back on screen: `SW_SHOW` when hidden
/// (keeps its normal/maximized placement), `SW_RESTORE` when minimized, and
/// nothing when already shown. Returns whether a `ShowWindow` call was made.
pub fn restore_platform_window(hwnd: windows::Win32::Foundation::HWND) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        IsIconic, IsWindowVisible, SW_RESTORE, SW_SHOW, ShowWindow,
    };
    // SAFETY: `hwnd` is the live platform window of the caller's gpui window;
    // the queries only read its placement and ShowWindow only changes it.
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_SHOW);
            true
        } else if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
            true
        } else {
            false
        }
    }
}
