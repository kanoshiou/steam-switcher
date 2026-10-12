//! Application bootstrap: gpui main loop, tray event bridging (D2), and the
//! switch flows shared by the tray menu and the window UI.
//!
//! Threading model (Phase 2): gpui owns the main thread and its win32 message
//! pump, so the tray icon and the app window live there. Menu/tray-click
//! events arrive on foreign global channels; a single foreground task polls
//! them and awaits a short timer between polls (never blocks the pump). The
//! blocking switch flow itself runs on the background executor and reports
//! back through `AsyncApp::update`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use gpui_kit::component::ThemeMode;
use gpui_kit::{AnyWindowHandle, App, AppContext, AsyncApp, Entity, Global, TitlebarOptions, WindowOptions, px, size};
use rust_i18n::t;

use crate::autostart;
use crate::config::Config;
use crate::logging;
use crate::notify;
use crate::app_window::{AppWindow, Page};
use crate::singleton;
use crate::steam::discover::{self, SteamInstall};
use crate::steam::library;
use crate::steam::switch::{self, SwitchOutcome};
use crate::steam::users::{self, SteamUser};
use crate::tray::{self, Action, Tray};
use crate::winutil;

/// Poll interval while waiting for foreign tray events.
const EVENT_POLL: Duration = Duration::from_millis(30);
/// Delay before firing the steam:// URL after a switch (Steam must boot; the
/// protocol queues internally — docs/research-game-list.md).
const LAUNCH_DELAY: Duration = Duration::from_secs(3);

