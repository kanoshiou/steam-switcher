# Steam Switcher

[English](README.md) | 简体中文 | [繁體中文](README.zh-TW.md) | [日本語](README.ja.md) | [한국어](README.ko.md)

一款 Windows 平台的 Steam 账号快速切换器,交互灵感来自 Nintendo Switch 的多用户选择界面。所有本机记住的 Steam 账号直接列在托盘菜单里——点一下,Steam 自动退出、切换账号、重新启动并已登录目标账号。无需重新输入密码,也不会停在登录界面。

<p align="center">
  <img src="assets/icon.ico" width="96" alt="steam-switcher 图标" />
</p>

## 功能特性

- **一键切换账号** —— 退出 Steam → 修改本地登录配置 → 以目标账号静默重启登录。
- **托盘优先** —— 右键托盘图标即可看到本机所有 Steam 账号,即刻切换。
- **设置窗口**(gpui-kit)—— 账号管理、已安装游戏列表、主题/语言选项、Steam 路径与启动参数。
- **指定账号启动游戏** —— 左键点击游戏:以当前账号直接启动;右键点击:先选账号(切换后自动启动游戏)。
- **默认安全** —— 有游戏正在运行时,关闭 Steam 前会弹出确认对话框;每次写入前都会备份 `loginusers.vdf`(`loginusers.vdf.bak-YYYYMMDD-HHMMSS`,保留最近 3 份)。
- **多语言界面** —— 简体中文、繁體中文、English、日本語、한국어。
- **体验细节** —— 深色/浅色/跟随系统主题、开机自启、关闭最小化到托盘、单实例运行、所有操作均有气泡通知。

## 切换原理

Steam 的静默自动登录由两处共同决定,每次切换都会同时写入:

1. **退出 Steam**(先尝试正常关闭;有游戏运行时会先请求确认)。
2. **修改 `config/loginusers.vdf`** —— 目标账号设为 `MostRecent=1` / `RememberPassword=1`,其余账号设为 `MostRecent=0`。它决定登录界面预选谁。
3. **写入注册表** `HKCU\Software\Valve\Steam` —— `AutoLoginUser`(Steam 静默登录的账号)和 `RememberPassword=1`。
4. **重新启动 Steam**(可附带自定义启动参数)。

## 下载

前往 [Releases](../../releases) 下载 zip 压缩包。每个标签会构建两个 Windows 可执行文件(`x86_64-pc-windows-msvc`):

| 变体 | 说明 |
|---|---|
| `gui` | 托盘图标 + 完整设置窗口(默认构建) |
| `tray` | 仅托盘版,体积更小,无 GUI 依赖 |

每个压缩包均附带 `.sha256` 校验文件。

> **平台支持:** 优先支持 Windows 10/11。macOS/Linux 暂未打包,尚在计划中。

## 从源码构建

需要 Rust 1.95+(stable)。

```sh
# GUI 构建(托盘 + 设置窗口)
cargo build --release

# 仅托盘构建,不依赖 gpui
cargo build --release --no-default-features
```

## 使用说明

- **自动发现账号** —— 账号列表来自 Steam 自己的 `loginusers.vdf`;只要勾选"记住密码"登录过一次,该账号就会出现。
- **移除账号** —— 在设置窗口中移除账号只会删除本地登录记录,不影响游戏所有权和云存档。
- **配置文件** 位于 `%APPDATA%\steam-switcher\config.json`。
- **小技巧:** 在"设置 → 启动参数"中添加 `-silent`(如 `-silent -no-browser`),切换更快且不弹窗口。

## 许可证

[MIT](LICENSE)
