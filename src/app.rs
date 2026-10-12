//! Application bootstrap and the tray/hotkey event loop.
//!
//! Threading model (Phase 1, no gpui): the main thread owns the tray icon,
//! the hotkey manager and the win32 message pump. Menu and hotkey events are
//! delivered by muda/global-hotkey into global crossbeam channels; the main
//! loop drains those channels between message-pump ticks. Switch actions run
//! inline (they are slow but rare, and the confirmation dialog pumps its own
//! modal loop, so the tray stays alive throughout).

use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Context;
use global_hotkey::GlobalHotKeyManager;

use crate::autostart;
use crate::hotkey;
use crate::logging;
use crate::notify;
use crate::singleton;
use crate::steam::discover;
use crate::steam::switch::{self, SwitchOutcome};
use crate::steam::users::{self, SteamUser};
use crate::tray::{self, Action, Tray};
use crate::winutil;

/// Long-running state shared by the event handlers.
struct App {
    steam: discover::SteamInstall,
    tray: Tray,
    /// Guards against two switches racing (e.g. menu click during a hotkey switch).
    switching: AtomicBool,
}

/// Start the tray application. Never returns under normal operation — the
/// win32 message pump runs until the user picks Quit.
pub fn run() -> anyhow::Result<()> {
    logging::init().context("initializing logging")?;
    tracing::info!("steam-switcher v{} starting", env!("CARGO_PKG_VERSION"));

    // Single instance: a second launch just nudges the user and exits cleanly.
    let _singleton = match singleton::acquire() {
        Ok(guard) => guard,
        Err(singleton::SingletonError::AlreadyRunning) => {
            #[cfg(windows)]
            winutil::info_dialog(
                "steam-switcher is already running in the system tray.",
                "Steam Switcher",
            );
            return Ok(());
        }
        Err(err) => return Err(err.into()),
    };

    let steam = match discover::discover() {
        Ok(steam) => steam,
        Err(err) => {
            #[cfg(windows)]
            winutil::info_dialog(
                &format!("Could not locate the Steam installation.\n\n{err}"),
                "Steam Switcher",
            );
            return Err(err.into());
        }
    };
    tracing::info!(root = %steam.root.display(), "steam discovered");

    let app = App {
        tray: Tray::new(&load_users_lenient(&steam), autostart::is_enabled())
            .context("creating tray icon")?,
        steam,
        switching: AtomicBool::new(false),
    };

    // The manager must outlive the loop and stay on the pumping thread.
    let manager = GlobalHotKeyManager::new().context("creating global hotkey manager")?;
    hotkey::register_default(&manager).context("registering global hotkey")?;
    tracing::info!(
        "hotkey {} registered (cycles to the next account)",
        hotkey::DEFAULT_HOTKEY_LABEL
    );

    event_loop(&app, &manager);
    Ok(())
}

/// Drain the tray-menu and hotkey channels, pumping win32 messages in between.
#[cfg(windows)]
fn event_loop(app: &App, _manager: &GlobalHotKeyManager) {
    use windows::Win32::Foundation::WAIT_OBJECT_0;
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, MsgWaitForMultipleObjectsEx, PeekMessageW, TranslateMessage, MSG,
        MWMO_INPUTAVAILABLE, PM_REMOVE, QS_ALLINPUT,
    };

    /// How long a single pump iteration waits before re-checking the event channels.
    const TICK_MS: u32 = 25;

    let mut msg = MSG::default();
    loop {
        while let Ok(event) = tray_icon::menu::MenuEvent::receiver().try_recv() {
            handle_action(app, tray::parse_action(&event.id));
        }
        while let Ok(event) = global_hotkey::GlobalHotKeyEvent::receiver().try_recv() {
            if hotkey::is_our_press(&event) {
                cycle_to_next_account(app);
            }
        }

        // Wait for win32 input (with a timeout), then flush whatever arrived.
        // SAFETY: null handle list + valid MSG pointer; constants request all
        // input events and make the call return when input is available.
        let wait = unsafe {
            MsgWaitForMultipleObjectsEx(None, TICK_MS, QS_ALLINPUT, MWMO_INPUTAVAILABLE)
        };
        if wait == WAIT_OBJECT_0 {
            // SAFETY: `msg` is a valid MSG for PeekMessageW to fill; PM_REMOVE
            // takes the message off the queue for dispatch below.
            while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
                // SAFETY: msg came from PeekMessageW and is a translated/dispatched pair.
                unsafe {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }
    }
}

/// Non-Windows builds have no tray in Phase 1 (Windows-first; see docs/plan.md).
#[cfg(not(windows))]
fn event_loop(_app: &App, _manager: &GlobalHotKeyManager) {
    tracing::warn!("tray UI is Windows-only in Phase 1; idling");
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}

