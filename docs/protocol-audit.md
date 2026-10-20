# Codex App Server integration audit

## Current requirement: original Desktop chat, in place

The user's latest requirement supersedes the terminal-only compromise below:
continue the selected conversation **inside its original Codex Desktop window**.
A new terminal, a fork, a new conversation or a context migration is not an
acceptable substitute. The released application does not implement this path.

### 0.163.0 and Desktop entry-point recheck

The official `rust-v0.163.0` release was downloaded into ignored `work/`, and
its experimental TypeScript schema was generated without accessing an account.
The official source and the installed macOS Desktop's static entry points were
inspected read-only. No existing thread was resumed, interrupted or queued;
Desktop was not restarted and its authentication was not inspected.

- The inspected running Desktop uses its own stdio App Server. Starting another
  server does not attach to that live instance.
- An ordinary thread deep link can navigate to a conversation and prefill its
  composer. Prefilling is not submission.
- Desktop also has a wake route for conversations with existing queued input.
  More significantly, the official persistent queue can notify the already-live
  owner through its external-commit polling. Therefore the earlier general
  assumption that there is *no route at all* into the original instance is too
  strong.
- The queue's execution uses an atomic idle-only start, which avoids steering
  input into an active turn. It does **not** bind that start to the failed turn
  that justified automatic continuation.

