# Steam Switcher

[English](README.md) | [简体中文](README.zh-CN.md) | 繁體中文 | [日本語](README.ja.md) | [한국어](README.ko.md)

一款 Windows 平台的 Steam 帳號快速切換器,互動靈感來自 Nintendo Switch 的多使用者選擇介面。所有本機記住的 Steam 帳號直接列在系統匣選單裡——點一下,Steam 自動結束、切換帳號、重新啟動並已登入目標帳號。無需重新輸入密碼,也不會停在登入畫面。

<p align="center">
  <img src="assets/icon.ico" width="96" alt="steam-switcher 圖示" />
</p>

## 功能特性

- **一鍵切換帳號** —— 結束 Steam → 修改本機登入設定 → 以目標帳號靜默重新啟動登入。
- **系統匣優先** —— 右鍵系統匣圖示即可看到本機所有 Steam 帳號,即刻切換。
- **設定視窗**(gpui-kit)—— 帳號管理、已安裝遊戲清單、主題/語言選項、Steam 路徑與啟動參數。
- **指定帳號啟動遊戲** —— 左鍵點擊遊戲:以目前帳號直接啟動;右鍵點擊:先選帳號(切換後自動啟動遊戲)。
- **預設安全** —— 有遊戲正在執行時,結束 Steam 前會彈出確認對話方塊;每次寫入前都會備份 `loginusers.vdf`(`loginusers.vdf.bak-YYYYMMDD-HHMMSS`,保留最近 3 份)。
- **多語言介面** —— 繁體中文、简体中文、English、日本語、한국어。
- **體驗細節** —— 深色/淺色/跟隨系統主題、開機自動啟動、關閉時最小化到系統匣、單一執行個體、所有操作皆有通知提示。

## 切換原理

Steam 的靜默自動登入由兩處共同決定,每次切換都會同時寫入:

1. **結束 Steam**(先嘗試正常關閉;有遊戲執行時會先請求確認)。
2. **修改 `config/loginusers.vdf`** —— 目標帳號設為 `MostRecent=1` / `RememberPassword=1`,其餘帳號設為 `MostRecent=0`。它決定登入畫面預選誰。
3. **寫入登錄檔** `HKCU\Software\Valve\Steam` —— `AutoLoginUser`(Steam 靜默登入的帳號)和 `RememberPassword=1`。
4. **重新啟動 Steam**(可附加自訂啟動參數)。

## 下載

前往 [Releases](../../releases) 下載 zip 壓縮檔。每個標籤會建置兩個 Windows 執行檔(`x86_64-pc-windows-msvc`):

| 變體 | 說明 |
|---|---|
| `gui` | 系統匣圖示 + 完整設定視窗(預設建置) |
| `tray` | 僅系統匣版本,體積更小,無 GUI 相依 |

每個壓縮檔均附帶 `.sha256` 校驗檔。

> **平台支援:** 優先支援 Windows 10/11。macOS/Linux 暫未封裝,尚在計畫中。

## 從原始碼建置

需要 Rust 1.95+(stable)。

```sh
# GUI 建置(系統匣 + 設定視窗)
cargo build --release

# 僅系統匣建置,不依賴 gpui
cargo build --release --no-default-features
```

## 使用說明

- **自動探索帳號** —— 帳號清單來自 Steam 自己的 `loginusers.vdf`;只要勾選「記住密碼」登入過一次,該帳號就會出現。
- **移除帳號** —— 在設定視窗中移除帳號只會刪除本機登入記錄,不影響遊戲所有權與雲端存檔。
- **設定檔** 位於 `%APPDATA%\steam-switcher\config.json`。
- **小技巧:** 在「設定 → 啟動參數」中加入 `-silent`(如 `-silent -no-browser`),切換更快且不彈視窗。

## 授權條款

[MIT](LICENSE)
