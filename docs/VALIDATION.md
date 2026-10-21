# Validation record

Updated 2026-10-21, Asia/Taipei. The execution revision is **Alpha**.
Historical results below remain attached to their original code and do not
establish that the current working tree is release-ready.

## First normal-icon startup and compatibility fixes — 2026-10-21

- After the user reopened Codex from its usual icon, the original Desktop
  process did load the private bridge and modified backend. Read-only control
  snapshots exposed four original chats; all continuation switches remained off.
  This establishes startup and observation, not automatic continuation.
- That first launch exposed a missing official `codex-code-mode-host` helper.
  Placing the unchanged official helper beside the private backend restored tool
  execution in the current original chat without a second restart. New installs
  instead create a verified link to that signed official file, and check it again
  before selecting the backend. A missing or mismatched helper falls back before
  backend startup; an already-running backend is never replayed.
- A nested App Server started by the Desktop's Node tool also inherited the
  override, creating an empty second bridge and hiding quota telemetry. Installed
  routing now recognizes only the audited main Desktop invocation; the auxiliary
  invocation passes through to the official CLI. Manual diagnostic routing is
  unchanged. This change has process-level mock coverage, not a second live
  Desktop startup yet.
- The first live generation rejected an unclassified native method. Source audit
  identified startup `rollout/compress` as a likely trigger: the official cold-file
  worker checks file identity and coordinates with active writers. This maintenance
  method is now audited separately from reads; unknown parameters still block
  execution. Known project changes revoke consent without declaring the whole
  connection incompatible. Unknown methods still fail closed, with bounded
  method-name-only diagnostics and a persistent visible reason. The original
  generic diagnostic did not record the triggering method, so the exact live
  cause remains unconfirmed until the next startup.
- The standalone backend also lacked the full CLI's `sandbox` subcommand used by
  MCP launcher probing. A narrowly scoped patch forwards that subcommand to the
  validated official CLI and preserves raw arguments, process exit and stdio.
  It retains the modified backend's existing arg0 helpers; it does not change
  sandbox policy. The new forwarding module passed 17 isolated process checks.
- Connection text now uses plain Chinese, distinguishes connection from the
  ability to enable continuation, avoids repeating restart instructions after
  connection, and checks the installation again when the user clicks recheck.
- Revised application checks passed: frontend type checking, production build
  and 15 tests; 180 Rust unit tests, 6 wrapper/fake-backend process tests,
  formatting and Clippy. The installation
  status tests remove or replace helpers, damage configuration, change the token
  and simulate a Desktop upgrade, while preserving the ability to turn it off.
- Installation status now distinguishes the current runtime from retired, old
  protocol and manually launched connections. An old connection remaining online
  after an update cannot hide the need to restart Codex. Missing old fields stay
  backward compatible; unknown fields and invalid types are still rejected.
- The newly compiled backend and the official CLI produced identical stdout,
  stderr and successful exit codes for an isolated `sandbox --help` probe. This
  is a helper compatibility check, not a model request or sandbox execution test.
- Desktop voice remains unsupported by the modified runtime: official voice
  components require a matching package layout and build identity. No build
  identity was spoofed. This limitation is shown in connection details.
- No live continuation prompt, chat opt-in, approval, interruption or account
  mutation was issued during these checks. Real quota recovery and the revised
  backend in the original Desktop still need separate acceptance. No new Release
  has been published.

## Normal-icon integration — 2026-10-21

- The inspected macOS Desktop build `20770` loads its login/interactive shell
  environment with `CODEX_SHELL=1` before selecting the CLI. The new connection
  setting installs one removable conditional zsh block and private runtime files;
  it does not change the official app, system-global environment, credentials,
  databases, or rollout files. This is a version-specific experimental entry,
  not a documented general extension API.
- Original profile bytes and POSIX permissions are preserved. Conflicting custom
  CLI configuration is rejected. Missing components, invalid configuration or a
  different Desktop build fall back to the official CLI before backend startup;
  failures after guarded execution begins never start a replacement backend.
