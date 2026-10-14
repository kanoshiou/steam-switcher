# Steam Switcher

[English](README.md) | [简体中文](README.zh-CN.md) | [繁體中文](README.zh-TW.md) | [日本語](README.ja.md) | 한국어

Windows용 Steam 계정 빠른 전환기입니다. Nintendo Switch의 멀티 유저 선택 화면에서 영감을 받은 UX로, 이 PC에 저장된 Steam 계정이 트레이 메뉴에 바로 표시됩니다. 계정을 클릭하면 Steam이 종료되고 → 계정이 전환되며 → 대상 계정으로 로그인된 상태로 다시 실행됩니다. 비밀번호를 다시 입력할 필요도, 로그인 화면에서 멈출 일도 없습니다.

<p align="center">
  <img src="assets/icon.ico" width="96" alt="steam-switcher 아이콘" />
</p>

## 주요 기능

- **원클릭 계정 전환** —— Steam 종료 → 로컬 로그인 설정 수정 → 대상 계정으로 자동 로그인 후 재실행.
- **트레이 우선 워크플로** —— 트레이 아이콘을 우클릭하면 이 PC의 모든 Steam 계정이 표시되어 즉시 전환할 수 있습니다.
- **설정 창**(gpui-kit)—— 계정 관리, 설치된 게임 목록, 테마/언어 옵션, Steam 경로 및 실행 인자.
- **계정을 지정해 게임 실행** —— 게임을 좌클릭: 현재 계정으로 바로 실행. 우클릭: 계정을 먼저 선택(전환 후 게임이 시작됨).
- **기본적으로 안전** —— 게임 실행 중에는 Steam을 종료하기 전에 확인 대화상자를 표시합니다. 쓰기 전마다 `loginusers.vdf`를 백업합니다(`loginusers.vdf.bak-YYYYMMDD-HHMMSS`, 최근 3개 유지).
- **다국어 UI** —— 한국어, English, 简体中文, 繁體中文, 日本語.
- **편의 기능** —— 다크/라이트/시스템 테마, 시작 프로그램 등록, 닫을 때 트레이로 최소화, 단일 인스턴스 실행, 모든 작업에 토스트 알림.

## 전환 원리

Steam의 자동 로그인은 두 곳에 의해 결정되며, 전환 시 항상 모두 기록합니다.

1. **Steam 종료**(먼저 정상 종료를 시도. 게임이 실행 중이면 먼저 확인을 요청).
2. **`config/loginusers.vdf` 수정** —— 대상 계정은 `MostRecent=1` / `RememberPassword=1`, 나머지 계정은 `MostRecent=0`으로 설정. 로그인 화면에서 미리 선택될 계정을 결정합니다.
3. **레지스트리 기록** `HKCU\Software\Valve\Steam` —— `AutoLoginUser`(Steam이 자동으로 로그인할 계정)와 `RememberPassword=1`.
4. **Steam 재실행**(선택적으로 추가 실행 인자를 붙일 수 있음).

## 다운로드

[Releases](../../releases)에서 zip을 내려받을 수 있습니다. 태그마다 두 가지 Windows 실행 파일(`x86_64-pc-windows-msvc`)이 빌드됩니다.

| 변형 | 내용 |
|---|---|
| `gui` | 트레이 아이콘 + 전체 설정 창(기본 빌드) |
| `tray` | 트레이 전용. 더 가볍고 GUI 의존성 없음 |

모든 zip에는 `.sha256` 체크섬 파일이 함께 제공됩니다.

> **플랫폼 지원:** Windows 10/11 우선. macOS/Linux는 계획 중이며 아직 패키징되지 않았습니다.

## 소스에서 빌드

Rust 1.95+(stable)가 필요합니다.

```sh
# GUI 빌드(트레이 + 설정 창)
cargo build --release

# 트레이 전용 빌드(gpui 의존성 없음)
cargo build --release --no-default-features
```

## 사용 팁

- **계정 자동 발견** —— 계정 목록은 Steam 자체의 `loginusers.vdf`에서 가져옵니다. "비밀번호 기억"에 체크한 채로 한 번만 로그인하면 해당 계정이 표시됩니다.
- **계정 제거** —— 설정 창에서 제거하면 로컬 로그인 정보만 삭제됩니다. 게임 소유권과 클라우드 데이터에는 영향이 없습니다.
- **설정 파일** 위치: `%APPDATA%\steam-switcher\config.json`.
- **팁:** "설정 → 실행 인자"에 `-silent`(예: `-silent -no-browser`)를 추가하면 팝업 없이 빠르게 전환됩니다.

## 라이선스

[MIT](LICENSE)
