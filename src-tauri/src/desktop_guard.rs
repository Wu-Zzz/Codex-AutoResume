//! Automatic continuation policy for a transparent Desktop stdio bridge.
//!
//! Native traffic is observed, never answered or rewritten here. Only the
//! executing server's conditional API can submit a continuation. Local history
//! and a successful preflight alone never grant that authority.
use crate::{
    quota,
    scheduler::{
        self, Decision, Ownership, SafetyGate, TaskRecord, TaskState, TaskView, TurnObservation,
        TurnStatus,
    },
    store::{ClaimResult, Store},
};
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::Path};

const REQUEST_TIMEOUT_SECONDS: i64 = 30;
const RETRY_SECONDS: i64 = 60;

#[derive(Clone, Debug, PartialEq)]
pub struct GuardRequest {
    pub id: u64,
    pub method: String,
    pub params: Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Capability {
    Unchecked,
    Supported,
    Unsupported,
}

struct DesktopThread {
    task: TaskRecord,
    revision: u64,
    /// Only a terminal event from this backend generation populates this field.
    candidate: Option<String>,
    latest: Option<TurnObservation>,
    next_check: i64,
    awaiting_user: bool,
}

#[derive(Clone)]
struct Attempt {
    thread: String,
    failed: String,
    revision: u64,
}

enum Purpose {
    Capabilities,
    Account(Attempt),
    Quota(Attempt),
    Continue(Attempt),
}

struct Pending {
    id: u64,
    deadline: i64,
    purpose: Purpose,
}

pub struct DesktopGuard {
    store: Store,
    threads: BTreeMap<String, DesktopThread>,
    ready: bool,
    compatible: bool,
    storage_ok: bool,
    paused: bool,
    capability: Capability,
    next_id: u64,
    pending: Option<Pending>,
    outgoing: Option<GuardRequest>,
    automatic_thread: Option<String>,
    telemetry: Option<quota::QuotaTelemetry>,
    native_work: BTreeMap<String, Option<String>>,
}

impl DesktopGuard {
    pub fn open(db: &Path) -> Result<Self, String> {
        let store = Store::open(db).map_err(|e| e.to_string())?;
        let paused = store.is_paused().map_err(|e| e.to_string())?;
        // Do not restore consent or recover all rows here: another live Desktop
        // bridge may share the tombstone database. Existing claims always block.
        Ok(Self {
            store,
            threads: BTreeMap::new(),
            ready: false,
            compatible: true,
            storage_ok: true,
            paused,
            capability: Capability::Unchecked,
            next_id: 1,
            pending: None,
            outgoing: None,
            automatic_thread: None,
            telemetry: None,
            native_work: BTreeMap::new(),
        })
    }

    pub fn native_client(&mut self, message: &Value, now: i64) {
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            // Track only opaque IDs for native work. Never inspect tool,
            // approval, attestation or authentication response contents.
            if let Some(id) = message.get("id") {
                if message.get("result").is_some() ^ message.get("error").is_some() {
                    self.native_work.remove(&id.to_string());
                } else if self.native_work.contains_key(&id.to_string()) {
                    self.incompatible("客户端响应无法识别，自动续跑已关闭", now);
                }
            }
            return;
        };
        if method == "initialized" {
            self.ready = true;
            return;
        }
        if method == "initialize" {
            return;
        }
        let thread = message.pointer("/params/threadId").and_then(Value::as_str);
        match method {
            "turn/start"
            | "turn/steer"
            | "turn/interrupt"
            | "thread/settings/update"
            | "thread/goal/set"
            | "thread/goal/clear"
            | "thread/archive"
            | "thread/unsubscribe"
            | "thread/rollback"
            | "thread/compact/start"
            | "thread/realtime/start"
            | "thread/queue/add"
            | "thread/queue/update"
            | "thread/queue/remove"
            | "thread/delete"
            | "thread/metadata/update"
            | "thread/inject_items"
            | "thread/shellCommand"
            | "thread/approveGuardianDeniedAction"
            | "review/start" => {
                if let Some(id) = thread {
                    self.revoke_thread(id, "用户操作已改变当前轮次，自动续跑已关闭", now);
                } else {
                    self.revoke_all("无法定位用户操作，自动续跑已关闭", now);
                }
            }
            "account/login/start"
            | "account/login/cancel"
            | "account/logout"
            | "config/value/write"
            | "config/batchWrite"
            | "configRequirements/write"
            | "remoteControl/enable"
            | "remoteControl/disable"
            | "skills/extraRoots/set"
            | "skills/config/write"
            | "experimentalFeature/enablement/set"
            | "config/mcpServer/reload"
            | "mcpServer/oauth/login"
            | "mcpServer/tool/call"
            | "plugin/install"
            | "plugin/uninstall"
            | "marketplace/add"
            | "marketplace/remove"
            | "marketplace/upgrade"
            | "externalAgentConfig/import"
            | "account/rateLimitResetCredit/consume"
            | "command/exec"
            | "command/exec/write"
            | "command/exec/terminate"
            | "fs/writeFile"
            | "fs/createDirectory"
            | "fs/remove"
            | "fs/copy" => {
                self.telemetry = None;
                self.revoke_all("账号或执行配置已变化，自动续跑已关闭", now);
            }
            "thread/start" | "thread/resume" | "thread/fork" => {
                if let Some(id) = thread {
                    self.revoke_thread(id, "会话运行上下文已变化，请重新开启自动续跑", now);
                }
            }
            _ if known_native_read(method) => {}
            _ => self.incompatible("Desktop 使用了未验证的协议操作，自动续跑已关闭", now),
        }
    }

