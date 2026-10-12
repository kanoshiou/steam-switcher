//! Application bootstrap: gpui main loop, tray + hotkey event bridging (D2),
//! and the switch flows shared by the tray menu and the global hotkey.
//!
//! Threading model (Phase 2): gpui owns the main thread and its win32 message
//! pump, so the tray icon, the hotkey manager and the settings window all live
//! on the main thread. Menu/hotkey/tray-click events arrive on foreign global
//! channels; a single foreground task polls them and awaits a short timer
//! between polls (never blocks the pump). The blocking switch flow itself runs
//! on the background executor and reports back through `AsyncApp::update`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use global_hotkey::GlobalHotKeyManager;
use gpui_kit::component::TitleBar;
use gpui_kit::{AnyWindowHandle, App, AppContext, AsyncApp, Global, WindowOptions, px, size};

use crate::autostart;
use crate::config::Config;
use crate::hotkey;
use crate::logging;
use crate::notify;
use crate::settings::SettingsWindow;
use crate::singleton;
use crate::steam::discover::{self, SteamInstall};
use crate::steam::switch::{self, SwitchOutcome};
use crate::steam::users::{self, SteamUser};
use crate::tray::{self, Action, Tray};
use crate::winutil;

/// Poll interval while waiting for foreign tray/hotkey events.
const EVENT_POLL: Duration = Duration::from_millis(30);

/// Main-thread application state. gpui globals are only touched from the main
/// thread, which is exactly what the tray icon requires (it is not `Send`).
pub struct AppState {
    tray: Tray,
    steam: SteamInstall,
    window: Option<AnyWindowHandle>,
    /// Approximation of the settings window's taskbar presence: `true` after
    /// open/activate, `false` after a minimize. Tray left-click toggles it.
    window_shown: bool,
    _hotkey_manager: GlobalHotKeyManager,
}

impl Global for AppState {}

impl AppState {
    /// Re-read accounts + config from disk and rebuild the tray menu.
    pub fn refresh_tray(&mut self) {
        let users = load_users_lenient(&self.steam);
        let config = Config::load();
        self.tray.update_menu(&users, autostart::is_enabled(), &config);
    }
}

/// Bring the settings window to the front (restores from minimized).
fn show_window(cx: &mut App) {
    let window = cx.global_mut::<AppState>().window;
    if let Some(window) = window {
        // Err = the window was closed meanwhile; nothing to show.
        let _ = window.update(cx, |_, window, _| window.activate_window());
    }
    cx.global_mut::<AppState>().window_shown = true;
}

/// Minimize the settings window (gpui has no hide; minimize is the tray-app
/// compromise planned in docs/plan.md P2).
fn hide_window(cx: &mut App) {
    let window = cx.global_mut::<AppState>().window;
    if let Some(window) = window {
        let _ = window.update(cx, |_, window, _| window.minimize_window());
    }
    cx.global_mut::<AppState>().window_shown = false;
}

/// Start the application. Returns when the process exits.
pub fn run() -> anyhow::Result<()> {
    logging::init().context("initializing logging")?;
    tracing::info!("steam-switcher v{} starting", env!("CARGO_PKG_VERSION"));

    // Single instance: a second launch just nudges the user and exits cleanly.
    let _singleton = match singleton::acquire() {
        Ok(guard) => guard,
        Err(singleton::SingletonError::AlreadyRunning) => {
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
            winutil::info_dialog(
                &format!("Could not locate the Steam installation.\n\n{err}"),
                "Steam Switcher",
            );
            return Err(err.into());
        }
    };
    tracing::info!(root = %steam.root.display(), "steam discovered");

    // Shared guard against concurrent switches (tray menu vs hotkey).
    let switching = Arc::new(AtomicBool::new(false));

    run_gpui(steam, switching);
    Ok(())
}