- Removing or replacing the installation token permanently retires an old live
  bridge's automatic controls, while original Desktop traffic remains usable.
  Installation does not authorize any chat. Per-chat consent is still explicit
  and bound to the live connection and observed failed turn.
- Current local checks passed: frontend type checking, 15 frontend tests and
  production build; 165 Rust unit tests, 4 wrapper/fake-backend process tests,
  formatting and all-target Clippy with warnings denied. Tests include shell
  quoting and unrelated environment preservation, profile conflicts/rollback,
  crash-released setup locking, zero-backend configuration preflight, and live
  installation revocation without quota-reply replay.
- The macOS ARM64 debug app was rebuilt (50.88 MiB), locally ad-hoc signed,
  and passed strict deep signature verification. Its native connection dialog
  successfully installed the one-time entry. A hash comparison confirmed every
  original profile byte outside the owned block and its POSIX mode were retained.
  Ordinary shell startup did not select the bridge; the Codex-scoped shell did.
  The installed wrapper's configuration preflight and official `--version`
  passthrough succeeded without protocol output or model requests. The installed
  runtime is under private application support, independent of the work checkout.
- Normal-icon original Desktop connection and actual quota exhaustion/recovery
  remain separate, pending acceptance. No live model prompt was submitted for
  this integration change. Windows execution activation remains unavailable.

## Interface corrections and activation status — 2026-10-21

- Revision `35c43bb` groups quota usage with its percentage, aligns the header
  controls at 32 px, clarifies task controls and moves the detail separator above
  the activity history. Light/dark workbench and 380 px tray layouts were visually
  checked; theme and pause controls worked in the demo, without horizontal overflow.
- Type checking, all 15 frontend tests and the production build passed. The macOS
  ARM64 debug app was rebuilt, locally signed and reopened; the updated controls,
  quota label and detail separator were checked in its native window. No scheduler
  code changed in this revision.
- The attempted original-Desktop activation stopped at `quit_desktop`; the
  original process remained running and the bridge was not loaded. The recorder
  did not retain enough error detail to establish the exact quit failure. A later
  native UI control attempt was explicitly denied access to Codex itself. No
  forced exit or substitute UI automation was used after that denial.
- Original-chat connection and a real quota recovery cycle remain unverified.
  The published installers are unchanged; this UI update is not a completion or
  release-readiness claim for original-chat continuation.

## Connection wording and activation preparation — 2026-10-21

- The UI distinguishes the original-chat experimental connection, managed terminal
  tasks and history-only monitoring. Counts describe enabled continuation, not
  every task that happens to use an executing adapter. Unpausing no longer says
  that it re-enables revoked Desktop consent. Quota labels identify their source.
- The Rust snapshot reports a live Desktop connection even before any chat is
  opened; the authenticated-loopback test also verifies disappearance after the
  connection closes. This is connection status, not execution authorization.
- Frontend type checking, 15 frontend tests, production build, 143 Rust unit
  tests, 2 wrapper integration tests, formatting and all-target Clippy passed.
  The Mac ARM64 debug bundle was rebuilt. Its incomplete development signature
  was replaced with a local ad-hoc signature and passed strict deep verification;
  the official Codex application was not modified.
- The user authorized restarting both applications for a read-only Desktop
  connection check. A one-shot restart and result recorder is prepared under
  ignored `work/`; this record does not yet claim that activation completed.

## Original Desktop chat bridge — 2026-10-20 source revision

The working tree adds the experimental macOS in-place path described in
[desktop-bridge.md](desktop-bridge.md). It uses the original Desktop connection
and chat ID with an explicitly modified, pinned official-source backend. The
published Alpha installers have **not** been replaced.