    pub fn native_server(&mut self, message: &Value, original_request: Option<&Value>, now: i64) {
        if let Some(method) = message.get("method").and_then(Value::as_str) {
            let params = message.get("params").unwrap_or(&Value::Null);
            let thread = params.get("threadId").and_then(Value::as_str);
            if message.get("id").is_some() {
                if matches!(
                    method,
                    "item/tool/call" | "currentTime/read" | "attestation/generate"
                ) {
                    // These are native runtime capabilities, not human approval.
                    // Keep consent, but do not submit while a reply is pending.
                    let key = message["id"].to_string();
                    if self.native_work.len() >= 1024 || self.native_work.contains_key(&key) {
                        self.incompatible("客户端请求标识无法确认，自动续跑已关闭", now);
                    } else {
                        self.native_work.insert(key, thread.map(str::to_owned));
                    }
                    return;
                }
                if thread.is_none() || self.automatic_thread.as_deref() == thread {
                    self.automatic_thread = None;
                }
                // Approval, input, dynamic tools, auth and future capabilities
                // stay entirely in the original Desktop connection.
                if let Some(id) = thread {
                    self.revoke_thread(id, "Codex 需要客户端处理，自动续跑已关闭", now);
                    if let Some(state) = self.threads.get_mut(id) {
                        state.awaiting_user = true;
                    }
                } else {
                    self.revoke_all("Codex 需要客户端处理，自动续跑已关闭", now);
                }
                return;
            }
            match method {
                "turn/started" | "turn/completed" => {
                    if let Some(id) = thread {
                        let observed = params.get("turn").and_then(parse_turn);
                        if let Some(turn) = observed {
                            if (method == "turn/started" && turn.status != TurnStatus::InProgress)
                                || (method == "turn/completed"
                                    && turn.status == TurnStatus::InProgress)
                            {
                                self.revoke_thread(id, "轮次事件与状态不一致，自动续跑已关闭", now);
                            } else {
                                self.observe(id, turn, now);
                            }
                        } else {
                            self.revoke_thread(id, "轮次事件无法识别，自动续跑已关闭", now);
                        }
                    } else {
                        self.incompatible("轮次事件缺少会话标识，自动续跑已关闭", now);
                    }
                }
                "thread/status/changed" => {
                    if let Some(id) = thread {
                        match params.pointer("/status/type").and_then(Value::as_str) {
                            Some("idle") => {
                                if let Some(state) = self.threads.get_mut(id) {
                                    state.awaiting_user = false;
                                }
                            }
                            Some("active") => {
                                let flags = params
                                    .pointer("/status/activeFlags")
                                    .and_then(Value::as_array);
                                if !flags.is_some_and(|flags| flags.is_empty()) {
                                    self.revoke_thread(
                                        id,
                                        "Codex 正在等待处理，自动续跑已关闭",
                                        now,
                                    );
                                    if let Some(state) = self.threads.get_mut(id) {
                                        state.awaiting_user = true;
                                    }
                                }
                            }
                            _ => self.revoke_thread(id, "无法确认实时状态，自动续跑已关闭", now),
                        }
                    }
                }
                "account/updated" | "config/updated" | "remoteControl/status/changed" => {
                    self.telemetry = None;
                    self.revoke_all("账号或执行连接已变化，自动续跑已关闭", now);
                }
                "thread/settings/updated"
                | "thread/goal/updated"
                | "thread/goal/cleared"
                | "thread/closed"
                | "thread/archived"
                | "thread/deleted"
                | "thread/environment/connected"
                | "thread/environment/disconnected"
                | "thread/queue/updated"
                | "model/rerouted" => {
                    if let Some(id) = thread {
                        self.revoke_thread(id, "会话执行上下文已变化，自动续跑已关闭", now);
                    } else {
                        self.revoke_all("执行上下文已变化，自动续跑已关闭", now);
                    }
                }
                "serverRequest/resolved" => {
                    if let Some(id) = params.get("requestId") {
                        self.native_work.remove(&id.to_string());
                    }
                }
                "account/rateLimits/updated" => {
                    // Sparse notifications only bring forward a complete query.
                    for state in self.threads.values_mut() {
                        state.next_check = state.next_check.min(now.saturating_add(2));
                    }
                }
                _ if known_notification(method) => {}
                _ => self.incompatible("Codex 返回了未验证的事件，自动续跑已关闭", now),
            }
            return;
        }
        let Some(request) = original_request else {
            return;
        };
        let method = request
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if method == "account/rateLimits/read" {
            self.telemetry = message.get("result").and_then(|result| {
                quota::parse_telemetry(result, now).map(|mut telemetry| {
                    telemetry.source = "desktop".into();
                    telemetry
                })
            });
        }
        if matches!(method, "thread/start" | "thread/resume" | "thread/fork") {
            if let Some(result) = message.get("result") {
                self.loaded(result, now);
            }
        } else if matches!(method, "turn/start" | "turn/steer") && message.get("error").is_some() {
            if let Some(id) = request.pointer("/params/threadId").and_then(Value::as_str) {
                self.revoke_thread(id, "用户提交未确认，自动续跑已关闭", now);
            }
        }
    }

    pub fn set_enabled(&mut self, thread: &str, enabled: bool, now: i64) -> Result<(), String> {
        if !enabled {
            if !self.threads.contains_key(thread) {
                return Err("会话未连接到当前 Desktop".into());
            }
            self.revoke_thread(thread, "自动续跑已关闭", now);
            return self.storage_result();
        }
        if !self.ready
            || !self.compatible
            || !self.storage_ok
            || self.capability != Capability::Supported
        {
            return Err("当前 Desktop 尚未确认条件续跑支持".into());
        }
        let state = self
            .threads
            .get_mut(thread)
            .ok_or("会话未连接到当前 Desktop")?;
        if state.awaiting_user {
            return Err("请先在原聊天中处理审批或输入请求".into());
        }
        state.revision = state.revision.wrapping_add(1);
        state.task.enabled = true;
        state.task.needs_reconciliation = false;
        state.task.resets_at = None;
        state.task.failed_turn_id = state.candidate.clone();
        state.next_check = now;
        if state.candidate.is_some() {
            state.task.set_status(
                TaskState::WaitingQuota,
                "已开启原聊天自动续跑，等待核对额度",
                now,
            );
        } else {
            state
                .task
                .set_status(TaskState::Monitoring, "已开启原聊天自动续跑", now);
        }
        self.persist(thread, now);
        self.storage_result()
    }

    pub fn set_paused(&mut self, paused: bool, now: i64) -> Result<(), String> {
        self.store.set_paused(paused).map_err(|e| e.to_string())?;
        self.paused = paused;
        if paused {
            self.revoke_all("全部自动续跑已暂停", now);
        }
        self.storage_result()
    }

    pub fn snapshot(&self) -> Vec<TaskView> {
        self.threads
            .values()
            .map(|state| {
                let mut view = TaskView::from(&state.task);
                view.mode = "desktop".into();
                view.can_enable = self.ready
                    && self.compatible
                    && self.storage_ok
                    && self.capability == Capability::Supported
                    && !state.awaiting_user;
                view
            })
            .collect()
    }

    pub fn telemetry(&self) -> Option<quota::QuotaTelemetry> {
        self.telemetry.clone()
    }

    pub fn control_lost(&mut self, now: i64) {
        // Losing the utility's heartbeat revokes consent, not the still-live
        // original Desktop connection. Reconnecting requires an explicit opt-in.
        self.revoke_all("AutoResume 连接已中断，自动续跑已关闭", now);
    }

    /// The transport may call this only before attempting any write. A write
    /// error, even one without an acknowledgement, is never proof of no send.
    pub fn not_dispatched(&mut self, request_id: u64, now: i64) {
        if !self.pending.as_ref().is_some_and(|pending| {
            pending.id == request_id && matches!(pending.purpose, Purpose::Continue(_))
        }) {
            return;
        }
        let Some(Pending {
            purpose: Purpose::Continue(attempt),
            ..
        }) = self.pending.take()
        else {
            return;
        };
        self.outgoing = None;
        self.automatic_thread = None;
        self.finish_not_submitted(&attempt, "续跑尚未发送，自动续跑已关闭", now);
    }

    pub fn disconnected(&mut self, now: i64) {
        self.telemetry = None;
        if let Some(request_id) = self.outgoing.as_ref().map(|request| request.id) {
            self.not_dispatched(request_id, now);
        }
        if let Some(Pending {
            purpose: Purpose::Continue(attempt),
            ..
        }) = self.pending.take()
        {
            let _ = self
                .store
                .mark_ambiguous(&attempt.thread, &attempt.failed, now);
        }
        self.outgoing = None;
        self.automatic_thread = None;
        self.native_work.clear();
        self.ready = false;
        self.compatible = false;
        self.revoke_all("原 Desktop 连接已结束，自动续跑已关闭", now);
    }

