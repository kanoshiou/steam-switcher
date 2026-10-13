//! Switch-flow glue shared verbatim by the gui and tray-only builds: the
//! blocking switch execution (with its native toasts and confirm dialog),
//! lenient account loading, and active-account resolution. Everything here is
//! gpui-free so the tray-only build can reuse it on a background thread.

use std::sync::atomic::{AtomicU8, Ordering};

use rust_i18n::t;

use crate::config::Config;
use crate::notify;
use crate::steam::discover::SteamInstall;
use crate::steam::switch::{self, SwitchOutcome, SwitchStage};
use crate::steam::users::{self, SteamUser};
use crate::winutil;

/// Reload accounts; on failure keep an empty list and toast instead of dying.
pub fn load_users_lenient(steam: &SteamInstall) -> Vec<SteamUser> {
    match users::load_users(steam) {
        Ok(users) => users,
        Err(err) => {
            tracing::error!(%err, "failed to load loginusers.vdf");
            notify::show(&t!("toast.read_failed", error = err.to_string()));
            Vec::new()
        }
    }
}

/// Resolve the account currently in use. The registry `AutoLoginUser` is the
/// stronger signal (Steam rewrites it on every in-client login); the vdf
/// `MostRecent` flag is the fallback when the registry value is unset or
/// points at an account no longer present in `loginusers.vdf`.
pub fn resolve_active_steam_id64(users: &[SteamUser]) -> Option<String> {
    if let Some(name) = switch::autologin_user()
        && let Some(user) = users.iter().find(|u| u.account_name == name)
    {
        return Some(user.steam_id64.clone());
    }
    users::active_user(users).map(|u| u.steam_id64.clone())
}

/// The blocking part of a switch: stale-target healing, game confirmation,
/// Steam shutdown, vdf + registry writes, relaunch. Runs off the main thread
/// (background executor in the gui build, `std::thread` in the tray-only
/// build). When `quiet` (the window is visible) native OS toasts are skipped —
/// the window's overlay and in-app toasts carry the feedback instead.
pub fn perform_switch(
    steam: &SteamInstall,
    steam_id64: &str,
    quiet: bool,
    stage_slot: &AtomicU8,
) -> Result<SwitchOutcome, switch::SwitchError> {
    let progress = |stage: SwitchStage| stage_slot.store(stage.as_u8(), Ordering::SeqCst);

    let users = users::load_users(steam)?;
    let Some(target) = users.iter().find(|u| u.steam_id64 == steam_id64) else {
        // Menu or list was stale; the caller refreshes the UI after.
        notify::show(&t!("toast.stale_account"));
        return Ok(SwitchOutcome::CancelledByUser);
    };
    let label = target.display_name().to_string();

    if !quiet {
        notify::show(&t!("toast.switching", name = label.clone()));
    }

    // Games running: kill them silently when configured, else ask (the
    // dialog must not be skippable when enabled), else proceed.
    let confirm = |games: &[String]| -> bool {
        let config = Config::load();
        if config.kill_games_before_switch || !config.confirm_before_switch {
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

    let launch_args = Config::load().launch_args_tokens();
    match switch::switch_to_account(steam, steam_id64, &confirm, &progress, &launch_args) {
        Ok(SwitchOutcome::Switched) => {
            if !quiet {
                notify::show(&t!("toast.switched", name = label));
            }
            Ok(SwitchOutcome::Switched)
        }
        Ok(SwitchOutcome::AlreadyActive) => {
            if !quiet {
                notify::show(&t!("toast.already_active", name = label));
            }
            Ok(SwitchOutcome::AlreadyActive)
        }
        other => other,
    }
}
