# Steam Switcher

English | [简体中文](README.zh-CN.md) | [繁體中文](README.zh-TW.md) | [日本語](README.ja.md) | [한국어](README.ko.md)

A Nintendo-Switch-style Steam account switcher for Windows. Your Steam accounts are listed right in the tray menu — pick one, and Steam quits, switches to that account, and relaunches already logged in. No password re-entry, no login screen.

<p align="center">
  <img src="assets/icon.ico" width="96" alt="steam-switcher icon" />
</p>

## Features

- **One-click account switching** — quit Steam → patch the local login config → relaunch silently logged in as the chosen account.
- **Tray-first workflow** — right-click the tray icon to see every account remembered on this machine and switch instantly.
- **Settings window** (gpui-kit) — account management, an installed-games list, theme/language options, and Steam path & launch arguments.
- **Launch games with any account** — left-click a game to launch it with the current account; right-click to pick an account first (Steam switches, then the game starts).
- **Safe by default** — running games trigger a confirmation dialog before Steam is closed; `loginusers.vdf` is backed up (`loginusers.vdf.bak-YYYYMMDD-HHMMSS`, latest 3 kept) before every write.
- **Localized UI** — English, 简体中文, 繁體中文, 日本語, 한국어.
- **Quality-of-life** — dark/light/system theme, launch on startup, minimize to tray on close, single instance, toasts for every action.

## How switching works

Steam's silent auto-login is controlled in two places, and both are written on every switch:

1. **Quit Steam** (graceful close first; a confirmation is shown when games are running).
2. **Patch `config/loginusers.vdf`** — target account gets `MostRecent=1` / `RememberPassword=1`, everyone else `MostRecent=0`. This decides who is pre-selected on the login screen.
3. **Write the registry** `HKCU\Software\Valve\Steam` — `AutoLoginUser` (who Steam silently logs in) and `RememberPassword=1`.
4. **Relaunch Steam** with your optional extra launch arguments.

## Download

Grab a zip from [Releases](../../releases). Each tag builds two Windows executables (`x86_64-pc-windows-msvc`):

| Variant | Contents |
|---|---|
| `gui` | Tray icon + full settings window (default build) |
| `tray` | Tray-only, smaller, no GUI dependency |

Every archive ships with a `.sha256` checksum.

> **Platform support:** Windows 10/11 first. macOS/Linux builds are planned but not packaged yet.

## Build from source

Requires Rust 1.95+ (stable).

```sh
# GUI build (tray + settings window)
cargo build --release

# Tray-only build, no gpui dependency
cargo build --release --no-default-features
```

## Usage notes

- **Accounts are discovered automatically** from Steam's own `loginusers.vdf`; log in once with "remember password" checked and the account appears.
- **Removing an account** in the settings window deletes only the local login entry — game ownership and cloud data are untouched.
- **Config** lives at `%APPDATA%\steam-switcher\config.json`.
- **Tip:** add `-silent` (e.g. `-silent -no-browser`) in Settings → launch arguments for a fast, popup-free switch.

## License

[MIT](LICENSE)
