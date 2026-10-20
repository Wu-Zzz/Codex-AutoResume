# Codex App Server integration audit

Checked 2026-10-20 (Asia/Taipei). Installed executable: `codex-cli 0.145.0`.

## Conclusion

Automatic recovery from a **confirmed quota failure in a session that this app owns** is feasible. The public protocol supports history discovery, terminal turn errors, live status, quota reset timestamps, and starting another turn in the same thread.

**Transparent takeover of arbitrary existing Codex Desktop or CLI sessions is not yet established as safe.** A new app-server can discover persisted history, but its `notLoaded` status does not prove that a different Desktop/CLI process is idle. The inspected schema exposes no cross-process ownership lease, compare-and-swap turn start, or documented exactly-once send operation. Do not ship unattended takeover based only on file timestamps, process-name checks, or a new server's runtime status.

A first implementation can safely deliver read-only discovery, explicit selection, quota status, a durable state machine, and a tested transport adapter. Automatic execution should require a proven ownership path, rather than displaying a successful automation toggle for a path that has only been simulated.

## Evidence and version boundary

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

- A tested ownership handoff/shared-backend path for each claimed Desktop/CLI integration; current public evidence does not close this gate.
- Demonstrate preservation of model, provider, reasoning effort, cwd, permission profile/sandbox, workspace roots, approvals routing, instructions, and required tools across a backend restart.
- Lost acknowledgement, simultaneous user message, two app instances, sleep/wake, stale quota notification, and multiple limit-bucket fixtures must fail safely.
- Actual Mac and Windows packaged-app smoke tests, including executable discovery, child process lifetime, tray shutdown/relaunch, and login handoff.
- A controlled live quota-failure/recovery run on an explicitly authorized disposable task. The audit intentionally did not consume inference or exercise a real user's quota boundary.

These gaps limit the claim of readiness; they do not prevent building the UI, durable scheduler, protocol adapter, and deterministic test suite now.