The decisive regression test is
[`a_new_turn_preserves_queued_messages_until_it_completes`](https://github.com/openai/codex/blob/rust-v0.163.0/codex-rs/app-server/tests/suite/v2/thread_queue.rs#L864):
queued input survives interruption and a newer user turn, then executes after
that newer turn completes. This is correct for a general message queue, but
violates this application's requirement to invalidate a quota continuation
after user stop or newer work.

Other relevant evidence:

- [`ThreadQueueAddParams` and related queue requests](https://github.com/openai/codex/blob/rust-v0.163.0/codex-rs/app-server-protocol/src/protocol/v2/thread.rs#L935)
  have no expected-previous-turn or quota-failure precondition.
- [Queue service](https://github.com/openai/codex/blob/rust-v0.163.0/codex-rs/ext/queue/src/service.rs#L89)
  observes external commits and can dispatch queued input automatically. Enqueue
  is already an execution-enabling operation, not a harmless draft step.
- [Queue insertion](https://github.com/openai/codex/blob/rust-v0.163.0/codex-rs/state/src/runtime/queued_items.rs#L77)
  creates a new item ID for every enqueue; `clientUserMessageId` is not a
  deduplication key.
- The core already provides
  [`continue_turn_if_idle` with an expected previous turn](https://github.com/openai/codex/blob/rust-v0.163.0/codex-rs/core/src/codex_thread.rs#L453).
  Ordinary public turn start and queue dispatch do not expose that full guard.

An external preflight followed by enqueue does not close the gap: the user can
act between those operations or before dispatch. A later queue deletion can race
with dispatch in the original server. Neither polling nor a lock in AutoResume
can make that check atomic in the Desktop owner.

### Implemented experimental execution-side support

The persistent queue was rejected for this automatic path. The source now adds
a transparent stdio bridge through Desktop's statically verified
`CODEX_CLI_PATH` entry point, plus an explicit modified-source extension to the
official 0.163.0 App Server. The original Desktop loads the original chat and
continues to handle native tool/approval traffic.

The distinct `turn/continueIfFailed` RPC accepts only a thread ID and expected
failed-turn ID. Its live evidence and configuration checks delegate final
reservation to Core's atomic `continue_turn_if_idle`. It cannot steer, enqueue,
or cold-resume. The bridge queries `autoresume/capabilities` and refuses to fall
back to normal `turn/start` when unsupported. Both extension methods are
project additions, **not official public protocol guarantees**.

The installed Desktop's bundled `0.163.0-alpha.10.2` and official release
`0.163.0` produced 896 experimental TypeScript definitions each; all matched
structurally after removing comments/whitespace. Only `generate-ts` and version
commands were used. This is schema compatibility evidence, not validation of
Chrome/CUA, native helpers, MCP behavior or real-account execution.

The preparer checks the audited CLI version and pins the installed Desktop
build in its manual launcher. An application update requires a fresh check.
The Windows override changes native sandbox/runtime setup, so activation there
is refused. No installed Desktop application, official database, auth file or
rollout was modified, and no existing chat was resumed or interrupted in this
development run.

The source and reproducible patch are described in
[desktop-bridge.md](desktop-bridge.md) and
[the integration package](../integrations/codex/README.md). The published Alpha
still has the older capabilities. Real Desktop activation and real quota
recovery remain separate acceptance steps.

## Previous managed-terminal audit

Checked 2026-10-20 (Asia/Taipei). Installed executable: `codex-cli 0.145.0`.
The managed execution revision below is **in progress**: final adversarial review,
packaged native checks and release validation have not yet closed. Current local
automated results are recorded separately in [VALIDATION.md](VALIDATION.md).

## Conclusion

Automatic recovery from a **confirmed quota failure in a session that this app owns** is feasible. The public protocol supports history discovery, terminal turn errors, live status, quota reset timestamps, and starting another turn in the same thread.

**Transparent takeover of arbitrary existing Codex Desktop or CLI sessions is not yet established as safe.** A new app-server can discover persisted history, but its `notLoaded` status does not prove that a different Desktop/CLI process is idle. The inspected schema exposes no cross-process ownership lease, compare-and-swap turn start, or documented exactly-once send operation. Do not ship unattended takeover based only on file timestamps, process-name checks, or a new server's runtime status.

The earlier revision implemented observation only. The working tree now adds a
separate managed execution path for **new tasks started through this application**.
That is an implementation change, not evidence that a real quota interruption has
recovered successfully. Existing Desktop/CLI histories remain observation only,
and their monitoring selections are never migrated into execution consent.

## Current managed path — validation in progress

The implementation in `src-tauri/src/managed.rs` owns a private application
`CODEX_HOME`, a loopback gateway and a separate official stdio backend for each
managed launch. It does not attach to an arbitrary running Desktop backend. The
user selects the project and grants separate automatic-continuation consent, then
uses the **official Codex terminal** for the task, approvals and answers.

The launcher first checks official `codex login status` in that same managed home.
If login is needed, it runs official `codex login` and checks again before opening
the remote terminal. A successful status exit alone does not establish a ChatGPT
account: the gateway additionally requires the structured `account/read` type to
be `chatgpt` before creating a managed thread. It does not accept API-key login
payloads or switch payment modes.

The backend starts lazily after the authenticated native client connects. This
ordering matters in 0.145.0: `AuthManager::auth()` returns from an empty cached
value without loading newly created login state, and the provider's account read
uses `auth_cached()`. Starting the backend before a separate official login could
therefore leave it unauthenticated. The implementation does not solve this by
reading or copying credentials. These findings come from the inspected
[`AuthManager` source](https://github.com/openai/codex/blob/rust-v0.145.0/codex-rs/login/src/auth/manager.rs#L2023)
and [provider account source](https://github.com/openai/codex/blob/rust-v0.145.0/codex-rs/model-provider/src/provider.rs#L285).

The ownership boundary is one still-live backend generation:

- The loopback gateway requires a generated local channel capability, rejects
  browser-origin connections and permits one connected native client. The
  capability is not an OpenAI credential and is not exposed to the webview or
  placed in command arguments. The macOS launcher uses a mode-0600 one-use file;
  Windows passes it only in the remote process environment. Official login keeps
  responsibility for account credential storage.
- One actor serializes native-client requests and automatic submissions to its
  owned backend. A new user turn reserves that writer and invalidates an older
  continuation check. The gateway refuses arbitrary external thread IDs, forks
  and unverified protocol/configuration changes.
- Only a structured terminal `usageLimitExceeded` for the current managed turn
  can begin recovery. Fresh ChatGPT account state, complete quota state, current
  history, idle status, consent, configuration and the live client generation
  must still agree before an automatic turn is eligible.
- The SQLite intent is claimed atomically before dispatch, keyed by thread and
  failed turn. An existing claim cannot submit again. An uncertain acknowledgement
  becomes ambiguous and disables further automatic work; it is not replayed.
  This is an application safeguard, not a claim that the remote protocol provides
  exactly-once delivery.
- Automatic starts share one application-level slot. Approval/input requests
  require the native user. User stop, login changes, configuration changes,
  disconnect or backend loss revoke or pause automatic execution as appropriate.
- Automatic continuation retains the warm session rather than reconstructing a
  historical execution configuration. Application restart does not cold-resume
  managed work or treat persisted consent as a live ownership lease. A terminal
  rejoin is limited to the already-created session in its still-live backend.

These are the implemented restrictions being reviewed and exercised with
fixtures. **No real model inference or real quota-exhaustion/recovery test has
been performed for this revision.** A concrete disposable-task execution test
requires explicit authorization under [AGENTS.md](../AGENTS.md).

### Quota parsing and display are separate

The execution parser consumes complete `account/rateLimits/read` snapshots, not
sparse notifications. It validates all known fields, ranges and nullable values;
rejects unknown fields/enums and conflicting identities; reads every map bucket;
and checks that the compatibility view agrees. Only a null map permits fallback
to an explicitly identified single bucket. The managed path conservatively gates
on **all returned buckets**, without guessing a model-to-bucket relationship.
Unknown spend-control state remains blocking. Credits and reset credits never
replace subscription-window availability.

A separate display parser can show valid known measurements when strict
execution parsing refuses a future or incomplete shape. Unavailable measurements
remain `null`; it does not invent a percentage or a five-hour deadline. Display
telemetry cannot grant permission to execute. Failed reads, lost connections and
missing login clear stale telemetry; pausing task monitoring does not stop quota
refresh. When a managed account is connected, the application uses that managed
telemetry rather than assuming it matches the ordinary local Codex account.

### Connection error correction and read-only evidence

The observation transport previously closed its child for any JSON-RPC error.
The runtime could then retain that unusable connection, fail subsequent task
reads and report the disconnect only in a later refresh. The correction treats a
valid correlated RPC rejection separately from a malformed frame, EOF or timeout.
Only damaged/uncertain transports are retired; they are cleared immediately and
their requests are not replayed. One unavailable history no longer invalidates
other history reads.

On the installed 0.145.0 binary, a deliberately nonexistent synthetic thread ID
returned RPC code `-32600`; the next `account/read` on the **same connection**
succeeded. Five consecutive read-only account/quota/list cycles also succeeded
under a Finder-style minimal PATH. One real quota response passed the new strict
parser through an in-memory validation pipe. No account identity, quota
percentages, conversation content or credentials were retained in this record.
These samples establish this particular error-handling defect and basic
compatibility, not long-duration connection reliability.

## Historical protocol audit and version boundary

The following initial audit evidence remains relevant to the 0.145.0 protocol.
Its observation-only implementation statements describe the earlier baseline;
they are not validation of the new managed path.

- Official [App Server documentation](https://learn.chatgpt.com/docs/app-server) was searched and fetched before inspecting the local executable. It documents thread lifecycle, read/list/resume, quotas, approvals, and transports.
- Ran `codex --version`, app-server/daemon/proxy help, and both stable and experimental TypeScript schema generation. Generated scratch references are under `work/protocol-schema/` and `work/protocol-schema-experimental/`.
- Ran only the read-only daemon version probe. It failed because the default `~/.codex/app-server-control/app-server-control.sock` did not exist. No daemon was installed, started, stopped, or restarted.
- No user thread was resumed; no thread or turn was created; no account, configuration, task, or approval was mutated; no model inference was issued by this audit.
- Current documentation is ahead of the installed stable schema. For example, `thread/turns/list` and `ThreadResumeParams.excludeTurns` are present in 0.145.0's **experimental** generated output but absent from its stable output. Use runtime/version capability handling, not documentation examples alone.
- Error names in the prose documentation use PascalCase in one section; the generated wire enum uses camelCase. Match `usageLimitExceeded` for this CLI version.
- Inspected official source at the exact `rust-v0.145.0` tag after the schema audit. Source files were downloaded to `work/` for read-only analysis. The decisive source findings below need no inference test.

## Protocol contract used by an adapter

Launch an owned child as `codex app-server --listen stdio://`. The wire format is newline-delimited JSON-RPC without the `jsonrpc` property. Send `initialize`, wait for its response, then send `initialized`. Client metadata should identify Codex AutoResume accurately.

| Purpose | RPC / event | Required behavior |
| --- | --- | --- |
| Discover histories | `thread/list` | Paginate until `nextCursor` is null. Request explicit source kinds. Use `useStateDbOnly: true` when avoiding metadata scan-and-repair. |
| Inspect selected history | `thread/read` with `threadId` and `includeTurns: true` | Does not resume, subscribe, or load the thread. Legacy full history can be expensive; use supported pagination when available. |
| Inspect local live ownership evidence | `thread/loaded/list` | Lists threads loaded in **that server's memory**, not a global ownership inventory. |
| Join/reopen thread | `thread/resume` with `threadId` | A mutating lifecycle operation. If the ID is already running in that server, the generated schema says it rejoins it. Do not use it merely to preview a task. |
| Confirm quota failure | `turn/completed` | Require `turn.status == "failed"` and `turn.error.codexErrorInfo == "usageLimitExceeded"`. Persist the exact failed `turn.id`. |
| Observe transient errors | `error` | Includes `willRetry`, `threadId`, and `turnId`; an error alone is not a terminal quota interruption. |
| Check quotas | `account/rateLimits/read` | Returns single-bucket compatibility view plus optional `rateLimitsByLimitId`. Never redeem reset credits automatically. |
| Observe quota change | `account/rateLimits/updated` | Treat notifications as a prompt to reconcile the relevant current state. |
| Observe user waiting states | `thread/status/changed` | `activeFlags` can include `waitingOnApproval` and `waitingOnUserInput`; never auto-answer these. |
| Continue one owned thread | `turn/start` | Send only after current history, live state, quota, consent, and configuration have been reconciled. |
| Detect goal boundary | `thread/goal/get` | Optional extra stop evidence; statuses include `active`, `paused`, `blocked`, `usageLimited`, `budgetLimited`, `complete`. Reading a goal does not authorize changing it. |

Explicit discovery sources for ordinary root sessions: `cli`, `vscode`, `appServer`, and `exec`. Omitted/empty `sourceKinds` defaults to interactive `cli` and `vscode` only. Exclude `parentThreadId != null` by default; spawned subagents are not independent selected user tasks. Show `name` when available, with a bounded preview fallback; retain `id`, `cwd`, provider, timestamps, source, and CLI version internally.

Relevant 0.145.0 types:

```ts
type ThreadStatus =
  | { type: "notLoaded" }
  | { type: "idle" }
  | { type: "systemError" }
  | { type: "active"; activeFlags: ("waitingOnApproval" | "waitingOnUserInput")[] };

type RateLimitWindow = {
  usedPercent: number;
  windowDurationMins: number | null;
  resetsAt: number | null; // Unix seconds
};

type ErrorNotification = {
  error: { message: string; codexErrorInfo: unknown; additionalDetails: string | null };
  willRetry: boolean;
  threadId: string;
  turnId: string;
};
```

`turn/start` accepts `clientUserMessageId`, but it is not a proven idempotency key. The [0.145.0 submission path](https://github.com/openai/codex/blob/rust-v0.145.0/codex-rs/core/src/session/mod.rs#L778-L794) generates a new submission ID on each call and carries the client ID into the submission. The inspected dispatch path contains no deduplication guarantee. Never assume resending after a lost response cannot duplicate work.

### Confirmed active-turn race

The [0.145.0 turn processor](https://github.com/openai/codex/blob/rust-v0.145.0/codex-rs/app-server/src/request_processors/turn_processor.rs#L566-L585) submits `Op::UserInput`. The [core input handler](https://github.com/openai/codex/blob/rust-v0.145.0/codex-rs/core/src/session/handlers.rs#L229-L242) then calls `steer_input` with **no expected turn ID**. If a regular turn is active, [the steer implementation](https://github.com/openai/codex/blob/rust-v0.145.0/codex-rs/core/src/session/mod.rs#L3839-L3912) appends the input to that active turn; it starts a new task only when no active turn exists. Review/compaction turns are rejected as not steerable.

Therefore `thread/read` reporting idle followed by `turn/start` is **not an atomic idle-only start**. A user could start a new turn in between, and the automated continuation would steer that unrelated new turn. Shared daemon state alone does not close this race. The core library has an internal `try_start_turn_if_idle` mechanism, but it is not exposed by the inspected app-server request schema. A safe execution route needs one controlled input writer or a future atomic public API.

## Preserving configuration and authorization

The stable `ThreadResumeResponse` returns the effective model, model provider, service tier, cwd, instruction sources, approval policy, approvals reviewer, sandbox policy, and reasoning effort. Experimental output additionally exposes named permission profile provenance and runtime workspace roots.

The read/list `Thread` shape does not expose enough information to reconstruct all those settings before resuming. `config/read` accepts `cwd` and `includeLayers`, not a thread ID: it reads effective on-disk configuration, not that stored thread's exact former settings. There is no complete stable read-before-resume configuration API in the inspected contract.

Omission of optional resume overrides is the least invasive request, but it does not establish full configuration equivalence. The [0.145.0 cold-resume code](https://github.com/openai/codex/blob/rust-v0.145.0/codex-rs/app-server/src/request_processors/thread_processor.rs#L3136-L3189) merges stored metadata and then loads configuration for the project cwd. Its [persisted metadata merge](https://github.com/openai/codex/blob/rust-v0.145.0/codex-rs/app-server/src/request_processors/thread_processor.rs#L149-L195) restores model/provider/reasoning effort when no model override is given and separately restores approvals reviewer. The [session configuration construction](https://github.com/openai/codex/blob/rust-v0.145.0/codex-rs/core/src/session/mod.rs#L657-L677) obtains approval policy and permission profile from the loaded config. This is stronger evidence than a general warning: cold resume must not be assumed to preserve every original sandbox and approval setting.

For sessions owned from the start, capture the effective response/settings and the authorized transport/process context. During a still-live owned session, omit turn overrides to preserve that session's current settings. Reconcile later user changes instead of replaying a stale settings snapshot. After a crash/restart, compare the actual resumed settings before starting a turn; stop for attention if model, provider, cwd, approval routing, sandbox/profile, workspace roots, or necessary tools cannot be verified.

For imported sessions, initial explicit takeover needs a verified ownership transition and configuration review. A child process should never force `approvalPolicy: "never"`, full access, a different model, or replacement developer instructions merely to make recovery work. `thread/resume` is not itself a safe read-only configuration inspector.

## Does daemon/proxy solve ownership?

`codex app-server daemon` manages a shared app-server; `codex app-server proxy --sock PATH` forwards stdio to a running Unix control socket. They can provide **one server instance** shared by cooperating clients. This can avoid loading a second independent copy of a thread and makes that server's live status relevant.

They do not, from the inspected contract, provide an exclusive task lease, prevent a second client from sending a turn, attach to every already-running Desktop backend, or prevent a separate CLI process from reopening the same persisted history. The installed machine had no default daemon socket, so existing Desktop-session ownership was not proven through this path. Windows daemon/proxy behavior was not tested on this Mac.

A safer managed architecture is: AutoResume owns one backend and mediates **all** input writes to it; one app-level durable scheduler serializes both user and automatic turns. Merely allowing independent interactive clients to connect to that backend leaves the active-turn race above. Do not present this architecture as transparent compatibility with arbitrary current Desktop chats until tested against that product's actual supported connection path.

## Scheduling decisions

1. User explicitly selects a thread and enables recovery. Persist the selection and monitoring baseline; do not send a prompt merely because it was selected.
2. Observe an exact terminal quota error for the latest turn of an owned thread. Persist the failed turn ID and effective context. A normal completion, interruption, manual stop, budget limit, authentication failure, approval request, or generic HTTP 429 is not an auto-resume trigger.
3. Read current rate limits. Inspect all relevant primary/secondary windows and metered buckets. `resetsAt` is nullable, and limit exhaustion can instead represent workspace credits or spending controls. Unknown/ambiguous bucket mapping needs an explicit conservative policy; never assume the compatibility bucket represents every model.
4. Schedule against the returned reset time, with a small delay/jitter, and fetch limits again when due. Never hard-code "five hours after failure" or infer recovery from elapsed time alone.
5. Immediately before sending, re-read selected thread history and live status. Require the failed turn still to be the latest turn, no active turn or pending approval/input, no user disable/pause, no incompatible goal boundary, and a valid ownership/configuration context. These checks are necessary but insufficient if another independent writer can call `turn/start`; that configuration remains read-only.
6. Persist a send intent before dispatch, keyed by selected thread plus failed turn. Send at most one concise continuation message retaining the original task scope. Record the acknowledged new turn ID.
7. If the process or connection fails between dispatch and acknowledgement, treat delivery as unknown. Reconcile history for the submitted message/new turn. If it cannot be established, pause for attention instead of replaying. A SQLite transaction alone cannot make an external RPC exactly-once.
8. Restart/wake-up follows the same reconciliation path. A second app instance must not run a second scheduler. Quota exhaustion in the resumed turn creates a new failure generation; normal completion stops this recovery cycle.

## Release gates still required

- Close final code review and rerun the full current-revision checks after the
  managed actor and failure fixtures have settled. Intermediate test counts do
  not establish a completed release gate.
- Validate the warm managed path end to end: official login, native TUI,
  permissions/configuration, manual input racing an automatic check, lost
  acknowledgement, sleep/wake and safe refusal after a backend restart.
- Validate current macOS and Windows packages, including discovery, terminal
  launch, child lifetime, tray shutdown/relaunch and fresh-machine login.
  Windows `CREATE_NEW_CONSOLE` handling still needs current-revision CI and
  Windows real-machine verification.
- Run a controlled real quota-failure/recovery test on a specifically authorized
  disposable task. No such inference or recovery test has been performed.
- Any future claim of external Desktop/CLI takeover or cold resume needs its own
  ownership/configuration evidence. Neither capability is claimed by the current
  managed warm-session design.

See [VALIDATION.md](VALIDATION.md) for the separation between historical checks,
current fixture/read-only results and outstanding release verification.
