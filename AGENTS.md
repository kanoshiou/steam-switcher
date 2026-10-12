# AGENTS.md

This file helps AI coding assistants (and new human contributors) get productive in this repository quickly.

## Project Overview

**steam-switcher** — a Windows-first Steam account quick switcher.

The core experience mirrors the Nintendo Switch multi-user picker: the tray right-click
menu lists Steam accounts remembered on this machine. Clicking an account quits Steam,
switches to the selected account, and relaunches Steam (Phase 1). Later phases add
"pick an account to launch a specific game" (Phase 2 UI, Phase 3 game library integration).

- Language: Rust (edition 2024, stable)
- UI framework: gpui-kit (https://gpui-kit.com — the component-library distribution of Zed's GPUI; supports Windows/macOS/Linux)
- Target platforms: Windows 10/11 first; macOS/Linux best-effort (account switching core only)

## Language Policy

- **All source code comments, doc comments, and identifiers must be written in English.**
- Commit messages: English, imperative mood.
- Planning documents under `docs/` may remain in Chinese for now.

## Tech Stack

| Purpose | Choice |
|---|---|
| UI framework | gpui-kit 0.58 (Phase 2 settings window; not used in Phase 1) |
| Tray | `tray-icon` crate |
| Global hotkey | `global-hotkey` crate |
| Single instance | Windows named mutex (`windows` crate FFI, ~20 lines) |
| Process management | `sysinfo` (find/kill steam.exe, detect running games) |
| Steam discovery | Registry `HKCU\Software\Valve\Steam` → `SteamPath` (`winreg`), fallback to well-known install paths |
| vdf parsing | `keyvalues-parser` (fallback: hand-written parser, see plan.md D1) |
| Registry writes | `winreg` |
| Config persistence | `serde` + `serde_json` → `%APPDATA%\steam-switcher\config.json` |
| Logging | `tracing` + `tracing-subscriber` (file + stdout) |
| Error handling | `anyhow` (application layer) + `thiserror` (`steam` module library layer) |

## Directory Layout (planned)

```
steam-switcher/
├── AGENTS.md            # this file
├── Cargo.toml
├── docs/
│   ├── plan.md               # phased implementation plan (read first)
│   ├── research-vdf.md       # loginusers.vdf format & switching recipe research
│   └── research-game-list.md # local game list & launching research (Steam Web API question)
└── src/
    ├── main.rs        # entry: single instance → tray+hotkey event loop → (P2) gpui window
    ├── tray.rs        # tray icon + context menu building / event forwarding
    ├── config.rs      # app config (game mappings, hotkey, autostart)
    └── steam/
        ├── mod.rs     # module root
        ├── discover.rs # Steam install directory discovery
        ├── vdf.rs     # loginusers.vdf parse / modify / write-back
        ├── users.rs   # SteamUser model and listing
        ├── switch.rs  # orchestration: kill process → patch vdf → write registry → launch
        ├── library.rs # libraryfolders.vdf → installed games (Phase 3)
        └── launch.rs  # steam:// protocol game launch (Phase 3)
```

## Switching Recipe (reference when implementing `steam/switch.rs`)

The complete passwordless switching recipe (verified against Watt Toolkit source code):

1. **Detect and quit Steam** (`sysinfo` finds `steam.exe` → graceful close first, force kill
   on timeout; if a game process is running, prompt the user for confirmation first to avoid
   cloud save loss)
2. **Patch `config/loginusers.vdf`**:
   - Target account block: `MostRecent=1`, `RememberPassword=1`
   - All other account blocks: `MostRecent=0`
3. **Write registry** `HKCU\Software\Valve\Steam`:
   - `AutoLoginUser` = target account name (`REG_SZ`)
   - `RememberPassword` = 1 (`REG_DWORD`)
4. **Launch Steam** (`steam.exe`, optional extra launch arguments)

Key insight: registry `AutoLoginUser` decides who Steam **silently auto-logs-in** on startup;
the vdf `MostRecent` only decides who is pre-selected on the login screen. **Both must be
written** for a fully seamless switch.

## Engineering Conventions

- No `unsafe` except where FFI is unavoidable (e.g. the Windows mutex); such code must carry
  a `// SAFETY:` comment explaining invariants.
- Error handling: `anyhow::Result` at the application layer; the `steam/` module exposes
  `thiserror`-based typed errors.
- Every public function gets a doc comment (`///`), in English.
- **Back up before writing vdf**: `loginusers.vdf.bak-YYYYMMDD-HHMMSS` (keep the latest 3).
- The game-process confirmation dialog before switching must not be skippable.
- Unit tests: `steam/vdf.rs` parse/modify round-trips must have tests (using a fixture vdf
  committed to the repo); other modules as needed.

## Open Questions

- **D1 vdf write-back fidelity**: `keyvalues-parser` drops comments and original formatting
  on write-back (SteamKit's KVSerializer does the same; Watt too). Acceptance criterion: as
  long as Steam parses the file and logs in normally, it's fine. If real-world testing shows
  problems, fall back to a hand-written format-preserving surgical patcher (byte offsets that
  only touch the `MostRecent` lines).
- **D2 tray/gpui event-loop integration**: Phase 1 is pure tray + `global-hotkey` running its
  own event loop, deliberately without gpui, to avoid this problem. Revisit in Phase 2 when
  the gpui-kit window lands (poll the tray/hotkey event receivers on a background executor
  and forward to the main thread). Preliminary design in plan.md Phase 2.
- **D3 silent launch flag**: whether Steam needs `-silent` to skip the login window is to be
  verified on a real machine (Watt's `StartSteamWithParameter` reads a user setting; the
  default arguments are unconfirmed).
