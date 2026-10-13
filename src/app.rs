//! Application bootstrap: gpui main loop, tray event bridging (D2), and the
//! switch flows shared by the tray menu and the window UI.
//!
//! Threading model (Phase 2): gpui owns the main thread and its win32 message
//! pump, so the tray icon and the app window live there. Menu/tray-click
//! events arrive on foreign global channels; a single foreground task polls
//! them and awaits a short timer between polls (never blocks the pump). The
//! blocking switch flow itself runs on the background executor and reports
//! back through `AsyncApp::update`. A separate background task watches
//! Steam's process/account state and pushes changes into the window view
//! (edge-triggered, so a quiet poll never re-renders).

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::WindowExt;
use gpui_kit::{Anchor, AnyWindowHandle, App, AppContext, AsyncApp, Entity, Global, TitlebarOptions, WindowOptions, px, size};
use rust_i18n::t;

use crate::autostart;
use crate::config::Config;
use crate::logging;
use crate::notify;
use crate::app_window::{AppWindow, Page};
use crate::singleton;
use crate::steam::discover::{self, SteamInstall};
use crate::steam::library;
use crate::steam::switch::{self, SwitchOutcome, SwitchStage};
use crate::steam::users;
use crate::switch_flow::{load_users_lenient, perform_switch, resolve_active_steam_id64};
use crate::tray::{self, Action, Tray};
use crate::winutil;

/// Poll interval while waiting for foreign tray events.
const EVENT_POLL: Duration = Duration::from_millis(30);
/// Delay before firing the steam:// URL after a switch (Steam must boot; the
/// protocol queues internally — docs/research-game-list.md).
const LAUNCH_DELAY: Duration = Duration::from_secs(3);
/// Poll interval for the Steam process/account status watcher. A full
/// enumeration costs a few ms; 2s keeps the UI current without measurable load.
const STATUS_POLL: Duration = Duration::from_secs(2);

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
    /// Progress milestone of the in-flight switch (a [`SwitchStage`] number,
    /// 0 = idle). Written by the background switch flow, read by the status
    /// watcher which forwards stage changes into the window view.
    switch_stage: Arc<AtomicU8>,
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

/// Embedded icon catalog with a filesystem fallback.
///
/// gpui serves every image through one asset source: the build-script
/// generated [`crate::app_icons`] embeds only the icons referenced under
/// `src/`, `gpui_kit::assets::Assets` adds the gpui-component defaults
/// (instead of `AllAssets`, which would embed the whole ~1830-icon Lucide
/// catalog), and game covers plus Steam avatars live on disk (absolute
/// paths), so `load` falls back to `std::fs` for anything not embedded.
struct AppAssets;

impl gpui_kit::AssetSource for AppAssets {
    fn load(&self, path: &str) -> anyhow::Result<Option<std::borrow::Cow<'static, [u8]>>> {
        if let Some(bytes) = crate::app_icons::load(path) {
            return Ok(Some(std::borrow::Cow::Borrowed(bytes)));
        }
        if let Ok(Some(bytes)) = gpui_kit::assets::Assets.load(path) {
            return Ok(Some(bytes));
        }
        match std::fs::read(path) {
            Ok(bytes) => Ok(Some(std::borrow::Cow::Owned(bytes))),
            Err(_) => Ok(None),
        }
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<gpui_kit::SharedString>> {
        let mut names = gpui_kit::assets::Assets.list(path)?;
        names.extend(
            crate::app_icons::list()
                .into_iter()
                .filter(|name| name.starts_with(path))
                .map(gpui_kit::SharedString::from),
        );
        Ok(names)
    }
}