| Check | Result and scope |
| --- | --- |
| Installed Desktop protocol compatibility | Bundled `0.163.0-alpha.10.2` and release `0.163.0`: all 896 experimental TypeScript definitions match structurally; schema generation only, no account connection |
| Frontend | Type check, 15 tests and production build passed |
| AutoResume Rust | 143 unit tests and 2 real-wrapper/fake-backend process tests passed; formatting and all-target Clippy with warnings denied passed |
| Same-chat transport tests | Existing chat ID preserved; native dynamic tool request/reply and RPC IDs preserved; internal responses hidden; one conditional send; newer native input cancels old preflight; EOF cleans registration |
| Modified App Server | Compiled locally on macOS ARM64 with Rust 1.99.1; 3 live-failure state tests, 1 launch-argument test, 3 local-model integration tests and 1 Core race test passed |
| Execution-side integration | A local simulated quota failure continues to completion in the same thread, produces exactly one internal continuation fragment and no fabricated user message; repeated requests, normal completion, unloaded history and parameter overrides are refused |
| Corrected test fixture | The first integration run had a metadata assertion failure because the third-party mock provider strips internal metadata. The fixture now uses OpenAI serialization against the same local mock, retains and strengthens the assertion, and all 3 tests pass |
| Multi-instance / persistence gates | Ownership declaration and conditional write share a cross-process SQLite critical section; shared pause is persisted before IPC; known no-send/rejected attempts retain tombstones without blocking later distinct failures; uncertain writes remain blocked |
| Quota / account display | Desktop continuation requires `ordinaryUsageAllowed = true` and all returned bucket gates; display uses the same Desktop connection, excludes account metadata and clears on account changes |
| Local app and launcher | Updated macOS ARM64 debug `.app` built, 50.47 MiB; private manual launcher prepared and syntax checked, pins the installed Desktop build. AutoResume was reopened; original-Desktop activation did not complete (see latest record above) |
| Actual runtime read-only probe | The built wrapper and modified backend completed initialization, advertised conditional continuation, read the signed-in account and returned quota with the ordinary-usage gate. The bridge used an isolated state directory; zero model requests and zero thread mutations were issued |
| Reproducibility | Source SHA256, clean patch application, Apache notices and exact local-version lock repair checked; no external dependency versions changed |
| New-revision macOS / Windows CI | Source revision `5e81742`: [application checks](https://github.com/Wu-Zzz/Codex-AutoResume/actions/runs/39448683702) and [modified-backend checks](https://github.com/Wu-Zzz/Codex-AutoResume/actions/runs/39448744123) both passed. The subsequent documentation revision also passed [application CI](https://github.com/Wu-Zzz/Codex-AutoResume/actions/runs/39449124625) |
| Original Desktop live activation / Chrome-CUA-MCP behavior | **Not verified**; activation attempt stopped before the original Desktop exited, so the bridge was not loaded |
| Real account quota exhaustion and restoration | **Not performed**; no new live inference request or existing-chat mutation was issued |
| Windows activation | Explicitly disabled pending native environment and sandbox validation |

Upstream builds emit existing Rust deprecation and large-unwind-section linker
warnings. They completed successfully; the AutoResume Clippy gate itself passes.
This is targeted coverage of the new backend feature, not the entire upstream
Codex test suite. Compiled runtime and sanitized test logs were retained under
ignored `work/`; this run's large upstream compilation intermediates were removed
after verification.

## Previous managed-terminal implementation and verification

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
| Integrated Rust tests | 92 passed locally, including managed actor, persistence, connection, quota and synthetic terminal/login cases |
| Rust formatting and Clippy | Passed; Clippy covered all targets with warnings denied |
| Current macOS ARM64 application and DMG | Rebuilt with the icon, terminal-startup and late-read corrections: app 14.55 MiB, DMG 4.63 MiB; strict ad-hoc signature and disk-image integrity checks passed |
| Final macOS/Windows CI | [Run 39430135628](https://github.com/Wu-Zzz/Codex-AutoResume/actions/runs/39430135628), commit `ad70a473a0ea65a44603417915d742fbb6495b2f`, both macOS and Windows passed all tests, static checks and release application compilation |
| Windows `CREATE_NEW_CONSOLE` terminal/login behavior | Compiled in final Windows CI; real-machine terminal/login GUI verification still required |
| Authorized isolated terminal launch | Fresh native attempt confirmed script startup, followed by the user's official login and one isolated request; automatic continuation remained off |
| Real managed model inference | One authorized isolated request completed: user reported `AUTORESUME_SMOKE_OK`, native UI showed the running-to-completed transition, and the application database recorded a completed turn with zero automatic submission intents |
| Real quota exhaustion and automatic recovery | **Not performed** |
| Signing/notarization and fresh-machine acceptance | Not established for this revision |

New fixture coverage includes user input arriving during a recovery check,
approval/input interruption, changed or unknown configuration, multiple quota
buckets, existing monitoring consent, failed login, and lost acknowledgements.
Independent review found and resolved official TUI optional-field compatibility and non-sticky per-turn configuration issues. The counts above are the
current local pass; any subsequent code correction requires the affected checks
to be rerun. They are not proof of real quota recovery or package acceptance.

### Published Alpha and download verification

[v0.1.0-alpha.1](https://github.com/Wu-Zzz/Codex-AutoResume/releases/tag/v0.1.0-alpha.1)
is a public prerelease of code commit
`ad70a473a0ea65a44603417915d742fbb6495b2f`.
[Packaging run 39430135542](https://github.com/Wu-Zzz/Codex-AutoResume/actions/runs/39430135542)
passed every platform job and the publish job. Both Mac DMGs passed disk-image
integrity and ad-hoc signature checks. The Windows x64 NSIS package passed its
silent-install check in a disposable runner. The publish job verified all three
installer assets and their checksum file before making the release public.

All four public assets were independently downloaded after publication. The
three installers passed `shasum -a 256 -c SHA256SUMS.txt`. The release is not a
draft and is marked as a prerelease. Intel Mac is cross-compiled; Intel Mac and
Windows interactive GUI behavior remain unverified. Neither these packaging
checks nor the isolated normal turn establish real quota recovery.

The final local application was reopened successfully and displayed read-only
account quota. The completed isolated test entry was removed from AutoResume
after its sanitized verification record was saved; no original Codex history
was deleted, and no second model request was issued.

### Earlier Mac icon correction and package check

The Mac icon generator previously removed the canonical SVG root and lost its
inherited `fill="none"`, leaving the open C-loop filled black. It now nests the
complete source SVG, preserving its paint attributes and the existing 80%
optical size. The generated ICNS was decoded and visually inspected; its center
is forest green. The packaged ICNS matches that verified file byte for byte.

Frontend type checking, production build, 15 frontend/probe tests, 80 Rust tests,
formatting and Clippy passed again. The rebuilt ARM64 DMG SHA-256 is
`dc30d26e48901ea16774af120a0a83c74556e5d71e7bff98fb19fa184fd41c3a`.
The Mac was locked during the final native check. The running application was
not restarted while the isolated test awaited user input, so refreshed Dock
rendering has not been verified.

### Earlier packaged Mac UI check (before the authorized smoke attempt)

The newly built native application launched successfully. Its existing local
account connection and live quota display were read-only. Native light/dark
switching updated both content and window appearance. The system folder picker
returned the isolated test directory, and automatic continuation was unchecked
by default. The new-task dialog was closed without opening a Codex terminal.
No existing task was enabled, disabled, removed, resumed or interrupted by this
check. Screenshots containing personal histories are not committed; public UI
images use preview fixtures only. Actual tray-icon interaction and Windows GUI
still need separate verification.

### Authorized isolated smoke attempt

The user explicitly authorized one new official Codex task in the ignored
`work/live-smoke` directory, with the sole prompt:
`只回复 AUTORESUME_SMOKE_OK，不调用工具，不读取或修改文件`.
This authorization does not include additional model requests, deliberate quota
exhaustion, or resuming or interrupting existing conversations.

The application opened the official terminal for that isolated directory.
Automatic continuation remained off. The computer-use tool denied control of
the native Terminal application. The user manually inspected its window and
confirmed that the official CLI was waiting for OAuth login, then requested a
fresh attempt. The old isolated login process had ended, and the unused local
launch entry was removed through the application's UI. No account files were
inspected or changed by AutoResume or the development tools.

The corrected native application was then used for the fresh attempt. It
received the script's startup acknowledgement, and the user completed official
login and sent the authorized prompt once. The user reported the exact reply
`AUTORESUME_SMOKE_OK`. AutoResume's native UI showed the running and completed
events. A read-only query of AutoResume's own database independently confirmed
the completed turn status, disabled automatic consent and **zero automatic
submission intents** for this isolated task.

The same activity record also contained unmatched-response safety blocks
before and after completion. The first was 37 seconds after connection. Source
review identified a reproducible path where a timed-out read is removed from
the pending map and its late response is then treated as unknown. Without raw
RPC capture, the precise method responsible for the live event is unknown;
the timing is consistent with this bug but does not establish causation.

No existing conversation was resumed or interrupted. This establishes one
normal managed execution, with a connection-handling issue discovered during
the test. It does **not** establish automatic recovery across a quota window.
No further real model request was sent to validate the subsequent correction.

### Terminal startup confirmation

The initial launcher treated a successful system open request as proof that the
terminal script was running. The corrected Mac launcher waits up to 15 seconds
for an exact, nonsecret acknowledgement written by the script after consuming
its private local channel capability. An accepted open request without that
acknowledgement now fails visibly, cancels the unused managed connection and
cleans the attempt's files. Official login may still take as long as the user
needs after startup is confirmed.

The task view distinguishes opening, confirmed startup awaiting login/input,
and an authenticated terminal connection. A failure before thread creation
retains its specific reason; a late launch confirmation cannot restore revoked
execution consent. Five new fake-opener tests cover missing/invalid confirmation,
open failure, timeout, slow login, late invocation and private files. A sixth
test checks the pre-thread failure boundary. All 86 Rust tests, formatting,
Clippy and 15 frontend tests passed locally. These are fixture tests, not
real-account inference or quota-recovery evidence.

### Late responses to timed-out reads

The actor retains at most 256 exact request IDs for timed-out, explicitly
read-only RPCs. A matching late response is consumed once and discarded in full:
it cannot update account, quota or history state, forward a second response to
the caller, or trigger execution. Unknown, duplicate, evicted and write-request
IDs retain strict failure behavior. A timed-out automatic submission keeps its
durable ambiguous intent even if a later success response arrives.

The final local suite contains 92 passing Rust tests, including stale client and
internal reads, deferred thread-start protection, unknown/duplicate IDs, bounded
retirement and uncertain writes. Formatting and Clippy passed. These tests
reproduce and address the source defect; no additional real inference was
performed after this correction, and its connection behavior still needs
long-duration real-account observation.

The final local ARM64 DMG SHA-256 is
`1d699d426630d8ed5539aac50e81bf56169ae3c29ac4f9627aa645826cdb881c`.
Published installers are built independently by the tagged release workflow;
their checksums will be supplied in that release's `SHA256SUMS.txt`.

### Earlier live read-only connection and quota checks

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

The checks in this earlier read-only subsection did not invoke login, resume,
turn start, interruption, approval or inference. The automated native-launch/login
tests used synthetic local CLI fixtures, not the user's official account.
The separately authorized smoke attempt above has a narrower, explicit scope
under [AGENTS.md](../AGENTS.md); it does not authorize a real quota-recovery test.

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
- Validate both the managed warm-session and experimental Desktop ownership and
  configuration boundaries end to end, including safe refusal after disconnect
  or restart. Unbridged external chats and automatic cold resume remain unsupported.
- Test real quota exhaustion and recovery on a disposable, explicitly
  authorized task, including lost response and simultaneous manual-input cases.
- Obtain developer code-signing credentials, notarize the Mac build, and complete fresh-machine installation and lifecycle validation.
