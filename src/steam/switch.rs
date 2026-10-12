//! The account switch orchestration (AGENTS.md "Switching recipe"):
//! detect running games → confirm → quit Steam gracefully → patch
//! `loginusers.vdf` → write registry auto-login → relaunch Steam.
//!
//! Key insight (docs/research-vdf.md): registry `AutoLoginUser` decides who
//! Steam *silently* logs in; the vdf `MostRecent` only pre-selects on the login
//! screen. Both must be written for a seamless switch.

use std::io::ErrorKind;
use std::path::Path;
use std::time::{Duration, Instant};

use sysinfo::{Pid, ProcessesToUpdate, System};
use thiserror::Error;

use crate::steam::discover::{DiscoverError, SteamInstall};
use crate::steam::users;
use crate::steam::vdf::{self, VdfError};

/// How long to wait after asking Steam to exit before resorting to stronger measures.
const GRACEFUL_TIMEOUT: Duration = Duration::from_secs(7);
/// Extra wait after force-killing Steam, so the OS reaps the process for good.
const FORCE_KILL_WAIT: Duration = Duration::from_secs(3);
/// Poll interval while waiting for Steam to exit.
const WAIT_TICK: Duration = Duration::from_millis(250);

/// Launch arguments passed to `steam.exe` on relaunch (see AGENTS.md D3 —
/// whether `-silent` is required is still pending real-machine verification).
const STEAM_LAUNCH_ARGS: &[&str] = &["-silent"];

/// Errors that can abort a switch.
#[derive(Debug, Error)]
pub enum SwitchError {
    /// Steam installation could not be located.
    #[error(transparent)]
    SteamMissing(#[from] DiscoverError),
    /// Reading or writing `loginusers.vdf` failed.
    #[error(transparent)]
    Vdf(#[from] VdfError),
    /// Process or registry I/O failed.
    #[error("io failure during switch: {0}")]
    Io(#[from] std::io::Error),
    /// The requested account is not present in `loginusers.vdf`.
    #[error("account {0} is not present in loginusers.vdf")]
    UnknownAccount(String),
}

/// Result of a switch attempt that didn't hard-fail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwitchOutcome {
    /// Steam was restarted logged into the target account.
    Switched,
    /// The target was already the active account; nothing was touched.
    AlreadyActive,
    /// The user declined the running-games confirmation dialog.
    CancelledByUser,
}

/// Switch the machine to the account identified by `steam_id64`.
///
/// `confirm_games` is called with the list of running Steam games whenever any
/// are detected; returning `false` aborts the switch. The dialog must not be
/// skippable (engineering conventions) — the caller owns its implementation.
pub fn switch_to_account(
    steam: &SteamInstall,
    steam_id64: &str,
    confirm_games: &dyn Fn(&[String]) -> bool,
) -> Result<SwitchOutcome, SwitchError> {
    // Re-read from disk so we never patch a stale account list.
    let mut users = users::load_users(steam)?;
    let target = users
        .iter()
        .find(|u| u.steam_id64 == steam_id64)
        .ok_or_else(|| SwitchError::UnknownAccount(steam_id64.to_string()))?;

    if target.most_recent {
        tracing::info!(steam_id64, "account is already active, skipping switch");
        return Ok(SwitchOutcome::AlreadyActive);
    }
    let target_label = target.display_name().to_string();
    let target_account = target.account_name.clone();

    let games = running_steam_games();
    if !games.is_empty() {
        tracing::info!(?games, "steam games are running, asking for confirmation");
        if !confirm_games(&games) {
            tracing::info!("switch cancelled by user (games running)");
            return Ok(SwitchOutcome::CancelledByUser);
        }
    }

    tracing::info!(%target_label, "closing Steam before switching");
    close_steam(steam)?;

    if !vdf::set_active(&mut users, steam_id64) {
        return Err(SwitchError::UnknownAccount(steam_id64.to_string()));
    }
    vdf::write_users(&steam.loginusers_vdf(), &users)?;
    write_autologin_registry(&target_account)?;
    launch_steam(steam)?;

    tracing::info!(%target_label, "switch complete, Steam relaunched");
    Ok(SwitchOutcome::Switched)
}

/// Names of running processes whose executable lives under a Steam library's
/// `steamapps/common` directory. Checking the process path directly avoids
/// needing the full library parse (that arrives in Phase 3).
pub fn running_steam_games() -> Vec<String> {
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::All, true);

    let mut games: Vec<String> = sys
        .processes()
        .values()
        .filter_map(|process| process.exe())
        .filter(|exe| is_under_steamapps_common(exe))
        .map(|exe| {
            exe.file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    games.sort();
    games.dedup();
    games
}

/// True when the path contains a `...\steamapps\common\...` component pair.
fn is_under_steamapps_common(path: &Path) -> bool {
    let mut components = path.components().map(|c| c.as_os_str().to_string_lossy().to_lowercase());
    // Case-insensitive scan for the adjacent pair ("steamapps", "common").
    let mut prev = String::new();
    for component in components.by_ref() {
        if prev == "steamapps" && component == "common" {
            return true;
        }
        prev = component;
    }
    false
}

/// Every running PID that is the Steam client itself.
fn find_steam_pids(sys: &mut System) -> Vec<Pid> {
    sys.refresh_processes(ProcessesToUpdate::All, true);
    sys.processes()
        .iter()
        .filter(|(_, process)| {
            let name = process.name().to_string_lossy();
            name.eq_ignore_ascii_case("steam.exe") || name.eq_ignore_ascii_case("steam")
        })
        .map(|(pid, _)| *pid)
        .collect()
}

/// Quit Steam if it is running: ask it to exit first (so cloud saves sync),
/// then escalate to a graceful `taskkill`, then force-kill as a last resort.
pub fn close_steam(steam: &SteamInstall) -> Result<(), SwitchError> {
    let mut sys = System::new_all();
    let mut pids = find_steam_pids(&mut sys);
    if pids.is_empty() {
        tracing::debug!("steam is not running");
        return Ok(());
    }
    tracing::info!(?pids, "steam is running, requesting graceful exit");

    // Stage 1: the client's own shutdown flag — the only path that reliably
    // finishes cloud-save sync before exit.
    let shutdown = std::process::Command::new(steam.exe()).arg("-shutdown").spawn();
    if let Err(err) = shutdown {
        tracing::warn!(%err, "failed to invoke `steam -shutdown`, falling back to taskkill");
    }
    if wait_until_gone(&mut sys, &mut pids, GRACEFUL_TIMEOUT) {
        return Ok(());
    }

    // Stage 2: `taskkill` without /F posts WM_CLOSE to Steam's windows.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        for pid in &pids {
            let _ = std::process::Command::new("taskkill")
                .args(["/PID", &pid.as_u32().to_string()])
                .creation_flags(0x0800_0000) // CREATE_NO_WINDOW, keeps the tray app console-free
                .spawn();
        }
    }
    if wait_until_gone(&mut sys, &mut pids, GRACEFUL_TIMEOUT) {
        return Ok(());
    }

    // Stage 3: force kill. Steam recovers from this (its lockfile handling is
    // defensive), but we only get here when every polite path failed.
    tracing::warn!("steam did not exit gracefully, force killing");
    for pid in &pids {
        if let Some(process) = sys.process(*pid) {
            process.kill();
        }
    }
    if !wait_until_gone(&mut sys, &mut pids, FORCE_KILL_WAIT) {
        tracing::error!("steam processes still alive after force kill");
    }
    Ok(())
}

/// Refresh just `pids` until none are alive or `timeout` elapses. On timeout
/// the slice is shrunk to the PIDs that are still alive.
fn wait_until_gone(sys: &mut System, pids: &mut Vec<Pid>, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        sys.refresh_processes(ProcessesToUpdate::Some(pids.as_slice()), false);
        pids.retain(|pid| sys.process(*pid).is_some());
        if pids.is_empty() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(WAIT_TICK);
    }
}