    pub fn tick(&mut self, now: i64) -> Vec<GuardRequest> {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| now >= pending.deadline)
        {
            if let Some(request_id) = self.outgoing.as_ref().map(|request| request.id) {
                self.not_dispatched(request_id, now);
            }
        }
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| now >= pending.deadline)
        {
            let pending = self.pending.take().unwrap();
            self.outgoing = None;
            match pending.purpose {
                Purpose::Capabilities => {
                    self.capability = Capability::Unsupported;
                    self.revoke_all("原 Desktop 未确认条件续跑支持", now);
                }
                Purpose::Continue(attempt) => {
                    self.automatic_thread = None;
                    let _ = self
                        .store
                        .mark_ambiguous(&attempt.thread, &attempt.failed, now);
                    self.revoke_thread(&attempt.thread, "续跑提交结果不明，未自动重试", now);
                }
                Purpose::Account(attempt) | Purpose::Quota(attempt) => {
                    if self.current(&attempt) {
                        if let Some(state) = self.threads.get_mut(&attempt.thread) {
                            state.next_check = now.saturating_add(RETRY_SECONDS);
                        }
                    }
                }
            }
        }
        if let Some(request) = self.outgoing.take() {
            let current = match self.pending.as_ref().map(|pending| &pending.purpose) {
                Some(
                    Purpose::Account(attempt)
                    | Purpose::Quota(attempt)
                    | Purpose::Continue(attempt),
                ) => self.current(attempt),
                Some(Purpose::Capabilities) => self.ready && self.compatible && self.storage_ok,
                None => false,
            };
            if current {
                return vec![request];
            }
            if matches!(
                self.pending.as_ref().map(|pending| &pending.purpose),
                Some(Purpose::Continue(_))
            ) {
                self.not_dispatched(request.id, now);
            } else {
                self.pending = None;
            }
        }
        if self.pending.is_some() || !self.ready || !self.compatible || !self.storage_ok {
            return vec![];
        }
        if self.capability == Capability::Unchecked {
            return vec![self.request(
                "autoresume/capabilities",
                json!({}),
                Purpose::Capabilities,
                now,
            )];
        }
        if self.capability != Capability::Supported
            || self.paused
            || self.automatic_thread.is_some()
        {
            return vec![];
        }
        let attempt = self.threads.iter().find_map(|(id, state)| {
            (state.task.enabled && !state.awaiting_user && now >= state.next_check)
                .then(|| {
                    state.candidate.as_ref().map(|failed| Attempt {
                        thread: id.clone(),
                        failed: failed.clone(),
                        revision: state.revision,
                    })
                })
                .flatten()
                .filter(|attempt| self.current(attempt))
        });
        if let Some(attempt) = attempt {
            return vec![self.request(
                "account/read",
                json!({"refreshToken":false}),
                Purpose::Account(attempt),
                now,
            )];
        }
        vec![]
    }

    pub fn reply(&mut self, id: u64, response: &Value, now: i64) {
        // Late timed-out reads and other bridge/native responses have no authority.
        if !self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.id == id)
        {
            return;
        }
        let pending = self.pending.take().unwrap();
        self.outgoing = None;
        if now >= pending.deadline {
            match pending.purpose {
                Purpose::Continue(attempt) => {
                    self.automatic_thread = None;
                    let _ = self
                        .store
                        .mark_ambiguous(&attempt.thread, &attempt.failed, now);
                    self.revoke_thread(&attempt.thread, "续跑提交确认超时，未自动重试", now);
                }
                Purpose::Capabilities => self.capability = Capability::Unsupported,
                Purpose::Account(attempt) | Purpose::Quota(attempt) => {
                    if let Some(state) = self.threads.get_mut(&attempt.thread) {
                        state.next_check = now.saturating_add(RETRY_SECONDS);
                    }
                }
            }
            return;
        }
        let result = response
            .get("result")
            .filter(|_| response.get("error").is_none());
        match pending.purpose {
            Purpose::Capabilities => {
                self.capability = if result.is_some_and(|r| {
                    r.as_object().is_some_and(|o| o.len() == 2)
                        && r.get("version").and_then(Value::as_u64) == Some(1)
                        && r.get("conditionalContinuation").and_then(Value::as_bool) == Some(true)
                }) {
                    Capability::Supported
                } else {
                    Capability::Unsupported
                };
            }
            Purpose::Account(attempt) => {
                if !self.current(&attempt) {
                    return;
                }
                if result
                    .and_then(|r| r.pointer("/account/type"))
                    .and_then(Value::as_str)
                    != Some("chatgpt")
                {
                    self.telemetry = None;
                    self.revoke_thread(&attempt.thread, "无法确认原账号的 ChatGPT 登录状态", now);
                    return;
                }
                self.outgoing = Some(self.request(
                    "account/rateLimits/read",
                    json!({}),
                    Purpose::Quota(attempt),
                    now,
                ));
            }
            Purpose::Quota(attempt) => {
                if !self.current(&attempt) {
                    return;
                }
                let Some(result) = result else {
                    if let Some(state) = self.threads.get_mut(&attempt.thread) {
                        state.next_check = now.saturating_add(RETRY_SECONDS);
                    }
                    return;
                };
                self.telemetry = quota::parse_telemetry(result, now).map(|mut telemetry| {
                    telemetry.source = "desktop".into();
                    telemetry
                });
                let Ok((snapshot, ordinary_usage_allowed)) =
                    quota::parse_desktop_response(result, now)
                else {
                    self.attention_thread(
                        &attempt.thread,
                        "完整额度信息无法识别，自动续跑已关闭",
                        now,
                    );
                    return;
                };
                if ordinary_usage_allowed.is_none() {
                    self.attention_thread(
                        &attempt.thread,
                        "服务端未确认常规额度可用，自动续跑已关闭",
                        now,
                    );
                    return;
                }
                let state = self.threads.get_mut(&attempt.thread).unwrap();
                state.task.required_limit_ids = snapshot
                    .buckets
                    .iter()
                    .map(|bucket| bucket.id.clone())
                    .collect();
                let mut decision = scheduler::evaluate(
                    &state.task,
                    &snapshot,
                    now,
                    &SafetyGate {
                        // This capability means the server makes the actual final
                        // idle/current-failed-turn reservation atomically.
                        ownership: Ownership::ExclusiveStopped,
                        execution_configuration_preserved: true,
                    },
                );
                if ordinary_usage_allowed != Some(true)
                    && matches!(decision, Decision::Submit { .. })
                {
                    decision = Decision::WaitUntil(None);
                }
                scheduler::apply_decision(&mut state.task, &decision, now);
                state.next_check = match decision {
                    Decision::WaitUntil(Some(reset)) => reset.saturating_add(2),
                    _ => now.saturating_add(RETRY_SECONDS),
                };
                self.persist(&attempt.thread, now);
                if !matches!(decision, Decision::Submit { .. }) || !self.current(&attempt) {
                    return;
                }
                match self
                    .store
                    .claim_submission(&attempt.thread, &attempt.failed, now)
                {
                    Ok(ClaimResult::Claimed(_)) => {
                        self.automatic_thread = Some(attempt.thread.clone());
                        if let Ok(Some(task)) = self.store.get_task(&attempt.thread) {
                            self.threads.get_mut(&attempt.thread).unwrap().task = task;
                        } else {
                            self.revoke_thread(&attempt.thread, "无法核对已保存的续跑意图", now);
                            return;
                        }
                        self.outgoing = Some(self.request(
                            "turn/continueIfFailed",
                            json!({
                                "threadId":attempt.thread,"expectedPreviousTurnId":attempt.failed,
                            }),
                            Purpose::Continue(attempt),
                            now,
                        ));
                    }
                    Ok(ClaimResult::Existing(_)) => {
                        self.revoke_thread(&attempt.thread, "此轮已有续跑记录，未重复提交", now)
                    }
                    Err(_) => {
                        self.revoke_thread(&attempt.thread, "无法保存或确认续跑意图，未发送", now)
                    }
                }
            }
            Purpose::Continue(attempt) => self.finish_continue(attempt, result, now),
        }
    }

    fn finish_continue(&mut self, attempt: Attempt, result: Option<&Value>, now: i64) {
        let turn_id = result
            .filter(|r| {
                r.get("status").and_then(Value::as_str) == Some("started")
                    && r.as_object().is_some_and(|object| object.len() == 2)
            })
            .and_then(|r| r.get("turnId"))
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty() && *id != attempt.failed);
        let Some(turn_id) = turn_id else {
            self.automatic_thread = None;
            // A rejected request is known not to have run, but its tombstone
            // still consumes the attempt. No future queue/retry is created.
            let rejected = result.is_some_and(strict_rejection);
            if rejected {
                self.finish_not_submitted(&attempt, "原会话已变化，服务端拒绝续跑；未重试", now);
                return;
            }
            let _ = self
                .store
                .mark_ambiguous(&attempt.thread, &attempt.failed, now);
            self.revoke_thread(&attempt.thread, "续跑提交结果不明，未自动重试", now);
            return;
        };
        if self
            .store
            .mark_submitted(&attempt.thread, &attempt.failed, turn_id, now)
            .is_err()
        {
            self.revoke_thread(&attempt.thread, "无法保存续跑确认，已停止自动操作", now);
            return;
        }
        let Some(state) = self.threads.get(&attempt.thread) else {
            return;
        };
        let observation = match state.latest.as_ref() {
            Some(turn) if turn.id == turn_id => turn.clone(),
            Some(turn) if turn.id != attempt.failed => {
                self.revoke_thread(&attempt.thread, "会话已有更新轮次，已停止自动操作", now);
                return;
            }
            _ => TurnObservation {
                id: turn_id.into(),
                status: TurnStatus::InProgress,
                error_code: None,
            },
        };
        match self
            .store
            .reconcile_observation(&attempt.thread, &observation, now)
        {
            Ok(task) => {
                if let Some(state) = self.threads.get_mut(&attempt.thread) {
                    state.task = task;
                    state.latest = Some(observation.clone());
                    state.candidate = quota_failure(&observation).then(|| observation.id.clone());
                    state.next_check = now.saturating_add(2);
                }
            }
            Err(_) => self.revoke_thread(&attempt.thread, "无法核对续跑结果，已停止自动操作", now),
        }
    }

    fn loaded(&mut self, result: &Value, now: i64) {
        let Some(thread) = result.get("thread") else {
            return;
        };
        let Some(id) = thread
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        else {
            return;
        };
        let cwd = thread
            .get("cwd")
            .or_else(|| result.get("cwd"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let title = thread
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| {
                Path::new(cwd)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("Codex 聊天")
                    .to_owned()
            });
        // Do not copy preview/prompt, turns, credentials, instructions or config.
        if self.threads.contains_key(id) {
            self.revoke_thread(id, "原聊天已重新加载，请重新开启自动续跑", now);
            return;
        }
        let mut task = TaskRecord::new(id.into(), title, cwd.into());
        task.needs_reconciliation = false;
        task.set_status(
            TaskState::Paused,
            "原 Desktop 聊天已连接；自动续跑未开启",
            now,
        );
        self.threads.insert(
            id.into(),
            DesktopThread {
                task,
                revision: 0,
                candidate: None,
                latest: None,
                next_check: now,
                awaiting_user: false,
            },
        );
        self.persist(id, now);
    }

    fn finish_not_submitted(&mut self, attempt: &Attempt, reason: &str, now: i64) {
        let finalized = self
            .store
            .mark_not_submitted(&attempt.thread, &attempt.failed, now)
            .is_ok();
        self.revoke_thread(
            &attempt.thread,
            if finalized {
                reason
            } else {
                "无法保存未发送结果，已停止自动操作"
            },
            now,
        );
    }

    fn observe(&mut self, id: &str, observation: TurnObservation, now: i64) {
        let Some(state) = self.threads.get_mut(id) else {
            return;
        };
        // Duplicate terminal notifications must not advance the revision or
        // undo a user disable/safety block. In-progress ACKs never undo terminal.
        if let Some(previous) = &state.latest {
            if previous == &observation
                || (previous.id == observation.id
                    && previous.status != TurnStatus::InProgress
                    && observation.status == TurnStatus::InProgress)
            {
                return;
            }
        }
        if observation.status != TurnStatus::InProgress
            && self.automatic_thread.as_deref() == Some(id)
        {
            self.automatic_thread = None;
        }
        state.revision = state.revision.wrapping_add(1);
        state.candidate = quota_failure(&observation).then(|| observation.id.clone());
        state.latest = Some(observation.clone());
        state.next_check = now.saturating_add(2);
        scheduler::observe_turn(&mut state.task, &observation, now);
        if observation.status == TurnStatus::Completed {
            state
                .task
                .set_status(TaskState::Completed, "本轮已结束", now);
        } else if observation.status == TurnStatus::InProgress && !state.task.enabled {
            state
                .task
                .set_status(TaskState::Running, "原聊天正在执行；自动续跑已关闭", now);
        } else if quota_failure(&observation) && !state.task.enabled {
            state.task.set_status(
                TaskState::Paused,
                "原聊天因额度停止；可手动开启自动续跑",
                now,
            );
        }
        self.persist(id, now);
    }

    fn current(&self, attempt: &Attempt) -> bool {
        self.ready
            && self.compatible
            && self.storage_ok
            && !self.paused
            && !self.store.is_paused().unwrap_or(true)
            && self.capability == Capability::Supported
            && !self.native_work.values().any(|thread| {
                thread
                    .as_deref()
                    .is_none_or(|thread| thread == attempt.thread)
            })
            && self.threads.get(&attempt.thread).is_some_and(|state| {
                state.task.enabled
                    && !state.awaiting_user
                    && state.revision == attempt.revision
                    && state.candidate.as_deref() == Some(attempt.failed.as_str())
            })
    }

    fn request(&mut self, method: &str, params: Value, purpose: Purpose, now: i64) -> GuardRequest {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.pending = Some(Pending {
            id,
            deadline: now.saturating_add(REQUEST_TIMEOUT_SECONDS),
            purpose,
        });
        GuardRequest {
            id,
            method: method.into(),
            params,
        }
    }

    fn revoke_thread(&mut self, id: &str, reason: &str, now: i64) {
        if let Some(state) = self.threads.get_mut(id) {
            state.revision = state.revision.wrapping_add(1);
            state.candidate = None;
            state.task.enabled = false;
            state.task.failed_turn_id = None;
            state.task.resets_at = None;
            state.task.set_status(TaskState::Paused, reason, now);
            self.persist(id, now);
        }
    }

    fn attention_thread(&mut self, id: &str, reason: &str, now: i64) {
        self.revoke_thread(id, reason, now);
        if let Some(state) = self.threads.get_mut(id) {
            state.task.needs_reconciliation = true;
            state
                .task
                .set_status(TaskState::NeedsAttention, reason, now);
        }
        self.persist(id, now);
    }

    fn revoke_all(&mut self, reason: &str, now: i64) {
        let ids: Vec<_> = self.threads.keys().cloned().collect();
        for id in ids {
            self.revoke_thread(&id, reason, now);
        }
    }

    fn incompatible(&mut self, reason: &str, now: i64) {
        self.compatible = false;
        self.revoke_all(reason, now);
    }

    fn persist(&mut self, id: &str, now: i64) {
        if let Some(state) = self.threads.get(id) {
            if self.store.save_task(&state.task).is_err() {
                self.storage_ok = false;
                for state in self.threads.values_mut() {
                    state.task.enabled = false;
                    state.candidate = None;
                    state.task.set_status(
                        TaskState::NeedsAttention,
                        "无法保存本地状态，自动续跑已关闭",
                        now,
                    );
                }
            }
        }
    }

    fn storage_result(&self) -> Result<(), String> {
        if self.storage_ok {
            Ok(())
        } else {
            Err("无法保存自动续跑设置".into())
        }
    }
}