/// Dispatch one tray menu action.
fn handle_action(app: &App, action: Option<Action>) {
    let Some(action) = action else {
        tracing::debug!(?action, "ignoring unrecognized menu id");
        return;
    };
    match action {
        Action::Switch(steam_id64) => switch_to(app, &steam_id64),
        Action::Refresh => {
            let users = load_users_lenient(&app.steam);
            app.tray.update_menu(&users, autostart::is_enabled());
            notify::show("Steam Switcher", "Account list refreshed.");
        }
        Action::ToggleAutostart => match autostart::toggle() {
            Ok(enabled) => {
                let users = load_users_lenient(&app.steam);
                app.tray.update_menu(&users, enabled);
                notify::show(
                    "Steam Switcher",
                    if enabled {
                        "steam-switcher will start with Windows."
                    } else {
                        "steam-switcher will no longer start with Windows."
                    },
                );
            }
            Err(err) => {
                tracing::error!(%err, "failed to toggle autostart");
                notify::show("Steam Switcher", &format!("Failed to toggle autostart: {err}"));
            }
        },
        Action::Settings => {
            notify::show("Steam Switcher", "The settings window ships in Phase 2.");
        }
        Action::Quit => {
            tracing::info!("quit requested, exiting");
            std::process::exit(0);
        }
    }
}

/// Hotkey action: switch to whichever account comes after the active one in
/// recency order (degraded design from docs/plan.md — no popup in Phase 1).
fn cycle_to_next_account(app: &App) {
    let users = load_users_lenient(&app.steam);
    if users.len() < 2 {
        notify::show("Steam Switcher", "Only one Steam account is remembered — nothing to cycle.");
        return;
    }
    let users = users::sorted_by_recency(users);
    let next = match users.iter().position(|u| u.most_recent) {
        Some(index) => (index + 1) % users.len(),
        None => 0,
    };
    let steam_id64 = users[next].steam_id64.clone();
    switch_to(app, &steam_id64);
}

/// Run the full switch flow for one account and surface the result to the user.
fn switch_to(app: &App, steam_id64: &str) {
    if app.switching.swap(true, Ordering::SeqCst) {
        notify::show("Steam Switcher", "A switch is already in progress — hold on.");
        return;
    }

    let result = perform_switch(app, steam_id64);
    if let Ok(SwitchOutcome::Switched) = result {
        // Reflect the new active account immediately.
        let users = load_users_lenient(&app.steam);
        app.tray.update_menu(&users, autostart::is_enabled());
    }

    match result {
        Ok(SwitchOutcome::Switched) => {} // perform_switch already toasted success
        Ok(SwitchOutcome::AlreadyActive) => {} // toasted as well
        Ok(SwitchOutcome::CancelledByUser) => {} // user declined; stay quiet
        Err(err) => {
            tracing::error!(%err, "switch failed");
            notify::show("Steam Switcher", &format!("Switch failed: {err}"));
        }
    }

    app.switching.store(false, Ordering::SeqCst);
}

/// The blocking part of a switch, with user-facing toasts at start and end.
fn perform_switch(
    app: &App,
    steam_id64: &str,
) -> Result<SwitchOutcome, switch::SwitchError> {
    let users = users::load_users(&app.steam)?;
    let Some(target) = users.iter().find(|u| u.steam_id64 == steam_id64) else {
        // Menu was stale; treat like refresh so the list heals itself.
        let fresh = load_users_lenient(&app.steam);
        app.tray.update_menu(&fresh, autostart::is_enabled());
        notify::show("Steam Switcher", "That account is no longer in loginusers.vdf — list refreshed.");
        return Ok(SwitchOutcome::CancelledByUser);
    };
    let label = target.display_name().to_string();

    notify::show("Steam Switcher", &format!("Switching to {label}… (Steam will restart)"));

    // Non-skippable confirmation when Steam games are running (cloud saves).
    let confirm = |games: &[String]| -> bool {
        #[cfg(windows)]
        {
            winutil::confirm_dialog(
                &format!(
                    "These Steam games are running:\n\n{}\n\nQuitting Steam now may lose unsaved \
                     progress (cloud saves upload on exit).\n\nQuit Steam and switch anyway?",
                    games.join("\n")
                ),
                "Steam games are running",
            )
        }
        #[cfg(not(windows))]
        {
            let _ = games;
            true
        }
    };

    match switch::switch_to_account(&app.steam, steam_id64, &confirm) {
        Ok(SwitchOutcome::Switched) => {
            notify::show("Steam Switcher", &format!("Switched to {label}."));
            Ok(SwitchOutcome::Switched)
        }
        Ok(SwitchOutcome::AlreadyActive) => {
            notify::show("Steam Switcher", &format!("Already using {label}."));
            Ok(SwitchOutcome::AlreadyActive)
        }
        other => other,
    }
}

/// Reload accounts; on failure keep an empty list and toast instead of dying.
fn load_users_lenient(steam: &discover::SteamInstall) -> Vec<SteamUser> {
    match users::load_users(steam) {
        Ok(users) => users,
        Err(err) => {
            tracing::error!(%err, "failed to load loginusers.vdf");
            notify::show(
                "Steam Switcher",
                &format!("Could not read Steam accounts: {err}"),
            );
            Vec::new()
        }
    }
}