/// Bring up gpui, the tray and the event loop. Never returns.
fn run_gpui(steam: SteamInstall) {
    gpui_kit::application()
        // Icons from the embedded Lucide catalog, covers/avatars from disk.
        .with_assets(AppAssets)
        .run(move |cx: &mut App| {
        gpui_kit::init(cx);

        // Apply persisted locale + theme before anything renders.
        let config = Config::load();
        rust_i18n::set_locale(config.language());
        crate::theme::apply(config.theme(), cx);

        let users = load_users_lenient(&steam);
        let games = library::installed_games(&steam).unwrap_or_default();
        let tray =
            Tray::new(&users, autostart::is_enabled(), &config).expect("failed to create tray icon");

        // Frameless window with a custom title bar (the app draws brand,
        // current-account pill and min/max/close inside a drag area).
        let window_options = WindowOptions {
            window_min_size: Some(size(px(640.0), px(520.0))),
            show: false,
            titlebar: Some(TitlebarOptions {
                title: Some(t!("app.title").to_string().into()),
                appears_transparent: true,
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
            switch_stage: Arc::new(AtomicU8::new(0)),
        });

        // Veto real closes: with tray-minimize on (the default) the X button
        // minimizes instead; with it off, closing quits. The callback runs
        // while the window is already on the update stack, so re-entering
        // `window.update` would silently fail — defer instead.
        let _ = handle.update(cx, |_, window, cx| {
            window.on_window_should_close(cx, |_window, cx| {
                cx.defer(close_main_window);
                false
            });
        });

        // D2 bridge: poll the foreign event channels on the main thread with
        // async waits between polls, so the win32 pump stays responsive.
        cx.spawn(async move |cx| event_loop(cx).await).detach();

        // Steam status watcher: feeds the sidebar footer light, the live
        // "in use" badge and the switch-overlay stage (see status_watcher).
        let state = cx.global::<AppState>();
        let (watch_steam, switching, switch_stage) =
            (state.steam.clone(), state.switching.clone(), state.switch_stage.clone());
        cx.spawn(async move |cx| status_watcher(watch_steam, switching, switch_stage, cx).await)
            .detach();

        tracing::info!("gui ready");
    });
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
                        // Keep the window's settings switch in step.
                        refresh_window(cx);
                        cx.global_mut::<AppState>().refresh_tray();
                    });
                }
                Some(Action::Settings) => {
                    cx.update(|cx| {
                        show_window(cx);
                        refresh_window(cx);
                    });
                }
                Some(Action::OpenWindow) => {
                    cx.update(|cx| {
                        show_window(cx);
                        refresh_window(cx);
                    });
                }
                Some(Action::SetLanguage(lang)) => {
                    cx.update(|cx| set_language(cx, &lang));
                }
                Some(Action::ToggleKillGames) => {
                    cx.update(|cx| toggle_config_flag(cx, |config| {
                        config.kill_games_before_switch = !config.kill_games_before_switch;
                    }));
                }
                Some(Action::ToggleConfirmSwitch) => {
                    cx.update(|cx| toggle_config_flag(cx, |config| {
                        config.confirm_before_switch = !config.confirm_before_switch;
                    }));
                }
                Some(Action::ToggleMinimizeToTray) => {
                    cx.update(|cx| toggle_config_flag(cx, |config| {
                        config.minimize_to_tray = !config.minimize_to_tray;
                    }));
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
            cx.update(|cx| begin_switch(cx, &steam_id64));
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

/// Watch Steam's process and account state on the background executor and
/// push changes into the window view. Edge-triggered: the poll only touches
/// the UI when the watched facts actually changed, so a quiet poll never
/// re-renders. While a switch is in flight Steam is deliberately bounced;
/// account/process pushes are skipped and only the switch's progress stage is
/// forwarded (driving the overlay).
async fn status_watcher(
    steam: SteamInstall,
    switching: Arc<AtomicBool>,
    switch_stage: Arc<AtomicU8>,
    cx: &mut AsyncApp,
) {
    let mut last: Option<(bool, Option<String>)> = None;
    let mut last_stage: u8 = 0;
    loop {
        // Per-iteration clone: the spawned block needs ownership for 'static.
        let steam = steam.clone();
        let snapshot = cx
            .background_executor()
            .spawn(async move {
                let running = switch::steam_running();
                let active = users::load_users(&steam)
                    .ok()
                    .and_then(|users| resolve_active_steam_id64(&users));
                (running, active)
            })
            .await;

        if switching.load(Ordering::SeqCst) {
            // Forward switch progress stages into the overlay.
            let stage = switch_stage.load(Ordering::SeqCst);
            if stage != last_stage {
                last_stage = stage;
                cx.update(|cx| {
                    with_window_view(cx, |view, cx| {
                        view.switch_stage = SwitchStage::from_u8(stage);
                        cx.notify();
                    });
                });
            }
        } else if last.as_ref() != Some(&snapshot) {
            last = Some(snapshot.clone());
            let (running, active) = snapshot;
            cx.update(|cx| {
                with_window_view(cx, |view, cx| {
                    view.steam_running = running;
                    view.active_steam_id64 = active;
                    cx.notify();
                });
            });
        }

        cx.background_executor().timer(STATUS_POLL).await;
    }
}

/// Run the full switch flow on the background executor, then refresh tray and
/// window. Shared by the tray menu, the users page and game launches.
async fn do_switch(cx: &mut AsyncApp, steam_id64: &str, switching: &AtomicBool) {
    let (steam, stage_slot, quiet) = cx.update(|cx| {
        let state = cx.global::<AppState>();
        (state.steam.clone(), state.switch_stage.clone(), state.window_shown)
    });

    let outcome = {
        let stage_slot = stage_slot.clone();
        let steam_id64 = steam_id64.to_string();
        cx.background_executor()
            .spawn(async move { perform_switch(&steam, &steam_id64, quiet, &stage_slot) })
            .await
    };
    stage_slot.store(0, Ordering::SeqCst);

    cx.update(|cx| {
        with_window_view(cx, |view, cx| {
            view.switching = false;
            view.switch_stage = None;
            view.switch_target = None;
            cx.notify();
        });
        cx.global_mut::<AppState>().refresh_tray();
        refresh_window(cx);
        match &outcome {
            Err(err) => {
                tracing::error!(%err, "switch failed");
                if quiet {
                    toast(cx, Notification::error(t!("toast.switch_failed", error = err.to_string()).to_string()));
                } else {
                    notify::show(&t!("app.title"), &t!("toast.switch_failed", error = err.to_string()));
                }
            }
            Ok(SwitchOutcome::CancelledByUser) => {
                if quiet {
                    toast(cx, Notification::warning(t!("toast.cancelled").to_string()));
                } else {
                    notify::show(&t!("app.title"), &t!("toast.cancelled"));
                }
            }
            // Switched / AlreadyActive were already announced by the native
            // toasts inside perform_switch on the non-quiet path.
            Ok(_) => {}
        }
    });
    switching.store(false, Ordering::SeqCst);
}

// --------------------------------------------------------------------------
// Entry points used by the window UI (all run on the main thread).
// --------------------------------------------------------------------------

/// Push an in-app toast into the app window (no-op when the window is gone).
fn toast(cx: &mut App, note: Notification) {
    let window = cx.global::<AppState>().window;
    if let Some(window) = window {
        let _ = window.update(cx, |_, window, cx| {
            window.push_notification(note.placement(Anchor::BottomRight), cx)
        });
    }
}

/// Show the switch overlay for `steam_id64` (the switch itself is started by
/// the caller, which owns the `switching` guard).
fn begin_switch(cx: &mut App, steam_id64: &str) {
    with_window_view(cx, |view, cx| {
        view.switching = true;
        view.switch_stage = Some(SwitchStage::Prepare);
        view.switch_target = view
            .users
            .iter()
            .find(|u| u.steam_id64 == steam_id64)
            .map(|u| view.config.display_for(u));
        cx.notify();
    });
}

/// Switch to the given account (overlay + in-app toasts report progress).
pub fn request_switch(cx: &mut App, steam_id64: String) {
    let switching = cx.global::<AppState>().switching.clone();
    if switching.swap(true, Ordering::SeqCst) {
        toast(cx, Notification::warning(t!("toast.busy").to_string()));
        return;
    }
    begin_switch(cx, &steam_id64);
    cx.spawn(async move |cx| do_switch(cx, &steam_id64, &switching).await)
        .detach();
}

/// Launch a game right now with whatever account Steam is logged into
/// (games-page left click). No switching happens; the steam:// protocol
/// boots Steam if it is not running.
pub fn request_launch_game(cx: &mut App, appid: u32) {
    toast(cx, Notification::info(t!("toast.launching_game").to_string()));
    cx.spawn(async move |cx| {
        let launched = cx
            .background_executor()
            .spawn(async move { crate::steam::launch::launch_game(appid) })
            .await;
        if let Err(err) = launched {
            tracing::error!(%err, "failed to launch game");
            cx.update(|cx| {
                toast(
                    cx,
                    Notification::error(t!("toast.game_launch_failed", error = err.to_string()).to_string()),
                );
            });
        }
    })
    .detach();
}

/// Switch to the account, then launch the game once Steam is back up.
pub fn request_switch_then_launch(cx: &mut App, steam_id64: String, appid: u32) {
    let switching = cx.global::<AppState>().switching.clone();
    if switching.swap(true, Ordering::SeqCst) {
        toast(cx, Notification::warning(t!("toast.busy").to_string()));
        return;
    }
    begin_switch(cx, &steam_id64);
    cx.spawn(async move |cx| {
        do_switch(cx, &steam_id64, &switching).await;
        // The switch either restarted Steam or it was already running; the
        // protocol queues requests while Steam boots (docs/research-game-list.md).
        cx.update(|cx| {
            with_window_view(cx, |view, cx| {
                view.switching = true;
                view.switch_stage = Some(SwitchStage::LaunchingGame);
                view.switch_target = None;
                cx.notify();
            })
        });
        cx.background_executor().timer(LAUNCH_DELAY).await;
        let launched = crate::steam::launch::launch_game(appid);
        cx.update(|cx| {
            with_window_view(cx, |view, cx| {
                view.switching = false;
                view.switch_stage = None;
                cx.notify();
            });
            if let Err(err) = launched {
                tracing::error!(%err, "failed to launch game");
                toast(
                    cx,
                    Notification::error(t!("toast.game_launch_failed", error = err.to_string()).to_string()),
                );
            }
        });
    })
    .detach();
}

/// Restart Steam cleanly from the sidebar footer button.
pub fn request_restart_steam(cx: &mut App) {
    let steam = cx.global::<AppState>().steam.clone();
    let launch_args = Config::load().launch_args_tokens();
    toast(cx, Notification::info(t!("toast.restarting_steam").to_string()));
    cx.spawn(async move |cx| {
        let result = cx
            .background_executor()
            .spawn(async move { switch::restart_steam(&steam, &launch_args) })
            .await;
        cx.update(|cx| {
            match result {
                Ok(()) => toast(cx, Notification::success(t!("toast.restarted_steam").to_string())),
                Err(err) => {
                    tracing::error!(%err, "failed to restart steam");
                    toast(
                        cx,
                        Notification::error(t!("toast.switch_failed", error = err.to_string()).to_string()),
                    );
                }
            }
            refresh_window(cx);
        });
    })
    .detach();
}

/// Remove an account from `loginusers.vdf` after the confirm dialog's OK.
/// The last remaining account is refused; a stale registry `AutoLoginUser`
/// pointing at the removed account is cleared.
pub fn request_remove_account(cx: &mut App, steam_id64: String) {
    let steam = cx.global::<AppState>().steam.clone();
    cx.spawn(async move |cx| {
        let result = cx
            .background_executor()
            .spawn(async move { users::remove_account(&steam, &steam_id64) })
            .await;
        cx.update(|cx| match result {
            Ok(Some(removed)) => {
                if switch::autologin_user().as_deref() == Some(removed.account_name.as_str())
                    && let Err(err) = switch::clear_autologin_registry()
                {
                    tracing::error!(%err, "failed to clear registry AutoLoginUser");
                }
                toast(cx, Notification::success(t!("users.deleted").to_string()));
                cx.global_mut::<AppState>().refresh_tray();
                refresh_window(cx);
            }
            Ok(None) => {
                toast(cx, Notification::warning(t!("users.last_account").to_string()));
            }
            Err(err) => {
                tracing::error!(%err, "failed to remove account");
                toast(
                    cx,
                    Notification::error(t!("users.delete_failed", error = err.to_string()).to_string()),
                );
            }
        });
    })
    .detach();
}

/// Re-run Steam discovery (settings page "auto detect"); adopts the new
/// install when found, keeping the old one (and saying so) otherwise.
pub fn request_detect_steam_path(cx: &mut App) {
    cx.spawn(async move |cx| {
        let result = cx
            .background_executor()
            .spawn(async move { discover::discover() })
            .await;
        cx.update(|cx| match result {
            Ok(steam) => {
                cx.global_mut::<AppState>().steam = steam;
                refresh_window(cx);
                toast(cx, Notification::success(t!("toast.path_detected").to_string()));
            }
            Err(err) => {
                tracing::error!(%err, "steam auto-detect failed");
                toast(
                    cx,
                    Notification::error(t!("toast.detect_failed", error = err.to_string()).to_string()),
                );
            }
        });
    })
    .detach();
}

/// Persist the language choice and re-render everything (tray included).
pub fn set_language(cx: &mut App, code: &str) {
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
    crate::theme::apply(code, cx);
    refresh_window(cx);
}

/// Persist the Steam launch arguments. Called when the settings-page input
/// loses focus (auto-save) — silent by design, the field simply keeps the
/// value. Empty input returns to the `-silent` default.
pub fn set_launch_args(args: String) {
    let mut config = Config::load();
    let trimmed = args.trim();
    config.launch_args = if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    };
    if let Err(err) = config.save() {
        tracing::error!(%err, "failed to save config");
    }
}

