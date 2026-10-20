# Validation record

Initial development baseline: 2026-10-20, Asia/Taipei.

## What this alpha can claim

The live integration is an observation client. It can inspect available local histories through the official Codex App Server. It calls the quota endpoint but only checks whether the call failed: returned buckets/reset timestamps are not parsed into the production scheduler. The successful probe below establishes endpoint connectivity and response shape, not a completed live quota feature.

The runtime passes empty quota buckets, unknown ownership, and unpreserved configuration into its safety gate. Production code does not claim or submit continuation intents; resume/start methods are excluded from the transport allowlist, and execution notifications are not consumed. Those implementation gaps exist in addition to the external-chat ownership/configuration limitations in [the protocol audit](protocol-audit.md). An application-owned execution path is a possible architecture, not an implemented capability.

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

This establishes cloud compilation and automated checks on both operating systems for the identified commit. It does not establish Windows GUI behavior, installation, signing, account discovery on a fresh machine, or safe real-account continuation. Later revisions need their own checks.

### Native minimal UI revision and capability correction

On 2026-10-20, the interface was simplified to one toolbar, a flat task list, and explicit monitoring switches. The runtime's user-facing messages and documentation now distinguish the implemented observation client from the unimplemented continuation path. Demo fixtures no longer imply a working live quota countdown.

- Local type checking, frontend production build and 11 frontend/probe tests passed.
- Rust formatting, 27 tests and Clippy passed after the runtime/scheduler copy changes.
- Browser interaction checks passed for adding, searching, rejecting a hidden selection, default-off monitoring, toggling, pausing/restoring, task details, removal and both information dialogs. These use isolated demo data.
- 760 × 580 and 560 × 580 browser views were visually checked; the narrower view had no horizontal overflow. Visible text is at least 12 px, with text contrast checked against its background.
- The updated macOS ARM64 debug `.app` bundle compiled successfully. This revision was not newly validated on a real Windows machine or through a real quota interruption/recovery cycle.
- [Three Figma views](DESIGN.md) were rendered and inspected. Their final structures contain editable text, vectors and component instances, with no image-filled UI layers. Prototype page connections were added; they do not implement application execution.

The prior successful dual-platform CI run above belongs to the earlier commit. The new push triggers an independent run; consult GitHub Actions for its current result. No GitHub Release or installer is published by this revision.

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
