# Validation record

Initial development baseline: 2026-10-20, Asia/Taipei.

## What this alpha can claim

The live integration is an observation client. It can inspect available local histories and account quota information through the official Codex App Server. Automatic continuation remains blocked for externally owned chats until ownership, configuration preservation, and an atomic or serialized input path are proven. See [the protocol audit](protocol-audit.md).

Tests of the scheduler prove deterministic decisions against fixtures; they do not prove that a real Desktop chat has safely resumed. A successful build proves compilation, not installation, signing, or a real quota-recovery cycle.

## Live read-only probe

The local macOS probe returned the following results against installed `codex-cli 0.145.0`:

| Check | Observed result |
| --- | --- |
| Initialize handshake | Passed |
| Account status | Signed in through ChatGPT; no address or credential retained here |
| History discovery | First page returned 10 threads and indicated another page |
| Selected `thread/read` | Passed |
| Quota read | Passed |
| Quota buckets returned | 1 |
| Service reset timestamp available | Yes |

Only the presence and shape of results are recorded. Private thread names, conversation content, paths, email addresses, quota values, and credentials are excluded from this document. The probe did not resume a thread, start a turn, approve an action, or consume reset credits. Its result is a point-in-time connectivity check, not evidence that account limits or login state remain unchanged.

## Automated checks

Local macOS ARM64 results (2026-10-20):

| Check | Result |
| --- | --- |
| Vue / TypeScript check | Passed |
| Frontend boundary and cross-platform probe-discovery tests | 11 passed |
| Production frontend build | Passed |
| Rust scheduler, persistence and transport tests | 27 passed |
| Rust formatting | Passed |
| Clippy, all targets, warnings denied | Passed |
| npm dependency audit | 0 reported vulnerabilities at the time of the check |
| Tauri debug `.app` bundle | Built, 35.84 MiB, ARM64, development build |
| Native macOS launch and account connection | Passed; UI displayed connected state |
| Native existing-session picker | Visually verified populated with actual local histories; none selected or resumed |
| Close/reopen and explicit quit | Process stayed alive after close, window reopened; explicit Quit terminated the app |
| Redesigned browser UI interactions | Passed: toggle, pause/resume, details, search, hidden-selection guard, add default-off candidate, remove, connection/capability dialogs, Escape and focus restoration |
| Narrow browser layout | 390 px viewport, no horizontal overflow |

The redesigned utility window defaults to 760 × 580. Three sample tasks fit without clipping; the main view has one add button and one details entry per task. UI screenshots use isolated demo fixtures only. The rebuilt macOS application was launched and its connection dialog and empty task view were checked against the local account; no real tasks were added or enabled.

The probe-discovery tests simulate Windows filesystem layouts (PATH, APPDATA, explicit executable overrides and npm launchers) on either runner. They are not Windows installation or live-account tests.

The Rust transport tests use in-memory JSONL byte streams, including handshake ordering, response correlation, rejection of approval/credential requests, malformed responses, EOF, timeout, and the blocked write-method boundary. The scheduler's full continuation path is a fixture-based simulation only.

The native test did not add or enable any real task. App data contains an empty task list. Native tests do not yet establish tray-menu interaction, a forced simultaneous second process, Windows behavior, code signing, or notarization. The macOS app is a local development artifact, not a public release.

The repository's [CI workflow](../.github/workflows/ci.yml) defines independent `macos-latest` and `windows-latest` jobs:

1. Install locked Node dependencies and use stable Rust with `rustfmt` and `clippy`.
2. Run frontend type checks, tests, and build.
3. Run Rust formatting checks, scheduler/persistence tests, and Clippy with warnings treated as errors.
4. Compile the Tauri desktop application with `--no-bundle`.

The workflow does not install Codex, access an account, issue inference, publish artifacts, create a release, sign, or notarize an application.

### Verified GitHub Actions run

[Run 39366176954](https://github.com/Wu-Zzz/codex-autoresume/actions/runs/39366176954) completed successfully on 2026-10-20 for code commit `63c0658ce1b001765e533bf3bfd2028b000391c1`:

| Runner | Result |
| --- | --- |
| macos-latest | 11 frontend/probe tests, 27 Rust tests, formatting, Clippy, frontend build and release desktop compilation all passed |
| windows-latest | 11 frontend/probe tests, 27 Rust tests, formatting, Clippy, frontend build and release desktop compilation all passed |

This establishes cloud compilation and automated checks on both operating systems. It does not establish Windows GUI behavior, installation, signing, account discovery on a fresh machine, or safe real-account continuation. This documentation-only update does not alter the tested implementation.

Local commands are:

```text
npm ci
npm run check
npm test
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo test --locked --manifest-path src-tauri/Cargo.toml
cargo clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
npm run tauri -- build --no-bundle
```

The live probe is intentionally separate from these commands. Run it only when an account read is authorized; it must keep its method allowlist read-only.

## Checks still needed before distribution

- Test installation and GUI behavior on Windows; the Mac and Windows cloud build results are recorded above.
- Complete tray-menu interaction, forced concurrent-instance handling, and Windows lifecycle checks; basic macOS opening, close/reopen, and explicit quit were exercised above.
- Test official Codex executable discovery and explicit login handoff on fresh Mac and Windows installations.
- Verify that logs, storage, UI errors, and diagnostic exports contain no credentials or unintended conversation content.
- Prove ownership and configuration preservation before enabling any automated turn submission; the current external-chat path is intentionally blocked.
- Test real quota exhaustion and recovery on a disposable, explicitly authorized task, including lost response and simultaneous manual-input cases.
- Add packaging, signing, notarization, and installer validation when preparing an actual release.