/// Bring up gpui, the tray and the event loop. Never returns.
fn run_gpui(steam: SteamInstall, switching: Arc<AtomicBool>) {
    gpui_kit::application().run(move |cx: &mut App| {
        gpui_kit::init(cx);

        let config = Config::load();
        let users = load_users_lenient(&steam);
        let tray = Tray::new(&users, autostart::is_enabled(), &config)
            .expect("failed to create tray icon");
        let hotkey_manager = GlobalHotKeyManager::new().expect("failed to create hotkey manager");
        hotkey::register_default(&hotkey_manager).expect("failed to register global hotkey");
        tracing::info!(
            "hotkey {} registered (cycles to the next account)",
            hotkey::DEFAULT_HOTKEY_LABEL
        );

        // Open the settings window hidden; it is shown from the tray.
        let window_options = WindowOptions {
            window_min_size: Some(size(px(560.0), px(480.0))),
            show: false,
            ..TitleBar::window_options()
        };
        let users_for_window = users.clone();
        let config_for_window = config.clone();
        let (handle, _view) = gpui_kit::open_window(window_options, cx, |window, cx| {
            cx.new(|cx| SettingsWindow::new(users_for_window, config_for_window, window, cx))
        })
        .expect("failed to open the settings window");

        cx.set_global(AppState {
            tray,
            steam,
            window: Some(handle),
            window_shown: false,
            _hotkey_manager: hotkey_manager,
        });

        // Veto real closes: the X button minimizes instead (tray-app lifecycle).
        let _ = handle.update(cx, |_, window, cx| {
            window.on_window_should_close(cx, |_window, cx| {
                hide_window(cx);
                false
            });
        });

        // D2 bridge: poll the foreign event channels on the main thread with
        // async waits between polls, so the win32 pump stays responsive.
        cx.spawn(async move |cx| event_loop(cx, switching).await).detach();

        tracing::info!("gui ready");
    });
}

/// Drain tray-menu, hotkey and tray-click events, dispatching each on the
/// main thread. Switch flows hop to the background executor while running.
async fn event_loop(cx: &mut AsyncApp, switching: Arc<AtomicBool>) {
    use tray_icon::{MouseButton, MouseButtonState, TrayIconEvent};

    loop {
        let mut switch_requests: Vec<String> = Vec::new();
        let mut quit = false;

        while let Ok(event) = tray_icon::menu::MenuEvent::receiver().try_recv() {
            match tray::parse_action(&event.id) {
                Some(Action::Switch(steam_id64)) => switch_requests.push(steam_id64),
                Some(Action::Refresh) => {
                    cx.update(|cx| cx.global_mut::<AppState>().refresh_tray());
                }
                Some(Action::ToggleAutostart) => {
                    cx.update(|cx| {
                        match autostart::toggle() {
                            Ok(enabled) => notify::show(
                                "Steam Switcher",
                                if enabled {
                                    "steam-switcher will start with Windows."
                                } else {
                                    "steam-switcher will no longer start with Windows."
                                },
                            ),
                            Err(err) => {
                                tracing::error!(%err, "failed to toggle autostart");
                                notify::show(
                                    "Steam Switcher",
                                    &format!("Failed to toggle autostart: {err}"),
                                );
                            }
                        }
                        cx.global_mut::<AppState>().refresh_tray();
                    });
                }
                Some(Action::Settings) => {
                    cx.update(show_window);
                }
                Some(Action::Quit) => quit = true,
                None => {}
            }
        }

        while let Ok(event) = global_hotkey::GlobalHotKeyEvent::receiver().try_recv() {
            if hotkey::is_our_press(&event)
                && let Some(next) = cx.update(next_account_id)
            {
                switch_requests.push(next);
            }
        }

        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                cx.update(|cx| {
                    // Extract state first: the window update needs `cx` too.
                    let (window, shown) = {
                        let state = cx.global::<AppState>();
                        (state.window, state.window_shown)
                    };
                    if let Some(window) = window {
                        let _ = window.update(cx, |_, window, _| {
                            if shown {
                                window.minimize_window()
                            } else {
                                window.activate_window()
                            }
                        });
                    }
                    cx.global_mut::<AppState>().window_shown = !shown;
                });
            }
        }

        for steam_id64 in switch_requests {
            if switching.swap(true, Ordering::SeqCst) {
                notify::show("Steam Switcher", "A switch is already in progress — hold on.");
                continue;
            }
            do_switch(cx, &steam_id64, &switching).await;
        }

        if quit {
            tracing::info!("quit requested, exiting");
            cx.update(|cx| cx.quit());
            return;
        }

        cx.background_executor().timer(EVENT_POLL).await;
    }
}