/// Main-thread application state. gpui globals are only touched from the main
/// thread, which is exactly what the tray icon requires (it is not `Send`).
pub struct AppState {
    tray: Tray,
    steam: SteamInstall,
    window: Option<AnyWindowHandle>,
    /// The app window's content view. `gpui_kit::open_window` wraps the real
    /// window root in `base::Root`, so the window handle cannot be downcast to
    /// `AppWindow`; this entity is the way to reach the view.
    view: Entity<AppWindow>,
    /// Approximation of the app window's taskbar presence: `true` after
    /// open/activate, `false` after a minimize. Tray left-click toggles it.
    window_shown: bool,
    switching: Arc<AtomicBool>,
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

/// Start the application. Returns when the process exits.
pub fn run() -> anyhow::Result<()> {
    logging::init().context("initializing logging")?;
    tracing::info!("steam-switcher v{} starting", env!("CARGO_PKG_VERSION"));

    // Single instance: a second launch just nudges the user and exits cleanly.
    let _singleton = match singleton::acquire() {
        Ok(guard) => guard,
        Err(singleton::SingletonError::AlreadyRunning) => {
            winutil::info_dialog(
                &t!("msg.already_running"),
                &t!("app.title"),
            );
            return Ok(());
        }
        Err(err) => return Err(err.into()),
    };

    let steam = match discover::discover() {
        Ok(steam) => steam,
        Err(err) => {
            winutil::info_dialog(
                &t!("msg.steam_not_found", error = err.to_string()),
                &t!("app.title"),
            );
            return Err(err.into());
        }
    };
    tracing::info!(root = %steam.root.display(), "steam discovered");

    run_gpui(steam);
    Ok(())
}

/// Bring up gpui, the tray and the event loop. Never returns.
fn run_gpui(steam: SteamInstall) {
    gpui_kit::application().run(move |cx: &mut App| {
        gpui_kit::init(cx);

        // Apply persisted locale + theme before anything renders.
        let config = Config::load();
        rust_i18n::set_locale(config.language());
        apply_theme(config.theme(), cx);

        let users = load_users_lenient(&steam);
        let games = library::installed_games(&steam).unwrap_or_default();
        let tray =
            Tray::new(&users, autostart::is_enabled(), &config).expect("failed to create tray icon");

        // Native window frame: real minimize/maximize/close buttons (the
        // custom TitleBar draws no working controls on Windows).
        let window_options = WindowOptions {
            window_min_size: Some(size(px(640.0), px(520.0))),
            show: false,
            titlebar: Some(TitlebarOptions {
                title: Some(t!("app.title").to_string().into()),
                appears_transparent: false,
                ..Default::default()
            }),
            ..Default::default()
        };
        let (handle, view) = gpui_kit::open_window(window_options, cx, |window, cx| {
            let users_for_window = users.clone();
            let games_for_window = games.clone();
            let config_for_window = config.clone();
            cx.new(|cx| {
                AppWindow::new(steam.clone(), users_for_window, games_for_window, config_for_window, window, cx)
            })
        })
        .expect("failed to open the app window");

        cx.set_global(AppState {
            tray,
            steam,
            window: Some(handle),
            view,
            window_shown: false,
            switching: Arc::new(AtomicBool::new(false)),
        });

        // Veto real closes: the X button minimizes instead (tray-app lifecycle).
        // The callback runs while the window is already on the update stack, so
        // re-entering `window.update` would silently fail — defer instead.
        let _ = handle.update(cx, |_, window, cx| {
            window.on_window_should_close(cx, |_window, cx| {
                cx.defer(|cx| minimize_window(cx));
                false
            });
        });

        // D2 bridge: poll the foreign event channels on the main thread with
        // async waits between polls, so the win32 pump stays responsive.
        cx.spawn(async move |cx| event_loop(cx).await).detach();

        tracing::info!("gui ready");
    });
}

/// Apply the configured theme mode (called at startup and from settings).
fn apply_theme(mode: &str, cx: &mut App) {
    match mode {
        "dark" => gpui_kit::component::Theme::change(ThemeMode::Dark, None, cx),
        "light" => gpui_kit::component::Theme::change(ThemeMode::Light, None, cx),
        // "system": follow the OS appearance.
        _ => gpui_kit::component::Theme::sync_system_appearance(None, cx),
    }
}

/// Drain tray-menu and tray-click events, dispatching each on the main
/// thread. Switch flows hop to the background executor while running.
async fn event_loop(cx: &mut AsyncApp) {
    use tray_icon::{MouseButton, MouseButtonState, TrayIconEvent};

    loop {
        let mut switch_requests: Vec<String> = Vec::new();
        let mut quit = false;

        while let Ok(event) = tray_icon::menu::MenuEvent::receiver().try_recv() {
            match tray::parse_action(&event.id) {
                Some(Action::Switch(steam_id64)) => switch_requests.push(steam_id64),
                Some(Action::Refresh) => {
                    cx.update(|cx| {
                        cx.global_mut::<AppState>().refresh_tray();
                        refresh_window(cx);
                    });
                }
                Some(Action::ToggleAutostart) => {
                    cx.update(|cx| {
                        match autostart::toggle() {
                            Ok(enabled) => notify::show(
                                &t!("app.title"),
                                &if enabled {
                                    t!("toast.autostart_on")
                                } else {
                                    t!("toast.autostart_off")
                                },
                            ),
                            Err(err) => {
                                tracing::error!(%err, "failed to toggle autostart");
                                notify::show(
                                    &t!("app.title"),
                                    &t!("toast.autostart_failed", error = err.to_string()),
                                );
                            }
                        }
                        cx.global_mut::<AppState>().refresh_tray();
                    });
                }
                Some(Action::Settings) => {
                    cx.update(|cx| {
                        show_window(cx);
                        refresh_window(cx);
                    });
                }
                Some(Action::Quit) => quit = true,
                None => {}
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
                        cx.global_mut::<AppState>().window_shown = !shown;
                        if !shown {
                            refresh_window(cx);
                        }
                    }
                });
            }
        }

        for steam_id64 in switch_requests {
            let switching = cx.update(|cx| cx.global::<AppState>().switching.clone());
            if switching.swap(true, Ordering::SeqCst) {
                notify::show(&t!("app.title"), &t!("toast.busy"));
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

/// Run the full switch flow on the background executor, then refresh tray and
/// window. Shared by the tray menu, the users page and game launches.
async fn do_switch(cx: &mut AsyncApp, steam_id64: &str, switching: &AtomicBool) {
    let steam = cx.update(|cx| cx.global::<AppState>().steam.clone());
    let steam_id64 = steam_id64.to_string();

    let outcome = cx
        .background_executor()
        .spawn(async move { perform_switch(&steam, &steam_id64) })
        .await;

    cx.update(|cx| {
        cx.global_mut::<AppState>().refresh_tray();
        refresh_window(cx);
        if let Err(err) = &outcome {
            tracing::error!(%err, "switch failed");
            notify::show(&t!("app.title"), &t!("toast.switch_failed", error = err.to_string()));
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
        // Menu or list was stale; the caller refreshes the UI after.
        notify::show(&t!("app.title"), &t!("toast.stale_account"));
        return Ok(SwitchOutcome::CancelledByUser);
    };
    let label = target.display_name().to_string();

    notify::show(&t!("app.title"), &t!("toast.switching", name = label.clone()));

    // Config-gated, non-skippable confirmation when Steam games are running.
    let confirm = |games: &[String]| -> bool {
        if !Config::load().confirm_before_switch {
            return true;
        }
        winutil::confirm_dialog(
            &t!(
                "msg.games_running",
                games = games.join("\n")
            ),
            &t!("msg.games_running_title"),
        )
    };

    match switch::switch_to_account(steam, steam_id64, &confirm) {
        Ok(SwitchOutcome::Switched) => {
            notify::show(&t!("app.title"), &t!("toast.switched", name = label));
            Ok(SwitchOutcome::Switched)
        }
        Ok(SwitchOutcome::AlreadyActive) => {
            notify::show(&t!("app.title"), &t!("toast.already_active", name = label));
            Ok(SwitchOutcome::AlreadyActive)
        }
        other => other,
    }
}

// --------------------------------------------------------------------------
// Entry points used by the window UI (all run on the main thread).
// --------------------------------------------------------------------------

/// Reload accounts; on failure keep an empty list and toast instead of dying.
pub fn load_users_lenient(steam: &SteamInstall) -> Vec<SteamUser> {
    match users::load_users(steam) {
        Ok(users) => users,
        Err(err) => {
            tracing::error!(%err, "failed to load loginusers.vdf");
            notify::show(&t!("app.title"), &t!("toast.read_failed", error = err.to_string()));
            Vec::new()
        }
    }
}

/// Switch to the given account (fire and forget; toasts report progress).
pub fn request_switch(cx: &mut App, steam_id64: String) {
    let switching = cx.global::<AppState>().switching.clone();
    if switching.swap(true, Ordering::SeqCst) {
        notify::show(&t!("app.title"), &t!("toast.busy"));
        return;
    }
    cx.spawn(async move |cx| do_switch(cx, &steam_id64, &switching).await)
        .detach();
}

/// Switch to the account, then launch the game once Steam is back up.
pub fn request_switch_then_launch(cx: &mut App, steam_id64: String, appid: u32) {
    let switching = cx.global::<AppState>().switching.clone();
    if switching.swap(true, Ordering::SeqCst) {
        notify::show(&t!("app.title"), &t!("toast.busy"));
        return;
    }
    cx.spawn(async move |cx| {
        do_switch(cx, &steam_id64, &switching).await;
        // The switch either restarted Steam or it was already running; the
        // protocol queues requests while Steam boots (docs/research-game-list.md).
        cx.background_executor().timer(LAUNCH_DELAY).await;
        if let Err(err) = crate::steam::launch::launch_game(appid) {
            tracing::error!(%err, "failed to launch game");
        }
    })
    .detach();
}

/// Restart Steam cleanly from the sidebar footer button.
pub fn request_restart_steam(cx: &mut App) {
    let steam = cx.global::<AppState>().steam.clone();
    notify::show(&t!("app.title"), &t!("toast.restarting_steam"));
    cx.spawn(async move |cx| {
        let result = cx
            .background_executor()
            .spawn(async move { switch::restart_steam(&steam) })
            .await;
        cx.update(|cx| {
            match result {
                Ok(()) => notify::show(&t!("app.title"), &t!("toast.restarted_steam")),
                Err(err) => {
                    tracing::error!(%err, "failed to restart steam");
                    notify::show(&t!("app.title"), &t!("toast.switch_failed", error = err.to_string()));
                }
            }
            refresh_window(cx);
        });
    })
    .detach();
}

/// Persist the language choice and re-render everything (tray included).
pub fn set_language(cx: &mut App, code: &'static str) {
    let mut config = Config::load();
    config.language = Some(code.to_string());
    if let Err(err) = config.save() {
        tracing::error!(%err, "failed to save config");
    }
    rust_i18n::set_locale(code);
    cx.global_mut::<AppState>().refresh_tray();
    refresh_window(cx);
}

/// Persist the theme choice and apply it immediately.
pub fn set_theme(cx: &mut App, code: &'static str) {
    let mut config = Config::load();
    config.theme = Some(code.to_string());
    if let Err(err) = config.save() {
        tracing::error!(%err, "failed to save config");
    }
    apply_theme(code, cx);
    refresh_window(cx);
}

/// Sidebar navigation: switch the visible page.
pub fn select_page(cx: &mut App, page: Page) {
    with_window_view(cx, |view, cx| {
        view.page = page;
        cx.notify();
    });
}

/// Sidebar collapse toggle.
pub fn toggle_sidebar(cx: &mut App) {
    with_window_view(cx, |view, cx| {
        view.sidebar_collapsed = !view.sidebar_collapsed;
        cx.notify();
    });
}

/// A game card was clicked: show the account picker for it.
pub fn pick_game(cx: &mut App, appid: u32) {
    with_window_view(cx, |view, cx| {
        view.picked_game = view.games.iter().find(|g| g.appid == appid).cloned();
        view.page = Page::Games;
        cx.notify();
    });
}

/// Close the account picker without launching anything.
pub fn cancel_game_pick(cx: &mut App) {
    with_window_view(cx, |view, cx| {
        view.picked_game = None;
        cx.notify();
    });
}

// --------------------------------------------------------------------------
// Window plumbing helpers.
// --------------------------------------------------------------------------

/// Reload the window view's data (config/accounts/games) and re-render.
pub(crate) fn refresh_window(cx: &mut App) {
    with_window_view(cx, |view, cx| {
        view.reload_data();
        cx.notify();
    });
}

/// Restore/activate the app window.
fn show_window(cx: &mut App) {
    let window = cx.global_mut::<AppState>().window;
    if let Some(window) = window {
        // Err = the window was closed meanwhile; nothing to show.
        let _ = window.update(cx, |_, window, _| window.activate_window());
    }
    cx.global_mut::<AppState>().window_shown = true;
}

/// Minimize the app window (gpui has no hide; minimize is the tray-app
/// compromise planned in docs/plan.md P2).
fn minimize_window(cx: &mut App) {
    let window = cx.global_mut::<AppState>().window;
    if let Some(window) = window {
        let _ = window.update(cx, |_, window, _| window.minimize_window());
    }
    cx.global_mut::<AppState>().window_shown = false;
}

/// Run `f` against the app window's content view. Uses the view entity rather
/// than the window handle because the window's root is `base::Root` (a
/// `downcast::<AppWindow>` on it can never succeed).
fn with_window_view(cx: &mut App, f: impl FnOnce(&mut AppWindow, &mut gpui_kit::Context<AppWindow>)) {
    let view = cx.global::<AppState>().view.clone();
    view.update(cx, f);
}
