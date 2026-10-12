//! Logging setup: rolling daily file + stdout, both gated by `RUST_LOG`
//! (default `info`, with `steam_switcher` at `debug`).

use std::path::PathBuf;

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

/// Initialize the global tracing subscriber.
pub fn init() -> std::io::Result<()> {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,steam_switcher=debug"));

    let log_dir = log_dir();
    std::fs::create_dir_all(&log_dir)?;
    let file_writer = tracing_appender::rolling::daily(&log_dir, "steam-switcher.log");

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_filter(filter.clone()))
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(file_writer)
                .with_filter(filter),
        )
        .init();
    Ok(())
}

/// Where log files live: `%APPDATA%\steam-switcher\logs` on Windows,
/// `./logs` elsewhere.
pub fn log_dir() -> PathBuf {
    #[cfg(windows)]
    if let Some(appdata) = std::env::var_os("APPDATA") {
        return PathBuf::from(appdata).join("steam-switcher").join("logs");
    }
    PathBuf::from("logs")
}
