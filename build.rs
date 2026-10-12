// Embeds `assets/icon.ico` into the executable so Explorer, the taskbar and
// the exe properties show the app icon (the tray icon is drawn at runtime by
// `tray::render_rgba`, the same design the .ico is generated from).
//
// Regenerate the .ico after changing the design: `cargo run --example gen-icon`.

fn main() {
    // CARGO_CFG_WINDOWS is set when compiling for a Windows target.
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        let icon = "assets/icon.ico";
        if !std::path::Path::new(icon).is_file() {
            // Fresh checkout before the icon was ever generated.
            println!(
                "cargo:warning=assets/icon.ico not found; exe will use the default icon. \
                 Run `cargo run --example gen-icon` to generate it."
            );
            return;
        }
        winresource::WindowsResource::new()
            .set_icon(icon)
            .compile()
            .expect("failed to embed assets/icon.ico into the executable");
    }
}
