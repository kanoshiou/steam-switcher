//! Single-instance guard via a Windows named mutex.
//!
//! Kept deliberately tiny: acquire on startup, hold for the process lifetime,
//! release on drop.

/// Handle to the held mutex; dropping it releases the instance lock.
#[derive(Debug)]
pub struct SingleInstance {
    #[cfg(windows)]
    handle: windows::Win32::Foundation::HANDLE,
}

/// Why acquiring the single-instance lock can fail.
#[derive(Debug, thiserror::Error)]
pub enum SingletonError {
    /// Another instance of the app is already running.
    #[error("another instance is already running")]
    AlreadyRunning,
    /// Creating the mutex failed at the OS level.
    #[error("failed to create single-instance mutex: {0}")]
    Os(#[from] std::io::Error),
}

/// Per-session mutex name: the app is per-user, so concurrent sessions of
/// different users may each run their own copy.
#[cfg(windows)]
const MUTEX_NAME: &str = "Local\\steam-switcher-single-instance";

/// Acquire the single-instance lock for the remainder of the process lifetime.
///
/// # Errors
/// [`SingletonError::AlreadyRunning`] when another instance holds the lock.
pub fn acquire() -> Result<SingleInstance, SingletonError> {
    #[cfg(windows)]
    {
        use windows::core::HSTRING;
        use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
        use windows::Win32::System::Threading::CreateMutexW;

        // SAFETY: CreateMutexW with no security attributes and a valid name;
        // the returned handle is either ours or (already-exists) a duplicate we
        // must still close, which Drop does.
        let handle = unsafe { CreateMutexW(None, false, &HSTRING::from(MUTEX_NAME)) }
            .map_err(std::io::Error::from)?;

        // SAFETY: GetLastError is always safe to call; right after CreateMutexW
        // it reports whether the mutex already existed.
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            tracing::warn!("single-instance mutex already held, exiting");
            return Err(SingletonError::AlreadyRunning);
        }
        Ok(SingleInstance { handle })
    }

    #[cfg(not(windows))]
    {
        // Best-effort platforms: no guard for now.
        Ok(SingleInstance {})
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            use windows::Win32::Foundation::CloseHandle;
            // SAFETY: `handle` is a valid mutex handle we own and have not closed yet.
            unsafe {
                let _ = CloseHandle(self.handle);
            }
        }
    }
}
