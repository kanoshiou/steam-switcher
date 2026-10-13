//! Tray-only application loop: tray icon + win32 message pump, no gpui window.
//!
//! This is the `--no-default-features` build (the Phase-1 design from
//! docs/plan.md): the switcher lives entirely in the tray menu. The tray icon
//! only receives events while its creating thread pumps messages, so the main
//! thread polls: drain queued win32 messages, drain menu events, sleep. Switch
//! flows run on a background `std::thread` so the pump (and therefore the
//! open menu) stays responsive; completion is reported back over a channel
//! because the tray menu must only be touched from the creating thread.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use rust_i18n::t;

use crate::autostart;
use crate::config::Config;
use crate::logging;
use crate::notify;
use crate::singleton;
use crate::steam::discover::{self, SteamInstall};
use crate::switch_flow::{load_users_lenient, perform_switch};
use crate::tray::{self, Action, Tray};
use crate::winutil;

/// Poll interval between pump passes (mirrors the gui build's tray bridge).
const EVENT_POLL: Duration = Duration::from_millis(30);

/// Start the tray-only application. Returns when the user picks Quit.
pub fn run() -> anyhow::Result<()> {
    logging::init().context("initializing logging")?;
    tracing::info!(
        "steam-switcher v{} starting (tray-only build)",
        env!("CARGO_PKG_VERSION")
    );

    // Single instance: a second launch just nudges the user and exits cleanly.
    let _singleton = match singleton::acquire() {
        Ok(guard) => guard,
        Err(singleton::SingletonError::AlreadyRunning) => {
            winutil::info_dialog(&t!("msg.already_running"), &t!("app.title"));
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

    let config = Config::load();
    rust_i18n::set_locale(config.language());
    notify::register_app_id();
    let users = load_users_lenient(&steam);
    let tray =
        Tray::new(&users, autostart::is_enabled(), &config).context("failed to create tray icon")?;

    let switching = Arc::new(AtomicBool::new(false));
    let stage_slot = Arc::new(AtomicU8::new(0));
    let (done_tx, done_rx) = mpsc::channel::<()>();

    loop {
        // The tray icon only receives events while this thread pumps messages.
        winutil::pump_pending_messages();

        while let Ok(event) = tray_icon::menu::MenuEvent::receiver().try_recv() {
            match tray::parse_action(&event.id) {
                Some(Action::Switch(steam_id64)) => {
                    if switching.swap(true, Ordering::SeqCst) {
                        notify::show(&t!("toast.busy"));
                        continue;
                    }
                    spawn_switch(
                        steam.clone(),
                        steam_id64,
                        stage_slot.clone(),
                        done_tx.clone(),
                    );
                }
                Some(Action::Refresh) => refresh(&tray, &steam),
                Some(Action::ToggleAutostart) => {
                    match autostart::toggle() {
                        Ok(enabled) => notify::show(&if enabled {
                            t!("toast.autostart_on")
                        } else {
                            t!("toast.autostart_off")
                        }),
                        Err(err) => {
                            tracing::error!(%err, "failed to toggle autostart");
                            notify::show(&t!("toast.autostart_failed", error = err.to_string()));
                        }
                    }
                    refresh(&tray, &steam);
                }
                Some(Action::SetLanguage(lang)) => {
                    let mut config = Config::load();
                    config.language = Some(lang.clone());
                    if let Err(err) = config.save() {
                        tracing::error!(%err, "failed to save config");
                    }
                    rust_i18n::set_locale(&lang);
                    refresh(&tray, &steam);
                }
                Some(Action::ToggleKillGames) => {
                    toggle_config_flag(|config| {
                        config.kill_games_before_switch = !config.kill_games_before_switch
                    });
                    refresh(&tray, &steam);
                }
                Some(Action::ToggleConfirmSwitch) => {
                    toggle_config_flag(|config| {
                        config.confirm_before_switch = !config.confirm_before_switch
                    });
                    refresh(&tray, &steam);
                }
                Some(Action::ToggleMinimizeToTray) => {
                    toggle_config_flag(|config| {
                        config.minimize_to_tray = !config.minimize_to_tray
                    });
                    refresh(&tray, &steam);
                }
                // The window menu items ("open", and the settings window) do
                // not exist in this build, so those events never arrive.
                Some(Action::Settings) | Some(Action::OpenWindow) => {}
                Some(Action::Quit) => {
                    tracing::info!("quit requested, exiting");
                    return Ok(());
                }
                None => {}
            }
        }

        // Finished background switches hand control back: rebuild the menu
        // here on the pump thread (the one that created the tray icon).
        while done_rx.try_recv().is_ok() {
            switching.store(false, Ordering::SeqCst);
            refresh(&tray, &steam);
        }

        std::thread::sleep(EVENT_POLL);
    }
}

/// Run the switch flow on a background thread; the completion signal makes
/// the main loop clear the busy flag and rebuild the menu.
fn spawn_switch(
    steam: SteamInstall,
    steam_id64: String,
    stage_slot: Arc<AtomicU8>,
    done_tx: mpsc::Sender<()>,
) {
    std::thread::spawn(move || {
        let outcome = perform_switch(&steam, &steam_id64, false, &stage_slot);
        if let Err(err) = outcome {
            tracing::error!(%err, "switch failed");
            notify::show(&t!("toast.switch_failed", error = err.to_string()));
        }
        let _ = done_tx.send(());
    });
}

/// Re-read accounts + config from disk and rebuild the tray menu.
fn refresh(tray: &Tray, steam: &SteamInstall) {
    let users = load_users_lenient(steam);
    tray.update_menu(&users, autostart::is_enabled(), &Config::load());
}

/// Flip one persisted config flag. The tray menu is rebuilt by the caller so
/// the check marks follow the new state.
fn toggle_config_flag(f: impl FnOnce(&mut Config)) {
    let mut config = Config::load();
    f(&mut config);
    if let Err(err) = config.save() {
        tracing::error!(%err, "failed to save config");
    }
}
