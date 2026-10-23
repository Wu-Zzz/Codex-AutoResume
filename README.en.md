# Codex AutoResume

[简体中文](README.md) | English

A lightweight macOS menu bar / Windows system tray utility that waits for Codex quota to recover, then continues tasks interrupted by quota exhaustion.

**Currently in Alpha. Continuing in the original Codex Desktop chat on Mac is experimental: launching Codex from its normal icon, connecting to the original chat, and enabling the continuation switch have been verified. Automatic recovery after real quota exhaustion still needs to be tested.**

![Application interface with example data](docs/images/app-preview.png)

## Download

This page covers [v0.2.0-alpha.1](https://github.com/Wu-Zzz/Codex-AutoResume/releases/tag/v0.2.0-alpha.1). Installers are built in the cloud with GitHub Actions.

| Platform | Download | Continue in the original Desktop chat |
| --- | --- | --- |
| Mac · Apple Silicon | [ARM64 DMG](https://github.com/Wu-Zzz/Codex-AutoResume/releases/download/v0.2.0-alpha.1/Codex-AutoResume-v0.2.0-alpha.1-macos-arm64.dmg) | Experimental components included; connection verified on one local Mac |
| Mac · Intel | [x64 DMG](https://github.com/Wu-Zzz/Codex-AutoResume/releases/download/v0.2.0-alpha.1/Codex-AutoResume-v0.2.0-alpha.1-macos-x64.dmg) | Experimental components included; not tested on a physical Intel Mac |
| Windows · x64 | [EXE installer](https://github.com/Wu-Zzz/Codex-AutoResume/releases/download/v0.2.0-alpha.1/Codex-AutoResume-v0.2.0-alpha.1-windows-x64.exe) | Not currently supported; new terminal tasks are available |

On Mac, open the DMG and drag the app into Applications. On Windows, run the EXE installer. A [SHA-256 checksum file](https://github.com/Wu-Zzz/Codex-AutoResume/releases/download/v0.2.0-alpha.1/SHA256SUMS.txt) is included with the release. Mac builds use ad-hoc signing only; they are not signed with an Apple Developer certificate or notarized. Windows builds are not code-signed.

## Use with an existing chat on Mac

Requires **Codex Desktop build `20770` and the default zsh environment**, with Codex installed in `/Applications`. Other builds are blocked from enabling this feature. It uses a modified backend executable included in the package, without modifying the official app, account credentials, or chat history. **Codex voice is currently unavailable while this integration is enabled.**

The interface is currently in Chinese; the steps below include the labels shown in the app.

1. Open AutoResume, click the connection status at the top, and select **自动连接 Codex → 开启自动连接** (Connect to Codex automatically → Enable automatic connection).
2. Fully quit Codex with **⌘Q**, then reopen it from its usual icon. Future launches work normally; no special launcher is needed.
3. Start a task in the original Codex chat, then enable **自动续跑** (Automatic continuation) for that chat in AutoResume's task list.
4. Keep AutoResume, Codex, and the computer running. Closing the AutoResume window keeps it running in the tray. **The app does not prevent the computer from sleeping.**

Each chat requires separate opt-in. Restarting, disconnecting, new user input, or configuration changes revoke permission to continue automatically; you must enable it again. Interruptions from before a restart are not replayed. To restore the official backend executable, turn off **自动连接 Codex** (Connect to Codex automatically) and restart Codex.

## Create a new terminal task (Mac / Windows)

1. Install the official [Codex CLI](https://github.com/openai/codex#quickstart). The version examined for this release is `0.145.0`.
2. Select **添加任务 → 新建终端任务** (Add task → New terminal task), choose a project, check **额度恢复后自动续跑** (Continue automatically when quota recovers), and open the Codex terminal.
3. Complete the official ChatGPT sign-in flow in the terminal and enter your task. This is a separate environment and requires its own initial sign-in. Existing credentials are not copied. API key mode is not currently supported.
4. Keep the terminal, AutoResume, and the computer running. Existing tasks are not automatically recovered after the app restarts or the terminal disconnects.

You can view the status of other existing Desktop / CLI sessions through **添加任务 → 历史监测** (Add task → Monitor existing sessions). The monitoring switch does not authorize automatic execution.

## Continuation rules and limitations

- Only an explicit `usageLimitExceeded` event marking a failed turn qualifies for automatic continuation. Normal completion, a user stop, pending approval, or a request for user input does not trigger a continuation prompt.
- Uses actual quota and reset times. Unknown values are shown as unknown. It does not wait a fixed five hours, and quota is checked again when the timer expires.
- Stops when permissions, session state, or the submission result are uncertain. It does not relax permissions or blindly resend requests.
- Pausing all tasks, turning off a switch, or quitting AutoResume only prevents future automatic submissions; it does not stop a task that is already running.
- Removing a task from the list does not delete its original chat or project files.
- Sleep prevention, Git branch checks, system notifications / webhooks, automatic CLI installation, and launch at login are not yet available.

## Validation status

Local checks passed: 190 Rust unit tests, 6 simulated connection tests, 15 frontend tests, and static checks. On a Mac with Apple Silicon, one ordinary terminal task was verified, along with connecting to the original Desktop chat after a normal icon launch and enabling the continuation switch.

The cloud builds and checks for all three installers passed, and the SHA-256 checksums of the downloads were verified. **Recovery after real quota exhaustion and interactive use on physical Intel Mac / Windows machines remain unverified.** Simulated tests and cloud builds do not replace these checks. See the [validation record](docs/VALIDATION.md) and [experimental connection documentation](docs/desktop-bridge.md).

## Development

Tauri 2 + Vue 3 + TypeScript + Rust + SQLite. Requires Node.js 22.12+, stable Rust, and the [Tauri system dependencies](https://v2.tauri.app/start/prerequisites/).

```sh
npm ci
npm run tauri dev
```

Standard build: `npm run tauri build`. See the [development documentation](docs/desktop-bridge.md) for building the components used to continue in the original chat. To preview the interface with example data, run `npm run dev` and append `?demo=1` to the URL.

[Development rules](AGENTS.md) · [Implementation plan](PLAN.md) · [MIT License](LICENSE). For the license and source of the bundled modified Codex executable, see the [upstream notes](integrations/codex/README.md). This project is not affiliated with OpenAI.