/// Write the registry keys that make Steam silently auto-login `account_name`
/// on next start (HKCU — no elevation needed).
#[cfg(windows)]
fn write_autologin_registry(account_name: &str) -> Result<(), SwitchError> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_SET_VALUE};
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu.open_subkey_with_flags("Software\\Valve\\Steam", KEY_SET_VALUE)?;
    key.set_value("AutoLoginUser", &account_name)?;
    key.set_value("RememberPassword", &1u32)?;
    tracing::info!(%account_name, "registry AutoLoginUser written");
    Ok(())
}

/// Non-Windows builds don't have an equivalent implemented yet (best-effort
/// platforms only switch the vdf for now).
#[cfg(not(windows))]
fn write_autologin_registry(_account_name: &str) -> Result<(), SwitchError> {
    tracing::warn!("registry autologin is not implemented on this platform");
    Ok(())
}

/// Start Steam again with the launch arguments from [`STEAM_LAUNCH_ARGS`].
fn launch_steam(steam: &SteamInstall) -> Result<(), SwitchError> {
    let exe = steam.exe();
    if !exe.is_file() {
        // Executable vanished between discovery and here — still a hard error.
        return Err(SwitchError::Io(std::io::Error::new(
            ErrorKind::NotFound,
            format!("steam executable not found at {}", exe.display()),
        )));
    }
    let mut command = std::process::Command::new(&exe);
    command.args(STEAM_LAUNCH_ARGS);
    tracing::info!(exe = %exe.display(), "launching Steam");
    command.spawn()?;
    Ok(())
}
