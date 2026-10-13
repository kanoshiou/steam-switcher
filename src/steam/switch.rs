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

/// Progress milestones of a switch, reported to the UI overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SwitchStage {
    /// Resolving the target and closing Steam.
    Prepare = 1,
    /// Waiting for the Steam client to exit.
    ClosingSteam = 2,
    /// Patching `loginusers.vdf` + registry.
    WritingConfig = 3,
    /// Starting Steam again.
    LaunchingSteam = 4,
    /// Starting the requested game (switch-then-launch only).
    LaunchingGame = 5,
}

impl SwitchStage {
    /// The stage as the small number the UI-side atomic stores (0 = idle).
    pub fn as_u8(self) -> u8 {
        self as u8
    }

    /// Decode a value previously stored by [`Self::as_u8`].
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Prepare),
            2 => Some(Self::ClosingSteam),
            3 => Some(Self::WritingConfig),
            4 => Some(Self::LaunchingSteam),
            5 => Some(Self::LaunchingGame),
            _ => None,
        }
    }
}

/// Switch the machine to the account identified by `steam_id64`.
///
/// `confirm_games` is called with the list of running Steam games whenever any
/// are detected; returning `false` aborts the switch. The dialog must not be
/// skippable (engineering conventions) — the caller owns its implementation.
/// `progress` receives each stage transition; `launch_args` are passed to the
/// relaunched `steam.exe`.
pub fn switch_to_account(
    steam: &SteamInstall,
    steam_id64: &str,
    confirm_games: &dyn Fn(&[String]) -> bool,
    progress: &dyn Fn(SwitchStage),
    launch_args: &[String],
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
    progress(SwitchStage::Prepare);

    let games = running_steam_games();
    if !games.is_empty() {
        tracing::info!(?games, "steam games are running, asking for confirmation");
        if !confirm_games(&games) {
            tracing::info!("switch cancelled by user (games running)");
            return Ok(SwitchOutcome::CancelledByUser);
        }
    }

    tracing::info!(%target_label, "closing Steam before switching");
    progress(SwitchStage::ClosingSteam);
    close_steam(steam)?;

    progress(SwitchStage::WritingConfig);
    if !vdf::set_active(&mut users, steam_id64) {
        return Err(SwitchError::UnknownAccount(steam_id64.to_string()));
    }
    vdf::write_users(&steam.loginusers_vdf(), &users)?;
    write_autologin_registry(&target_account)?;

    progress(SwitchStage::LaunchingSteam);
    launch_steam(steam, launch_args)?;

    tracing::info!(%target_label, "switch complete, Steam relaunched");
    Ok(SwitchOutcome::Switched)
}

/// Quit Steam gracefully and start it again (sidebar footer button).
pub fn restart_steam(steam: &SteamInstall, launch_args: &[String]) -> Result<(), SwitchError> {
    close_steam(steam)?;
    launch_steam(steam, launch_args)
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

/// True while the Steam client itself is running (any `steam.exe` process).
/// One full process enumeration per call — callers should poll, not per-frame.
pub fn steam_running() -> bool {
    let mut sys = System::new();
    !find_steam_pids(&mut sys).is_empty()
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

/// Read the account name Steam silently auto-logs-in on startup (registry
/// `HKCU\Software\Valve\Steam\AutoLoginUser`). Steam maintains this value on
/// every in-client login, so it doubles as a "who is active" signal.
/// `None` when the value is unset or on non-Windows platforms.
#[cfg(windows)]
pub fn autologin_user() -> Option<String> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_QUERY_VALUE};
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu
        .open_subkey_with_flags("Software\\Valve\\Steam", KEY_QUERY_VALUE)
        .ok()?;
    key.get_value("AutoLoginUser").ok()
}

/// Non-Windows builds have no registry equivalent yet.
#[cfg(not(windows))]
pub fn autologin_user() -> Option<String> {
    None
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

/// Clear the registry `AutoLoginUser` value (used when its account is removed
/// locally, so Steam does not try to silently auto-login a deleted account).
#[cfg(windows)]
pub fn clear_autologin_registry() -> Result<(), SwitchError> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_SET_VALUE};
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu.open_subkey_with_flags("Software\\Valve\\Steam", KEY_SET_VALUE)?;
    match key.delete_value("AutoLoginUser") {
        Ok(()) => tracing::info!("registry AutoLoginUser cleared"),
        // Already absent is exactly the state we want.
        Err(ref err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err.into()),
    }
    Ok(())
}

/// Non-Windows builds have no registry to clear.
#[cfg(not(windows))]
pub fn clear_autologin_registry() -> Result<(), SwitchError> {
    Ok(())
}

/// Start Steam again with the given launch arguments (an empty slice launches
/// with no arguments; the `-silent` default lives in `Config`).
fn launch_steam(steam: &SteamInstall, args: &[String]) -> Result<(), SwitchError> {
    let exe = steam.exe();
    if !exe.is_file() {
        // Executable vanished between discovery and here — still a hard error.
        return Err(SwitchError::Io(std::io::Error::new(
            ErrorKind::NotFound,
            format!("steam executable not found at {}", exe.display()),
        )));
    }
    let mut command = std::process::Command::new(&exe);
    command.args(args);
    tracing::info!(exe = %exe.display(), "launching Steam");
    command.spawn()?;
    Ok(())
}