/// Flip one persisted config flag, then sync both UIs (tray menu check marks
/// and the window's settings switches) with the new state. This is the single
/// code path behind the tray check items and the GUI switches, so the two
/// can never drift apart.
fn toggle_config_flag(cx: &mut App, f: impl FnOnce(&mut Config)) {
    let mut config = Config::load();
    f(&mut config);
    if let Err(err) = config.save() {
        tracing::error!(%err, "failed to save config");
    }
    cx.global_mut::<AppState>().refresh_tray();
    refresh_window(cx);
}

/// Persist the close-to-tray behavior (GUI switch and tray check item).
pub fn set_minimize_to_tray(cx: &mut App, checked: bool) {
    toggle_config_flag(cx, |config| config.minimize_to_tray = checked);
}

/// Persist the "kill running games without asking" switch.
pub fn set_kill_games_before_switch(cx: &mut App, checked: bool) {
    toggle_config_flag(cx, |config| config.kill_games_before_switch = checked);
}

/// Persist the confirm-before-switch switch.
pub fn set_confirm_before_switch(cx: &mut App, checked: bool) {
    toggle_config_flag(cx, |config| config.confirm_before_switch = checked);
}

/// Enable/disable launch-on-startup (GUI switch and tray check item).
pub fn set_autostart(cx: &mut App, checked: bool) {
    if let Err(err) = autostart::set_enabled(checked) {
        tracing::error!(%err, "failed to set autostart");
    }
    cx.global_mut::<AppState>().refresh_tray();
    refresh_window(cx);
}

/// The title bar close button / native close: minimize to tray when enabled,
/// otherwise quit the whole app.
pub fn close_main_window(cx: &mut App) {
    if Config::load().minimize_to_tray {
        minimize_window(cx);
    } else {
        tracing::info!("quit requested from window close");
        cx.quit();
    }
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