fn quota_failure(turn: &TurnObservation) -> bool {
    turn.status == TurnStatus::Failed && turn.error_code.as_deref() == Some("usageLimitExceeded")
}

fn strict_rejection(value: &Value) -> bool {
    value.as_object().is_some_and(|object| object.len() == 2)
        && value.get("status").and_then(Value::as_str) == Some("rejected")
        && matches!(
            value.get("reason").and_then(Value::as_str),
            Some(
                "noMatchingQuotaFailure"
                    | "configurationUnverified"
                    | "notLoaded"
                    | "notSubscribed"
                    | "liveGenerationChanged"
                    | "notIdle"
                    | "superseded"
                    | "pendingTriggerTurn"
                    | "serverDraining"
                    | "guardRejected"
            )
        )
}

fn parse_turn(value: &Value) -> Option<TurnObservation> {
    const KNOWN: &[&str] = &[
        "id",
        "items",
        "itemsView",
        "status",
        "error",
        "startedAt",
        "completedAt",
        "durationMs",
        "rootTurnId",
    ];
    if value
        .as_object()?
        .keys()
        .any(|key| !KNOWN.contains(&key.as_str()))
    {
        return None;
    }
    let id = value.get("id")?.as_str()?.to_owned();
    if id.is_empty() {
        return None;
    }
    let status = match value.get("status")?.as_str()? {
        "inProgress" => TurnStatus::InProgress,
        "completed" => TurnStatus::Completed,
        "interrupted" => TurnStatus::Interrupted,
        "failed" => TurnStatus::Failed,
        _ => return None,
    };
    Some(TurnObservation {
        id,
        status,
        error_code: value
            .pointer("/error/codexErrorInfo")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

fn known_native_read(method: &str) -> bool {
    matches!(
        method,
        "thread/read"
            | "thread/list"
            | "thread/loaded/list"
            | "thread/turns/list"
            | "thread/items/list"
            | "thread/goal/get"
            | "thread/queue/list"
            | "account/read"
            | "account/rateLimits/read"
            | "model/list"
            | "modelProvider/capabilities/read"
            | "config/read"
            | "configRequirements/read"
            | "permissionProfile/list"
            | "collaborationMode/list"
            | "skills/list"
            | "hooks/list"
            | "mcpServerStatus/list"
            | "app/list"
            | "app/read"
            | "plugin/list"
            | "plugin/read"
            | "experimentalFeature/list"
            | "externalAgentConfig/detect"
            | "fs/readFile"
            | "fs/getMetadata"
            | "fs/readDirectory"
            | "thread/name/set"
            | "fuzzyFileSearch"
            | "fuzzyFileSearch/sessionStart"
            | "fuzzyFileSearch/sessionUpdate"
            | "fuzzyFileSearch/sessionStop"
            | "getAuthStatus"
            | "getUserAgent"
            | "getUserSavedConfig"
            | "skills/remote/list"
            | "plugin/installed"
            | "plugin/skill/read"
            | "plugin/share/list"
            | "app/installed"
            | "fs/watch"
            | "fs/unwatch"
            | "mcpServer/resource/read"
            | "windowsSandbox/readiness"
            | "account/usage/read"
            | "account/workspaceMessages/read"
            | "externalAgentConfig/import/readHistories"
            | "getConversationSummary"
            | "gitDiffToRemote"
            | "project/list"
            | "project/read"
            | "threadSection/list"
            | "thread/attachment/list"
            | "thread/attachmentOwner/list"
    )
}

fn known_notification(method: &str) -> bool {
    matches!(
        method,
        "error"
            | "thread/started"
            | "thread/unarchived"
            | "skills/changed"
            | "thread/name/updated"
            | "thread/tokenUsage/updated"
            | "hook/started"
            | "hook/completed"
            | "turn/diff/updated"
            | "turn/plan/updated"
            | "item/started"
            | "item/completed"
            | "item/autoApprovalReview/started"
            | "item/autoApprovalReview/completed"
            | "rawResponseItem/completed"
            | "rawResponse/completed"
            | "item/agentMessage/delta"
            | "item/plan/delta"
            | "command/exec/outputDelta"
            | "process/outputDelta"
            | "process/exited"
            | "item/commandExecution/outputDelta"
            | "item/commandExecution/terminalInteraction"
            | "item/fileChange/outputDelta"
            | "item/fileChange/patchUpdated"
            | "serverRequest/resolved"
            | "item/mcpToolCall/progress"
            | "mcpServer/oauthLogin/completed"
            | "mcpServer/startupStatus/updated"
            | "app/list/updated"
            | "externalAgentConfig/import/progress"
            | "externalAgentConfig/import/completed"
            | "fs/changed"
            | "item/reasoning/summaryTextDelta"
            | "item/reasoning/summaryPartAdded"
            | "item/reasoning/textDelta"
            | "thread/compacted"
            | "model/verification"
            | "turn/moderationMetadata"
            | "model/safetyBuffering/updated"
            | "warning"
            | "guardianWarning"
            | "deprecationNotice"
            | "configWarning"
            | "fuzzyFileSearch/sessionUpdated"
            | "fuzzyFileSearch/sessionCompleted"
            | "windows/worldWritableWarning"
            | "windowsSandbox/setupCompleted"
            | "account/login/completed"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::IntentStatus;
    use tempfile::TempDir;

    fn setup() -> (TempDir, DesktopGuard) {
        let dir = tempfile::tempdir().unwrap();
        let mut guard = DesktopGuard::open(&dir.path().join("desktop.sqlite")).unwrap();
        guard.native_client(&json!({"method":"initialized","params":{}}), 100);
        let request = one(guard.tick(100), "autoresume/capabilities");
        guard.reply(
            request.id,
            &json!({"result":{"version":1,"conditionalContinuation":true}}),
            100,
        );
        loaded(&mut guard, "thread");
        (dir, guard)
    }

    fn loaded(guard: &mut DesktopGuard, id: &str) {
        guard.native_server(&json!({"id":1,"result":{"thread":{
            "id":id,"name":"Example task","cwd":"/project","preview":"private prompt omitted",
            "turns":[{"id":"historical","status":"failed","error":{"codexErrorInfo":"usageLimitExceeded"}}]
        }}}), Some(&json!({"id":1,"method":"thread/resume","params":{"threadId":id}})), 100);
    }

    fn event(
        guard: &mut DesktopGuard,
        thread: &str,
        id: &str,
        status: &str,
        code: Option<&str>,
        now: i64,
    ) {
        guard.native_server(&json!({"method":if status=="inProgress" {"turn/started"} else {"turn/completed"},"params":{
            "threadId":thread,"turn":{"id":id,"status":status,"error":code.map(|value|json!({"codexErrorInfo":value}))}
        }}), None, now);
    }

    fn quota(used: i64, reset: Option<i64>) -> Value {
        let bucket = json!({"limitId":"codex","limitName":"Codex",
            "primary":{"usedPercent":used,"windowDurationMins":300,"resetsAt":reset},
            "secondary":null,"credits":null,"individualLimit":null,"spendControlReached":false,
            "planType":"pro","rateLimitReachedType":null,"normalModelSlug":null});
        json!({"rateLimits":bucket,"rateLimitsByLimitId":{"codex":bucket},"rateLimitResetCredits":null,
            "ordinaryUsageAllowed":true,"accountId":null,"rateLimitUpsell":null})
    }

    fn one(requests: Vec<GuardRequest>, method: &str) -> GuardRequest {
        assert_eq!(requests.len(), 1, "expected {method}: {requests:?}");
        let request = requests.into_iter().next().unwrap();
        assert_eq!(request.method, method);
        request
    }

    fn enable_failure(guard: &mut DesktopGuard) {
        event(
            guard,
            "thread",
            "failed",
            "failed",
            Some("usageLimitExceeded"),
            100,
        );
        guard.set_enabled("thread", true, 101).unwrap();
    }

    fn ready_quota_request(guard: &mut DesktopGuard, now: i64) -> GuardRequest {
        let request = one(guard.tick(now), "account/read");
        guard.reply(
            request.id,
            &json!({"result":{"account":{"type":"chatgpt"}}}),
            now,
        );
        one(guard.tick(now), "account/rateLimits/read")
    }

    fn ready_continue(guard: &mut DesktopGuard, now: i64) -> GuardRequest {
        let request = ready_quota_request(guard, now);
        guard.reply(request.id, &json!({"result":quota(0,None)}), now);
        one(guard.tick(now), "turn/continueIfFailed")
    }

    #[test]
    fn historical_failure_and_legacy_selection_are_not_execution_consent() {
        let (_dir, mut guard) = setup();
        assert!(!guard.snapshot()[0].enabled);
        assert!(guard.tick(200).is_empty());
        guard.set_enabled("thread", true, 200).unwrap();
        assert!(
            guard.tick(200).is_empty(),
            "thread/resume history is not live failure evidence"
        );
        assert!(guard.set_enabled("not-loaded", true, 200).is_err());
    }

    #[test]
    fn existing_live_failure_can_be_enabled_and_claim_is_durable_before_send() {
        let (dir, mut guard) = setup();
        enable_failure(&mut guard);
        let request = ready_continue(&mut guard, 101);
        assert_eq!(
            request.params,
            json!({"threadId":"thread","expectedPreviousTurnId":"failed"})
        );
        let second_handle = Store::open(dir.path().join("desktop.sqlite")).unwrap();
        assert_eq!(
            second_handle
                .get_intent("thread", "failed")
                .unwrap()
                .unwrap()
                .status,
            IntentStatus::Claimed
        );
        guard.reply(
            request.id,
            &json!({"result":{"status":"started","turnId":"continued"}}),
            102,
        );
        event(&mut guard, "thread", "continued", "completed", None, 103);
        assert_eq!(guard.snapshot()[0].state, TaskState::Completed);
        assert!(guard.tick(1000).is_empty());
    }

    #[test]
    fn native_input_invalidates_account_or_quota_reply() {
        for stage in ["account", "quota"] {
            let (_dir, mut guard) = setup();
            enable_failure(&mut guard);
            let request = if stage == "quota" {
                ready_quota_request(&mut guard, 101)
            } else {
                one(guard.tick(101), "account/read")
            };
            guard.native_client(
                &json!({"id":2,"method":"turn/start","params":{"threadId":"thread","input":[]}}),
                102,
            );
            guard.reply(request.id,&json!({"result":if stage=="quota" {quota(0,None)} else {json!({"account":{"type":"chatgpt"}})}}),103);
            assert!(guard.tick(103).is_empty());
            assert!(guard
                .store
                .get_intent("thread", "failed")
                .unwrap()
                .is_none());
            assert!(!guard.snapshot()[0].enabled);
        }
    }

    #[test]
    fn user_stop_after_claim_before_write_cancels_pending_dispatch() {
        let (_dir, mut guard) = setup();
        enable_failure(&mut guard);
        let request = ready_quota_request(&mut guard, 101);
        guard.reply(request.id, &json!({"result":quota(0,None)}), 101);
        assert!(guard
            .store
            .get_intent("thread", "failed")
            .unwrap()
            .is_some());
        guard.native_client(
            &json!({"id":3,"method":"turn/interrupt","params":{"threadId":"thread"}}),
            101,
        );
        assert!(guard.tick(101).is_empty());
        assert_eq!(
            guard
                .store
                .get_intent("thread", "failed")
                .unwrap()
                .unwrap()
                .status,
            IntentStatus::Reconciled
        );
    }

    #[test]
    fn timeout_disconnect_and_reject_consume_attempt_across_restart() {
        for outcome in ["timeout", "disconnect", "rejected", "error"] {
            let (dir, mut guard) = setup();
            enable_failure(&mut guard);
            let request = ready_continue(&mut guard, 101);
            match outcome {
                "timeout" => {
                    guard.tick(132);
                }
                "disconnect" => guard.disconnected(102),
                "rejected" => guard.reply(
                    request.id,
                    &json!({"result":{"status":"rejected","reason":"superseded"}}),
                    102,
                ),
                _ => guard.reply(
                    request.id,
                    &json!({"error":{"code":-1,"message":"unavailable"}}),
                    102,
                ),
            }
            assert_eq!(
                guard
                    .store
                    .get_intent("thread", "failed")
                    .unwrap()
                    .unwrap()
                    .status,
                if outcome == "rejected" {
                    IntentStatus::Reconciled
                } else {
                    IntentStatus::Ambiguous
                }
            );
            drop(guard);
            let mut restarted = DesktopGuard::open(&dir.path().join("desktop.sqlite")).unwrap();
            restarted.native_client(&json!({"method":"initialized"}), 200);
            let cap = one(restarted.tick(200), "autoresume/capabilities");
            restarted.reply(
                cap.id,
                &json!({"result":{"version":1,"conditionalContinuation":true}}),
                200,
            );
            loaded(&mut restarted, "thread");
            assert!(!restarted.snapshot()[0].enabled);
            enable_failure(&mut restarted);
            let quota_request = ready_quota_request(&mut restarted, 201);
            restarted.reply(quota_request.id, &json!({"result":quota(0,None)}), 201);
            assert!(
                restarted.tick(201).is_empty(),
                "existing attempt must never send again ({outcome})"
            );
        }
    }

    #[test]
    fn terminal_notification_before_ack_is_not_regressed() {
        let (_dir, mut guard) = setup();
        enable_failure(&mut guard);
        let request = ready_continue(&mut guard, 101);
        event(&mut guard, "thread", "continued", "inProgress", None, 102);
        event(&mut guard, "thread", "continued", "completed", None, 103);
        guard.reply(
            request.id,
            &json!({"result":{"status":"started","turnId":"continued"}}),
            104,
        );
        assert_eq!(guard.snapshot()[0].state, TaskState::Completed);
        assert_eq!(
            guard
                .store
                .get_intent("thread", "failed")
                .unwrap()
                .unwrap()
                .status,
            IntentStatus::Reconciled
        );
        event(&mut guard, "thread", "continued", "inProgress", None, 105);
        assert_eq!(guard.snapshot()[0].state, TaskState::Completed);
    }

    #[test]
    fn explicit_terminal_quota_code_is_required() {
        for (status, code) in [
            ("completed", Some("usageLimitExceeded")),
            ("inProgress", Some("usageLimitExceeded")),
            ("failed", Some("http429")),
            ("failed", Some("Quota usageLimitExceeded")),
            ("failed", None),
            ("interrupted", Some("usageLimitExceeded")),
        ] {
            let (_dir, mut guard) = setup();
            guard.set_enabled("thread", true, 101).unwrap();
            event(&mut guard, "thread", "failed", status, code, 102);
            assert!(guard.tick(200).is_empty(), "{status} {code:?}");
        }
    }

    #[test]
    fn no_capability_and_unknown_protocol_never_authorize() {
        let dir = tempfile::tempdir().unwrap();
        let mut guard = DesktopGuard::open(&dir.path().join("db")).unwrap();
        loaded(&mut guard, "thread");
        assert!(guard.tick(100).is_empty());
        assert!(guard.set_enabled("thread", true, 100).is_err());
        guard.native_client(&json!({"method":"initialized"}), 100);
        let request = one(guard.tick(100), "autoresume/capabilities");
        guard.reply(
            request.id,
            &json!({"result":{"version":1,"conditionalContinuation":true,"unverified":true}}),
            100,
        );
        assert!(guard.set_enabled("thread", true, 100).is_err());
        let (_dir, mut guard) = setup();
        enable_failure(&mut guard);
        guard.native_server(
            &json!({"method":"future/automaticWork","params":{}}),
            None,
            102,
        );
        assert!(!guard.snapshot()[0].enabled);
        assert!(!guard.snapshot()[0].can_enable);
        assert!(guard.tick(102).is_empty());
    }

    #[test]
    fn approvals_revoke_without_fabricating_a_response_and_thread_scoping_is_preserved() {
        let (_dir, mut guard) = setup();
        loaded(&mut guard, "other");
        enable_failure(&mut guard);
        guard.set_enabled("other", true, 101).unwrap();
        guard.native_server(&json!({"id":77,"method":"item/commandExecution/requestApproval","params":{"threadId":"thread"}}),None,102);
        assert!(!guard.threads["thread"].task.enabled);
        assert!(guard.threads["other"].task.enabled);
        assert!(guard.set_enabled("thread", true, 103).is_err());
        guard.native_client(&json!({"id":77,"result":{"decision":"accept"}}), 103);
        assert!(guard.tick(104).is_empty());
    }

    #[test]
    fn unknown_quota_rejects_and_reset_clock_does_not_authorize() {
        let (_dir, mut guard) = setup();
        enable_failure(&mut guard);
        let request = ready_quota_request(&mut guard, 101);
        guard.reply(request.id, &json!({"result":quota(100,Some(300))}), 101);
        assert_eq!(guard.snapshot()[0].resets_at, Some(300));
        assert!(guard.tick(301).is_empty());
        let request = ready_quota_request(&mut guard, 302);
        let mut unknown = quota(0, None);
        unknown["newField"] = json!(true);
        guard.reply(request.id, &json!({"result":unknown}), 302);
        assert!(guard.tick(303).is_empty());
        assert!(!guard.snapshot()[0].enabled);
        assert!(guard
            .store
            .get_intent("thread", "failed")
            .unwrap()
            .is_none());
    }

    #[test]
    fn late_read_response_cannot_revive_a_timed_out_check() {
        let (_dir, mut guard) = setup();
        enable_failure(&mut guard);
        let request = ready_quota_request(&mut guard, 101);
        assert!(guard.tick(132).is_empty());
        guard.reply(request.id, &json!({"result":quota(0,None)}), 133);
        assert!(guard.tick(134).is_empty());
        assert!(guard
            .store
            .get_intent("thread", "failed")
            .unwrap()
            .is_none());
    }

    #[test]
    fn global_pause_between_claim_and_write_consumes_attempt_without_sending() {
        let (_dir, mut guard) = setup();
        enable_failure(&mut guard);
        let request = ready_quota_request(&mut guard, 101);
        guard.reply(request.id, &json!({"result":quota(0,None)}), 101);
        guard.set_paused(true, 101).unwrap();
        assert!(guard.tick(101).is_empty());
        guard.set_paused(false, 102).unwrap();
        assert!(!guard.snapshot()[0].enabled);
        assert!(guard.tick(1000).is_empty());
    }
    #[test]
    fn automatic_tasks_are_serial_until_terminal_or_approval() {
        let (_dir, mut guard) = setup();
        loaded(&mut guard, "second");
        enable_failure(&mut guard);
        event(
            &mut guard,
            "second",
            "failed-second",
            "failed",
            Some("usageLimitExceeded"),
            100,
        );
        guard.set_enabled("second", true, 101).unwrap();
        let request = ready_continue(&mut guard, 101);
        let first_id = request.params["threadId"].as_str().unwrap().to_owned();
        guard.reply(
            request.id,
            &json!({"result":{"status":"started","turnId":"continued-first"}}),
            102,
        );
        assert!(
            guard.tick(200).is_empty(),
            "another automatic task must wait"
        );
        event(
            &mut guard,
            &first_id,
            "continued-first",
            "completed",
            None,
            201,
        );
        let account = one(guard.tick(201), "account/read");
        guard.reply(
            account.id,
            &json!({"result":{"account":{"type":"chatgpt"}}}),
            201,
        );
        let quota_request = one(guard.tick(201), "account/rateLimits/read");
        guard.reply(quota_request.id, &json!({"result":quota(0,None)}), 201);
        let second = one(guard.tick(201), "turn/continueIfFailed");
        assert_ne!(second.params["threadId"], first_id);
    }

    #[test]
    fn subsequent_real_quota_failure_is_a_distinct_attempt_even_before_ack() {
        let (_dir, mut guard) = setup();
        enable_failure(&mut guard);
        let first = ready_continue(&mut guard, 101);
        event(&mut guard, "thread", "continued", "inProgress", None, 102);
        event(
            &mut guard,
            "thread",
            "continued",
            "failed",
            Some("usageLimitExceeded"),
            103,
        );
        guard.reply(
            first.id,
            &json!({"result":{"status":"started","turnId":"continued"}}),
            104,
        );
        assert_eq!(guard.snapshot()[0].state, TaskState::WaitingQuota);
        let next = ready_continue(&mut guard, 106);
        assert_eq!(next.params["expectedPreviousTurnId"], "continued");
        assert_eq!(
            guard
                .store
                .get_intent("thread", "failed")
                .unwrap()
                .unwrap()
                .status,
            IntentStatus::Reconciled
        );
        assert_eq!(
            guard
                .store
                .get_intent("thread", "continued")
                .unwrap()
                .unwrap()
                .status,
            IntentStatus::Claimed
        );
    }
    #[test]
    fn native_desktop_tool_calls_preserve_consent_without_answering() {
        let (_dir, mut guard) = setup();
        guard.set_enabled("thread", true, 101).unwrap();
        event(&mut guard, "thread", "running", "inProgress", None, 102);
        guard.native_server(&json!({"id":"tool-request", "method":"item/tool/call", "params":{
            "threadId":"thread", "turnId":"running", "callId":"call", "tool":"native_tool", "arguments":{}
        }}), None, 103);
        assert!(guard.snapshot()[0].enabled);
        assert!(guard.tick(103).is_empty());
        guard.native_client(
            &json!({"id":"tool-request", "result":{"success":true,"contentItems":[]}}),
            104,
        );
        event(
            &mut guard,
            "thread",
            "running",
            "failed",
            Some("usageLimitExceeded"),
            105,
        );
        let request = ready_continue(&mut guard, 107);
        assert_eq!(request.params["expectedPreviousTurnId"], "running");
    }

    #[test]
    fn control_loss_revokes_consent_without_destroying_live_desktop_generation() {
        let (_dir, mut guard) = setup();
        enable_failure(&mut guard);
        let request = ready_quota_request(&mut guard, 101);
        guard.control_lost(102);
        guard.reply(request.id, &json!({"result":quota(0,None)}), 103);
        assert!(guard.tick(103).is_empty());
        assert!(!guard.snapshot()[0].enabled);
        assert!(guard.snapshot()[0].can_enable);
        guard.set_enabled("thread", true, 104).unwrap();
        assert!(
            guard.tick(104).is_empty(),
            "a new opt-in cannot resurrect the cleared candidate"
        );
        event(
            &mut guard,
            "thread",
            "new-failure",
            "failed",
            Some("usageLimitExceeded"),
            105,
        );
        ready_continue(&mut guard, 107);
    }
    #[test]
    fn shared_global_pause_blocks_a_claim_waiting_for_dispatch() {
        let (dir, mut guard) = setup();
        enable_failure(&mut guard);
        let request = ready_quota_request(&mut guard, 101);
        guard.reply(request.id, &json!({"result":quota(0,None)}), 101);
        let other_bridge = Store::open(dir.path().join("desktop.sqlite")).unwrap();
        other_bridge.set_paused(true).unwrap();
        assert!(guard.tick(101).is_empty());
        assert_eq!(
            guard
                .store
                .get_intent("thread", "failed")
                .unwrap()
                .unwrap()
                .status,
            IntentStatus::Reconciled
        );
    }
    #[test]
    fn desktop_metadata_reads_and_native_capabilities_preserve_consent() {
        let (_dir, mut guard) = setup();
        enable_failure(&mut guard);
        for method in [
            "project/list",
            "project/read",
            "threadSection/list",
            "thread/attachment/list",
            "thread/attachmentOwner/list",
        ] {
            guard.native_client(&json!({"id":1,"method":method,"params":{}}), 101);
            assert!(guard.snapshot()[0].enabled, "{method}");
        }
        for method in ["currentTime/read", "attestation/generate"] {
            guard.native_server(
                &json!({"id":"native-work","method":method,"params":{}}),
                None,
                101,
            );
            assert!(guard.snapshot()[0].enabled, "{method}");
            assert!(
                guard.tick(101).is_empty(),
                "native capability response is still pending"
            );
            guard.native_client(&json!({"id":"native-work","result":{}}), 101);
        }
        ready_continue(&mut guard, 101);
    }
    #[test]
    fn ordinary_usage_gate_is_required_even_with_zero_percent_used() {
        for allowed in [json!(false), Value::Null] {
            let (_dir, mut guard) = setup();
            enable_failure(&mut guard);
            let request = ready_quota_request(&mut guard, 101);
            let mut snapshot = quota(0, None);
            snapshot["ordinaryUsageAllowed"] = allowed.clone();
            guard.reply(request.id, &json!({"result":snapshot}), 101);
            assert!(guard.tick(101).is_empty());
            assert!(guard
                .store
                .get_intent("thread", "failed")
                .unwrap()
                .is_none());
            assert_eq!(
                guard.snapshot()[0].state,
                if allowed.is_null() {
                    TaskState::NeedsAttention
                } else {
                    TaskState::WaitingQuota
                }
            );
        }
    }
    fn new_failure_after_user_work(guard: &mut DesktopGuard) {
        guard.native_client(&json!({"id":"new-request","method":"turn/start","params":{"threadId":"thread","input":[]}}), 110);
        event(guard, "thread", "new-failure", "inProgress", None, 110);
        event(
            guard,
            "thread",
            "new-failure",
            "failed",
            Some("usageLimitExceeded"),
            111,
        );
        guard.set_enabled("thread", true, 112).unwrap();
    }

    #[test]
    fn strict_rejection_and_transport_no_write_allow_a_distinct_future_attempt() {
        for rejected in [true, false] {
            let (_dir, mut guard) = setup();
            enable_failure(&mut guard);
            let request = ready_continue(&mut guard, 101);
            if rejected {
                guard.reply(
                    request.id,
                    &json!({"result":{"status":"rejected","reason":"superseded"}}),
                    102,
                );
            } else {
                guard.not_dispatched(request.id, 102);
            }
            assert_eq!(
                guard
                    .store
                    .get_intent("thread", "failed")
                    .unwrap()
                    .unwrap()
                    .status,
                IntentStatus::Reconciled
            );
            assert!(matches!(
                guard
                    .store
                    .claim_submission("thread", "failed", 103)
                    .unwrap(),
                ClaimResult::Existing(_)
            ));
            new_failure_after_user_work(&mut guard);
            let future = ready_continue(&mut guard, 112);
            assert_eq!(future.params["expectedPreviousTurnId"], "new-failure");
        }
    }

    #[test]
    fn unrecognized_rejection_remains_ambiguous_and_blocks_future_attempts() {
        for result in [
            json!({"status":"rejected","reason":"futureReason"}),
            json!({"status":"rejected","reason":"superseded","unknown":true}),
        ] {
            let (_dir, mut guard) = setup();
            enable_failure(&mut guard);
            let request = ready_continue(&mut guard, 101);
            guard.reply(request.id, &json!({"result":result}), 102);
            assert_eq!(
                guard
                    .store
                    .get_intent("thread", "failed")
                    .unwrap()
                    .unwrap()
                    .status,
                IntentStatus::Ambiguous
            );
            new_failure_after_user_work(&mut guard);
            let quota_request = ready_quota_request(&mut guard, 112);
            guard.reply(quota_request.id, &json!({"result":quota(0,None)}), 112);
            assert!(guard.tick(112).is_empty());
            assert!(guard
                .store
                .get_intent("thread", "new-failure")
                .unwrap()
                .is_none());
        }
    }

    #[test]
    fn late_rejection_cannot_clear_an_uncertain_write() {
        let (_dir, mut guard) = setup();
        enable_failure(&mut guard);
        let request = ready_continue(&mut guard, 101);
        guard.tick(132);
        guard.reply(
            request.id,
            &json!({"result":{"status":"rejected","reason":"superseded"}}),
            133,
        );
        guard.not_dispatched(request.id, 134);
        assert_eq!(
            guard
                .store
                .get_intent("thread", "failed")
                .unwrap()
                .unwrap()
                .status,
            IntentStatus::Ambiguous
        );
    }

    #[test]
    fn desktop_telemetry_excludes_account_metadata_and_clears_on_account_change() {
        let (_dir, mut guard) = setup();
        let mut response = quota(100, Some(500));
        response["accountId"] = json!("synthetic-private-account");
        guard.native_server(
            &json!({"id":4,"result":response}),
            Some(&json!({"method":"account/rateLimits/read"})),
            101,
        );
        let telemetry = guard.telemetry().unwrap();
        assert_eq!(telemetry.source, "desktop");
        assert!(!serde_json::to_string(&telemetry)
            .unwrap()
            .contains("synthetic-private-account"));
        assert!(!guard.snapshot()[0].enabled);
        guard.native_server(&json!({"method":"account/updated","params":{}}), None, 102);
        assert!(guard.telemetry().is_none());
    }
}