/// The account after the active one in recency order (hotkey cycling).
fn next_account_id(cx: &mut App) -> Option<String> {
    let steam = cx.global_mut::<AppState>().steam.clone();
    let users = load_users_lenient(&steam);
    if users.len() < 2 {
        notify::show("Steam Switcher", "Only one Steam account is remembered — nothing to cycle.");
        return None;
    }
    let users = users::sorted_by_recency(users);
    let next = match users.iter().position(|u| u.most_recent) {
        Some(index) => (index + 1) % users.len(),
        None => 0,
    };
    Some(users[next].steam_id64.clone())
}

/// Run the full switch flow on the background executor, then refresh the tray.
async fn do_switch(cx: &mut AsyncApp, steam_id64: &str, switching: &AtomicBool) {
    let steam = cx.update(|cx| cx.global_mut::<AppState>().steam.clone());
    let steam_id64 = steam_id64.to_string();

    let outcome = cx
        .background_executor()
        .spawn(async move { perform_switch(&steam, &steam_id64) })
        .await;

    cx.update(|cx| {
        cx.global_mut::<AppState>().refresh_tray();
        match outcome {
            Ok(SwitchOutcome::Switched) => {} // perform_switch toasted
            Ok(SwitchOutcome::AlreadyActive) => {}
            Ok(SwitchOutcome::CancelledByUser) => {}
            Err(err) => {
                tracing::error!(%err, "switch failed");
                notify::show("Steam Switcher", &format!("Switch failed: {err}"));
            }
        }
    });
    switching.store(false, Ordering::SeqCst);
}

/// The blocking part of a switch: stale-target healing, game confirmation,
/// Steam shutdown, vdf + registry writes, relaunch. Runs off the main thread.
fn perform_switch(
    steam: &SteamInstall,
    steam_id64: &str,
) -> Result<SwitchOutcome, switch::SwitchError> {
    let users = users::load_users(steam)?;
    let Some(target) = users.iter().find(|u| u.steam_id64 == steam_id64) else {
        // Menu or hotkey list was stale; the caller refreshes the tray after.
        notify::show(
            "Steam Switcher",
            "That account is no longer in loginusers.vdf — list refreshed.",
        );
        return Ok(SwitchOutcome::CancelledByUser);
    };
    let label = target.display_name().to_string();

    notify::show("Steam Switcher", &format!("Switching to {label}… (Steam will restart)"));

    // Config-gated, non-skippable confirmation when Steam games are running.
    let confirm = |games: &[String]| -> bool {
        if !Config::load().confirm_before_switch {
            return true;
        }
        winutil::confirm_dialog(
            &format!(
                "These Steam games are running:\n\n{}\n\nQuitting Steam now may lose unsaved \
                 progress (cloud saves upload on exit).\n\nQuit Steam and switch anyway?",
                games.join("\n")
            ),
            "Steam games are running",
        )
    };

    match switch::switch_to_account(steam, steam_id64, &confirm) {
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
fn load_users_lenient(steam: &SteamInstall) -> Vec<SteamUser> {
    match users::load_users(steam) {
        Ok(users) => users,
        Err(err) => {
            tracing::error!(%err, "failed to load loginusers.vdf");
            notify::show("Steam Switcher", &format!("Could not read Steam accounts: {err}"));
            Vec::new()
        }
    }
}
