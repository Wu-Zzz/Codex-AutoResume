# Validation record

Updated 2026-10-20, Asia/Taipei. The managed execution revision is **in progress**.
Historical results below remain attached to their original code and do not
establish that the current working tree is release-ready.

## Current implementation and verification status

The working tree now contains a separate managed path for new tasks opened
through the application's official Codex terminal. It uses an application-owned
`CODEX_HOME`, explicit execution consent, an authenticated loopback gateway and a
single writer to each warm backend. Official login happens before the backend is
started. Existing Desktop/CLI histories remain **monitoring only**; their
monitoring switches are not execution consent.

The managed actor implements quota-trigger handling, strict quota gates,
history/idle rechecks, an atomic persisted intent before dispatch, response
reconciliation and refusal to replay uncertain submissions. It retains the warm
execution context and does not automatically cold-resume after restart or take
over unrelated sessions. Independent source review and local macOS packaging checks passed. These implementation statements are not real model-inference or quota-recovery acceptance.

| Current-revision check | Status and scope |
| --- | --- |
| Frontend tests | 15 passed for the current UI revision |
| Theme and quota presentation | Browser checks passed for the current theme/real-versus-demo presentation and quota views |
| Connection regression tests | Fixture checks passed for RPC rejection versus transport failure, immediate bad-connection cleanup and isolation of individual history failures |
| Strict quota parser and separate display telemetry | Targeted parser/scheduler checks passed; display unknowns remain unknown and cannot authorize execution |
| Integrated Rust tests | 80 passed locally, including managed actor, persistence, connection, quota and synthetic terminal/login cases |
| Rust formatting and Clippy | Passed; Clippy covered all targets with warnings denied |
| New macOS ARM64 application and DMG | Built: app 14.57 MiB, DMG 4.62 MiB; strict ad-hoc signature and disk-image integrity checks passed |
| Current macOS/Windows CI | [Run 39404011614](https://github.com/Wu-Zzz/Codex-AutoResume/actions/runs/39404011614), commit `a60bfd958f5b01bdb66047647a396281fc439312`, both macOS and Windows passed all tests, static checks and release application compilation; not a published release |
| Windows `CREATE_NEW_CONSOLE` terminal/login behavior | Compiled and tested in current Windows CI; real-machine terminal/login GUI verification still required |
| Real managed model inference | **Not performed** |
| Real quota exhaustion and automatic recovery | **Not performed** |
| Signing/notarization and fresh-machine acceptance | Not established for this revision |

New fixture coverage includes user input arriving during a recovery check,
approval/input interruption, changed or unknown configuration, multiple quota
buckets, existing monitoring consent, failed login, and lost acknowledgements.
Independent review found and resolved official TUI optional-field compatibility and non-sticky per-turn configuration issues. The counts above are the
current local pass; any subsequent code correction requires the affected checks
to be rerun. They are not proof of real quota recovery or package acceptance.

### Current packaged Mac UI check

The newly built native application launched successfully. Its existing local
account connection and live quota display were read-only. Native light/dark
switching updated both content and window appearance. The system folder picker
returned the isolated test directory, and automatic continuation was unchecked
by default. The new-task dialog was closed without opening a Codex terminal.
No existing task was enabled, disabled, removed, resumed or interrupted by this
check. Screenshots containing personal histories are not committed; public UI
images use preview fixtures only. Actual tray-icon interaction and Windows GUI
still need separate verification.

### Current live read-only connection and quota checks

The installed official CLI was `codex-cli 0.145.0`. A Finder-style minimal PATH
successfully launched its npm entry and resolved the sibling Node executable.
Five consecutive cycles of account status, quota read and one-page history
discovery succeeded on one connection. Account reads took 3–8 ms, list reads 1–13 ms
and quota reads 959–5029 ms in that sample. This did not reproduce a network
timeout and does not establish long-duration reliability.

A deliberately nonexistent synthetic thread ID returned JSON-RPC error
`-32600`. The next account read on that **same official server connection**
succeeded in 4 ms. This confirms that a normal RPC rejection does not itself
damage the transport. The previous adapter incorrectly retired its child on such
errors; the corrected adapter keeps healthy transports and immediately clears
actual EOF/timeout/malformed-frame failures without replaying requests.

One further quota response was passed in memory to the new strict Rust parser.
It parsed successfully, contained an identified bucket and reported a known
spend-control state. The output recorded only parser success and structural
availability. No account identity, actual quota percentages, reset timestamps,
private task names or credentials are included here.

All these real-account checks were read-only. They did not invoke login, resume,
turn start, interruption, approval or inference. Native-launch/login tests use
synthetic local CLI fixtures, not the user's official account. As required by
[AGENTS.md](../AGENTS.md), a real execution/recovery test needs explicit
authorization for a concrete disposable task; general development or release
authorization does not substitute for that test scope.

### Display and authentication boundaries

- Strict quota parsing covers complete server snapshots and all returned buckets.
  The independent display parser may expose valid known fields from a shape that
  the execution parser refuses. Neither a rendered percentage nor a reset
  countdown is execution permission.
- Failed quota reads, disconnects and missing login clear stale display data.
  Global task pause leaves quota refresh active. Managed account telemetry is
  selected when the managed path is connected, rather than assuming that it
  belongs to the ordinary local Codex account.
- Official Codex performs account login and credential storage. The application
  does not read/copy account credential files. The gateway uses its own generated
  local channel capability; macOS passes it through a mode-0600 one-use file and
  then the remote terminal environment, while Windows uses the environment
  directly. It is absent from command arguments and script content.
- Terminal fixtures cover literal arguments containing shell metacharacters,
  removal of nonce files, and refusal to continue after failed or incomplete
  login. Their eventual integrated result must not be described as a Windows
  real-machine or real-account acceptance test.

## Historical observation-only baseline

The remaining historical records describe earlier revisions. Their screenshots,
test counts, bundle sizes and CI commits must not be attributed to the current
managed implementation.

The initial live integration was an observation client. It inspected available
local histories through the official Codex App Server. Its quota endpoint call
only checked whether the call failed; returned buckets/reset timestamps were not
yet parsed into production scheduling.

That runtime passed empty quota buckets, unknown ownership and unpreserved
configuration to its safety gate. It neither claimed nor submitted production
continuation intents, and its transport excluded resume/start. The subsequently
added managed path is described above and in [the protocol audit](protocol-audit.md).

Fixture tests proved deterministic decisions, not that a real Desktop chat had
safely resumed. Successful builds proved compilation, not installation, signing
or a real quota-recovery cycle.

## Historical live read-only probe

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

## Historical automated checks

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

The historical native test did not add or enable any real task; its application
data had an empty task list. It did not establish tray-menu interaction, a forced
simultaneous second process, Windows behavior, code signing or notarization. That
macOS app was a local development artifact.

The repository's [CI workflow](../.github/workflows/ci.yml) defines independent `macos-latest` and `windows-latest` jobs:

1. Install locked Node dependencies and use stable Rust with `rustfmt` and `clippy`.
2. Run frontend type checks, tests, and build.
3. Run Rust formatting checks, scheduler/persistence tests, and Clippy with warnings treated as errors.
4. Compile the Tauri desktop application with `--no-bundle`.

The workflow does not install Codex, access an account, issue inference, publish artifacts, create a release, sign, or notarize an application.

### Historical verified GitHub Actions run

[Run 39366176954](https://github.com/Wu-Zzz/codex-autoresume/actions/runs/39366176954) completed successfully on 2026-10-20 for code commit `63c0658ce1b001765e533bf3bfd2028b000391c1`:

| Runner | Result |
| --- | --- |
| macos-latest | 11 frontend/probe tests, 27 Rust tests, formatting, Clippy, frontend build and release desktop compilation all passed |
| windows-latest | 11 frontend/probe tests, 27 Rust tests, formatting, Clippy, frontend build and release desktop compilation all passed |

This establishes cloud compilation and automated checks on both operating systems for the identified commit. It does not establish Windows GUI behavior, installation, signing, account discovery on a fresh machine, or safe real-account continuation. Later revisions need their own checks.

### Historical native minimal UI revision and capability correction

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

### Historical dark UI and installer prerelease preparation

The `v0.1.0-alpha.1` revision implements the accepted dark Figma direction in Vue, with a 960 × 640 main window and a separate 380 × 560 tray window. The native tray and main window share the same Rust state. No continuation protocol methods were enabled.

- Frontend type checking, production build and all 11 frontend/path tests passed locally.
- All 30 Rust tests, formatting and Clippy passed locally. Three new tests cover tray placement below a Retina menu bar, above the Windows taskbar and on a secondary display with a negative origin.
- Browser checks passed for monitor toggles, global pause/restore, task filtering and selection, search, hidden candidate actions, adding a task with monitoring off, removal, connection/capability dialogs, Escape/focus return and tray-to-detail navigation.
- The 960 × 640 main view and 380 × 560 tray view were inspected visually. No horizontal overflow, missing SVG assets, visible text below 12 px, or JavaScript runtime errors were found in these checks.
- The logo arrow was corrected in one canonical SVG and used to regenerate the macOS and Windows icons. Public screenshots contain isolated demo data.

The [installer workflow](../.github/workflows/release.yml) runs the required checks before producing two Mac DMGs and a Windows x64 NSIS installer. It verifies Mac image integrity/ad-hoc signatures and performs a Windows silent-install smoke check in a disposable runner. Only after all three jobs pass does it publish the prerelease with a complete installer set and SHA-256 checksums. The actual result is visible in [release workflow runs](https://github.com/Wu-Zzz/Codex-AutoResume/actions/workflows/release.yml).

This is an unsigned/unnotarized developer-identity preview, not a verified automatic-continuation product. Ad-hoc Mac signing is not Developer ID signing or notarization. Cloud installation smoke checks do not establish Windows GUI or fresh-machine account/login behavior.

## Checks still needed for general availability

- Finish final code review, current-revision automated tests, dual-platform CI and
  package checks. Do not reuse historical or intermediate passes for this gate.
- Test installation and GUI behavior on Windows, including official login and
  `CREATE_NEW_CONSOLE`; the historical cloud builds above do not validate the
  current terminal implementation.
- Complete tray-menu interaction, forced concurrent-instance handling, and Windows lifecycle checks; basic macOS opening, close/reopen, and explicit quit were exercised above.
- Test official Codex executable discovery and explicit login handoff on fresh Mac and Windows installations.
- Verify that logs, storage, UI errors, and diagnostic exports contain no credentials or unintended conversation content.
- Validate the managed warm-session ownership and configuration boundary end to
  end, including safe refusal after disconnect/restart. External-chat execution
  and automatic cold resume remain unsupported.
- Test real inference, quota exhaustion and recovery on a disposable, explicitly
  authorized task, including lost response and simultaneous manual-input cases.
- Obtain developer code-signing credentials, notarize the Mac build, and complete fresh-machine installation and lifecycle validation.
