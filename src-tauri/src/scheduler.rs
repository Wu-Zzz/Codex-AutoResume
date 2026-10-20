use serde::{Deserialize, Serialize};

/// A quota observation is useful only briefly, regardless of its reset time.
pub const MAX_QUOTA_AGE_SECONDS: i64 = 60;
const MAX_EVENTS: usize = 40;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Paused,
    Monitoring,
    Running,
    WaitingQuota,
    Reconciling,
    NeedsAttention,
    Completed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskEvent {
    pub at: i64,
    pub message: String,
}

/// Local application metadata only. Never put credentials or conversation text here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRecord {
    pub id: String,
    pub title: String,
    pub cwd: String,
    pub enabled: bool,
    pub state: TaskState,
    pub detail: String,
    pub resets_at: Option<i64>,
    pub events: Vec<TaskEvent>,
    pub failed_turn_id: Option<String>,
    pub observed_turn_id: Option<String>,
    #[serde(default)]
    pub observed_turn_status: Option<TurnStatus>,
    #[serde(default)]
    pub observed_error_code: Option<String>,
    pub needs_reconciliation: bool,
    /// Must come from the verified runtime/model association, not an assumed bucket.
    pub required_limit_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskView {
    pub id: String,
    pub title: String,
    pub cwd: String,
    pub enabled: bool,
    pub state: TaskState,
    pub detail: String,
    pub resets_at: Option<i64>,
    pub events: Vec<TaskEvent>,
}

impl From<&TaskRecord> for TaskView {
    fn from(task: &TaskRecord) -> Self {
        Self {
            id: task.id.clone(),
            title: task.title.clone(),
            cwd: task.cwd.clone(),
            enabled: task.enabled,
            state: task.state.clone(),
            detail: task.detail.clone(),
            resets_at: task.resets_at,
            events: task.events.clone(),
        }
    }
}

impl TaskRecord {
    pub fn new(id: String, title: String, cwd: String) -> Self {
        Self {
            id,
            title,
            cwd,
            enabled: false,
            state: TaskState::Paused,
            detail: "自动续跑已关闭".into(),
            resets_at: None,
            events: Vec::new(),
            failed_turn_id: None,
            observed_turn_id: None,
            observed_turn_status: None,
            observed_error_code: None,
            needs_reconciliation: true,
            required_limit_ids: Vec::new(),
        }
    }

    pub fn set_status(&mut self, state: TaskState, detail: impl Into<String>, now: i64) {
        let detail = detail.into();
        if self.state != state || self.detail != detail {
            self.events.push(TaskEvent {
                at: now,
                message: detail.clone(),
            });
            if self.events.len() > MAX_EVENTS {
                self.events.drain(..self.events.len() - MAX_EVENTS);
            }
        }
        self.state = state;
        self.detail = detail;
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnStatus {
    InProgress,
    Completed,
    Interrupted,
    Failed,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TurnObservation {
    pub id: String,
    pub status: TurnStatus,
    /// The exact structured App Server error code; never infer this from prose.
    pub error_code: Option<String>,
}

pub fn set_enabled(task: &mut TaskRecord, enabled: bool, now: i64) {
    task.enabled = enabled;
    task.resets_at = None;
    if enabled {
        // A toggle is consent to monitor, not evidence that an old turn is still current.
        task.needs_reconciliation = true;
        task.set_status(TaskState::Reconciling, "正在核对会话状态", now);
    } else {
        task.set_status(TaskState::Paused, "自动续跑已关闭", now);
    }
}

/// Use for a verified current observation. Reconciliation is deliberately a separate step.
pub fn observe_turn(task: &mut TaskRecord, turn: &TurnObservation, now: i64) {
    if turn.id.trim().is_empty() {
        block(task, "无法识别当前轮次，需要重新核对", now);
        return;
    }
    if task.observed_turn_id.as_deref() == Some(turn.id.as_str())
        && task.observed_turn_status.as_ref() == Some(&turn.status)
        && task.observed_error_code == turn.error_code
        && task.state != TaskState::Reconciling
    {
        // Polling the same terminal history must not undo a later safety block or
        // produce a WaitingQuota -> NeedsAttention log pair every poll. Explicit
        // reconciliation and enabling set Reconciling and perform a fresh transition.
        return;
    }
    task.observed_turn_id = Some(turn.id.clone());
    task.observed_turn_status = Some(turn.status.clone());
    task.observed_error_code = turn.error_code.clone();
    task.failed_turn_id = None;
    task.resets_at = None;
    match turn.status {
        TurnStatus::Failed if turn.error_code.as_deref() == Some("usageLimitExceeded") => {
            task.failed_turn_id = Some(turn.id.clone());
            if task.enabled {
                task.set_status(TaskState::WaitingQuota, "因额度限制停止，等待核对额度", now);
            }
        }
        TurnStatus::InProgress if task.enabled => {
            task.set_status(TaskState::Running, "任务正在执行", now);
        }
        TurnStatus::Completed if task.enabled => {
            task.set_status(TaskState::Completed, "本轮已结束", now);
        }
        TurnStatus::Interrupted => {
            // Manual stop revokes automatic continuation until the user enables it again.
            task.enabled = false;
            task.set_status(TaskState::Paused, "任务已停止，自动续跑已关闭", now);
        }
        TurnStatus::Failed if task.enabled => {
            task.set_status(
                TaskState::NeedsAttention,
                "任务因其他错误停止，需要处理",
                now,
            );
        }
        TurnStatus::Unknown if task.enabled => {
            task.needs_reconciliation = true;
            task.set_status(TaskState::NeedsAttention, "无法确认会话状态，需要处理", now);
        }
        _ => {}
    }
    if !task.enabled && task.state != TaskState::Paused {
        task.set_status(TaskState::Paused, "自动续跑已关闭", now);
    }
}

pub fn block(task: &mut TaskRecord, reason: impl Into<String>, now: i64) {
    task.failed_turn_id = None;
    task.resets_at = None;
    task.needs_reconciliation = true;
    if task.enabled {
        task.set_status(TaskState::NeedsAttention, reason, now);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ownership {
    /// The runtime has proof that no other execution client can start this task.
    ExclusiveStopped,
    ActiveElsewhere,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SafetyGate {
    pub ownership: Ownership,
    pub execution_configuration_preserved: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuotaWindow {
    pub used_percent: i64,
    pub resets_at: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuotaBucket {
    pub id: String,
    pub primary: Option<QuotaWindow>,
    pub secondary: Option<QuotaWindow>,
    pub credits: Option<CreditsSnapshot>,
    pub individual_limit: Option<SpendControlWindow>,
    pub spend_control_reached: Option<bool>,
    pub rate_limit_reached_type: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreditsSnapshot {
    pub has_credits: bool,
    pub unlimited: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpendControlWindow {
    pub remaining_percent: i64,
    pub resets_at: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuotaSnapshot {
    /// UTC epoch seconds at the successful query, never the last local clock tick.
    pub observed_at: i64,
    pub buckets: Vec<QuotaBucket>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    Noop,
    /// None means the server supplied no useful future reset time. Poll with backoff.
    WaitUntil(Option<i64>),
    NeedsAttention(String),
    Submit {
        failed_turn_id: String,
    },
}

/// Pure decision function. A Submit is only permission to atomically claim an intent;
/// the persisted claim and a final runtime ownership check are still required before I/O.
pub fn evaluate(
    task: &TaskRecord,
    quota: &QuotaSnapshot,
    now: i64,
    safety: &SafetyGate,
) -> Decision {
    if !task.enabled {
        return Decision::Noop;
    }
    if task.needs_reconciliation {
        return Decision::NeedsAttention("需要先核对会话与上次提交结果".into());
    }
    let Some(failed_turn_id) = task.failed_turn_id.as_ref() else {
        return Decision::Noop;
    };
    if task.state != TaskState::WaitingQuota {
        return Decision::Noop;
    }
    if safety.ownership != Ownership::ExclusiveStopped {
        return Decision::NeedsAttention("无法确认已安全接管会话，自动续跑未启动".into());
    }
    if !safety.execution_configuration_preserved {
        return Decision::NeedsAttention("无法保留原执行权限与配置，自动续跑未启动".into());
    }
    let age = now.checked_sub(quota.observed_at);
    if !matches!(age, Some(0..=MAX_QUOTA_AGE_SECONDS)) {
        return Decision::WaitUntil(None);
    }
    if task.required_limit_ids.is_empty()
        || task
            .required_limit_ids
            .iter()
            .any(|id| id.trim().is_empty())
    {
        return Decision::NeedsAttention("无法确认此任务适用的额度，需要处理".into());
    }

    let mut exhausted = false;
    let mut unknown_reset = false;
    let mut next_reset = None;
    for id in &task.required_limit_ids {
        let mut matches = quota.buckets.iter().filter(|bucket| &bucket.id == id);
        let Some(bucket) = matches.next() else {
            return Decision::NeedsAttention("服务端未返回此任务所需的完整额度".into());
        };
        if matches.next().is_some() {
            return Decision::NeedsAttention("服务端额度标识重复，需要重新核对".into());
        }
        match bucket.spend_control_reached {
            Some(false) => {}
            Some(true) => return Decision::NeedsAttention("已触及消费控制，需要用户处理".into()),
            None => return Decision::NeedsAttention("无法确认消费控制状态，需要处理".into()),
        }
        match bucket.rate_limit_reached_type.as_deref() {
            None => {}
            Some("rate_limit_reached") => {
                exhausted = true;
                // A reached flag with apparently available windows is contradictory.
                if ![&bucket.primary, &bucket.secondary]
                    .into_iter()
                    .flatten()
                    .any(|window| window.used_percent == 100)
                {
                    unknown_reset = true;
                }
            }
            Some(_) => {
                return Decision::NeedsAttention("工作区额度或未知限制需要用户处理".into());
            }
        }
        // Credits never substitute for subscription-window availability. This app
        // neither changes a billing mode nor consumes a reset credit.
        if let Some(individual) = &bucket.individual_limit {
            if !(0..=100).contains(&individual.remaining_percent) {
                return Decision::NeedsAttention("个人消费控制数值无法识别，需要处理".into());
            }
            if individual.remaining_percent == 0 {
                exhausted = true;
                match individual.resets_at {
                    Some(reset) if reset > now => {
                        next_reset = Some(next_reset.map_or(reset, |value: i64| value.max(reset)));
                    }
                    _ => unknown_reset = true,
                }
            }
        }
        if bucket.primary.is_none() && bucket.secondary.is_none() {
            return Decision::NeedsAttention("服务端额度窗口未知，需要处理".into());
        }
        for window in [&bucket.primary, &bucket.secondary].into_iter().flatten() {
            if !(0..=100).contains(&window.used_percent) {
                return Decision::NeedsAttention("服务端额度数值无法识别，需要处理".into());
            }
            if window.used_percent == 100 {
                exhausted = true;
                match window.resets_at {
                    Some(reset) if reset > now => {
                        next_reset = Some(next_reset.map_or(reset, |value: i64| value.max(reset)));
                    }
                    _ => unknown_reset = true,
                }
            }
        }
    }
    if exhausted {
        // Passing a reset timestamp never establishes availability. A fresh server
        // snapshot must actually show capacity in every required window.
        return Decision::WaitUntil(if unknown_reset { None } else { next_reset });
    }
    Decision::Submit {
        failed_turn_id: failed_turn_id.clone(),
    }
}

pub fn apply_decision(task: &mut TaskRecord, decision: &Decision, now: i64) {
    match decision {
        Decision::WaitUntil(reset) => {
            task.resets_at = *reset;
            task.set_status(
                TaskState::WaitingQuota,
                if reset.is_some() {
                    "等待额度恢复，到时将重新核对"
                } else {
                    "等待服务端确认额度恢复"
                },
                now,
            );
        }
        Decision::NeedsAttention(reason) => {
            task.resets_at = None;
            task.set_status(TaskState::NeedsAttention, reason.clone(), now);
        }
        Decision::Noop | Decision::Submit { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eligible() -> TaskRecord {
        let mut task = TaskRecord::new("thread".into(), "test".into(), "/project".into());
        task.enabled = true;
        task.needs_reconciliation = false;
        task.required_limit_ids = vec!["codex".into()];
        observe_turn(
            &mut task,
            &TurnObservation {
                id: "failed-turn".into(),
                status: TurnStatus::Failed,
                error_code: Some("usageLimitExceeded".into()),
            },
            100,
        );
        task
    }

    fn safe() -> SafetyGate {
        SafetyGate {
            ownership: Ownership::ExclusiveStopped,
            execution_configuration_preserved: true,
        }
    }

    fn quota(used_percent: i64, resets_at: Option<i64>) -> QuotaSnapshot {
        QuotaSnapshot {
            observed_at: 100,
            buckets: vec![QuotaBucket {
                id: "codex".into(),
                primary: Some(QuotaWindow {
                    used_percent,
                    resets_at,
                }),
                secondary: None,
                credits: None,
                individual_limit: None,
                spend_control_reached: Some(false),
                rate_limit_reached_type: None,
            }],
        }
    }

    #[test]
    fn only_terminal_structured_usage_error_qualifies() {
        let task = eligible();
        assert!(matches!(
            evaluate(&task, &quota(0, None), 100, &safe()),
            Decision::Submit { .. }
        ));
        for (status, code) in [
            (TurnStatus::InProgress, Some("usageLimitExceeded")),
            (TurnStatus::Completed, Some("usageLimitExceeded")),
            (TurnStatus::Interrupted, Some("usageLimitExceeded")),
            (TurnStatus::Unknown, Some("usageLimitExceeded")),
            (TurnStatus::Failed, Some("http429")),
            (TurnStatus::Failed, Some("Quota usageLimitExceeded")),
            (TurnStatus::Failed, None),
        ] {
            let mut candidate = eligible();
            observe_turn(
                &mut candidate,
                &TurnObservation {
                    id: "latest".into(),
                    status,
                    error_code: code.map(str::to_owned),
                },
                100,
            );
            assert!(candidate.failed_turn_id.is_none());
            assert!(!matches!(
                evaluate(&candidate, &quota(0, None), 100, &safe()),
                Decision::Submit { .. }
            ));
        }
    }

    #[test]
    fn every_applicable_bucket_and_window_must_have_capacity() {
        let mut task = eligible();
        task.required_limit_ids.push("model".into());
        let mut snapshot = quota(20, Some(200));
        snapshot.buckets.push(QuotaBucket {
            id: "model".into(),
            primary: Some(QuotaWindow {
                used_percent: 1,
                resets_at: Some(200),
            }),
            secondary: Some(QuotaWindow {
                used_percent: 100,
                resets_at: Some(900),
            }),
            credits: None,
            individual_limit: None,
            spend_control_reached: Some(false),
            rate_limit_reached_type: None,
        });
        assert_eq!(
            evaluate(&task, &snapshot, 100, &safe()),
            Decision::WaitUntil(Some(900))
        );
        snapshot.buckets[0].primary.as_mut().unwrap().used_percent = 100;
        snapshot.buckets[0].primary.as_mut().unwrap().resets_at = Some(1200);
        assert_eq!(
            evaluate(&task, &snapshot, 100, &safe()),
            Decision::WaitUntil(Some(1200))
        );
        snapshot.buckets[0].primary.as_mut().unwrap().used_percent = 99;
        snapshot.buckets[1].secondary.as_mut().unwrap().used_percent = 99;
        assert!(matches!(
            evaluate(&task, &snapshot, 100, &safe()),
            Decision::Submit { .. }
        ));
    }

    #[test]
    fn reset_time_alone_and_stale_or_future_snapshot_never_allow_send() {
        let task = eligible();
        assert_eq!(
            evaluate(&task, &quota(100, Some(99)), 100, &safe()),
            Decision::WaitUntil(None)
        );
        assert_eq!(
            evaluate(&task, &quota(100, None), 100, &safe()),
            Decision::WaitUntil(None)
        );
        assert_eq!(
            evaluate(&task, &quota(0, Some(200)), 161, &safe()),
            Decision::WaitUntil(None)
        );
        assert_eq!(
            evaluate(&task, &quota(0, None), 99, &safe()),
            Decision::WaitUntil(None)
        );
    }

    #[test]
    fn unknown_missing_duplicate_or_malformed_quota_fails_closed() {
        let task = eligible();
        let mut snapshots = vec![
            QuotaSnapshot {
                observed_at: 100,
                buckets: vec![],
            },
            quota(-1, None),
            quota(101, None),
        ];
        let mut unknown = quota(0, None);
        unknown.buckets[0].primary = None;
        snapshots.push(unknown);
        let mut duplicate = quota(0, None);
        duplicate.buckets.push(duplicate.buckets[0].clone());
        snapshots.push(duplicate);
        for snapshot in snapshots {
            assert!(matches!(
                evaluate(&task, &snapshot, 100, &safe()),
                Decision::NeedsAttention(_)
            ));
        }
        let mut unidentified = task;
        unidentified.required_limit_ids.clear();
        assert!(matches!(
            evaluate(&unidentified, &quota(0, None), 100, &safe()),
            Decision::NeedsAttention(_)
        ));
    }

    #[test]
    fn pause_stop_approval_and_normal_completion_never_auto_continue() {
        let mut task = eligible();
        set_enabled(&mut task, false, 100);
        assert_eq!(
            evaluate(&task, &quota(0, None), 100, &safe()),
            Decision::Noop
        );
        set_enabled(&mut task, true, 100);
        assert!(matches!(
            evaluate(&task, &quota(0, None), 100, &safe()),
            Decision::NeedsAttention(_)
        ));
        let mut task = eligible();
        block(&mut task, "需要用户审批", 100);
        assert!(task.failed_turn_id.is_none());
        assert!(!matches!(
            evaluate(&task, &quota(0, None), 100, &safe()),
            Decision::Submit { .. }
        ));
        for status in [TurnStatus::Interrupted, TurnStatus::Completed] {
            let mut task = eligible();
            observe_turn(
                &mut task,
                &TurnObservation {
                    id: "latest".into(),
                    status,
                    error_code: None,
                },
                100,
            );
            assert_eq!(
                evaluate(&task, &quota(0, None), 100, &safe()),
                Decision::Noop
            );
        }
    }

    #[test]
    fn external_unknown_ownership_or_changed_configuration_refuses() {
        for gate in [
            SafetyGate {
                ownership: Ownership::Unknown,
                execution_configuration_preserved: true,
            },
            SafetyGate {
                ownership: Ownership::ActiveElsewhere,
                execution_configuration_preserved: true,
            },
            SafetyGate {
                ownership: Ownership::ExclusiveStopped,
                execution_configuration_preserved: false,
            },
        ] {
            assert!(matches!(
                evaluate(&eligible(), &quota(0, None), 100, &gate),
                Decision::NeedsAttention(_)
            ));
        }
    }

    #[test]
    fn repeated_history_does_not_clear_a_safety_block_or_spam_events() {
        let mut task = eligible();
        let observation = TurnObservation {
            id: "failed-turn".into(),
            status: TurnStatus::Failed,
            error_code: Some("usageLimitExceeded".into()),
        };
        let gate = SafetyGate {
            ownership: Ownership::Unknown,
            execution_configuration_preserved: false,
        };
        let decision = evaluate(&task, &quota(0, None), 100, &gate);
        apply_decision(&mut task, &decision, 100);
        let event_count = task.events.len();
        for now in 101..110 {
            observe_turn(&mut task, &observation, now);
            let decision = evaluate(&task, &quota(0, None), now, &gate);
            apply_decision(&mut task, &decision, now);
        }
        assert_eq!(task.events.len(), event_count);
        assert_eq!(task.state, TaskState::NeedsAttention);
        block(&mut task, "需要用户审批", 111);
        observe_turn(&mut task, &observation, 112);
        assert!(task.failed_turn_id.is_none());
        assert_eq!(task.detail, "需要用户审批");
    }

    #[test]
    fn spend_controls_reached_flags_and_credits_cannot_bypass_windows() {
        let task = eligible();
        let mut snapshot = quota(100, Some(900));
        snapshot.buckets[0].credits = Some(CreditsSnapshot {
            has_credits: true,
            unlimited: true,
        });
        assert_eq!(
            evaluate(&task, &snapshot, 100, &safe()),
            Decision::WaitUntil(Some(900))
        );
        snapshot.buckets[0].primary.as_mut().unwrap().used_percent = 10;
        snapshot.buckets[0].rate_limit_reached_type = Some("rate_limit_reached".into());
        assert_eq!(
            evaluate(&task, &snapshot, 100, &safe()),
            Decision::WaitUntil(None)
        );
        for reached in [
            "workspace_owner_credits_depleted",
            "workspace_member_credits_depleted",
            "workspace_owner_usage_limit_reached",
            "workspace_member_usage_limit_reached",
            "future_unknown_type",
        ] {
            snapshot.buckets[0].rate_limit_reached_type = Some(reached.into());
            assert!(matches!(
                evaluate(&task, &snapshot, 100, &safe()),
                Decision::NeedsAttention(_)
            ));
        }
        snapshot.buckets[0].rate_limit_reached_type = None;
        for spend in [None, Some(true)] {
            snapshot.buckets[0].spend_control_reached = spend;
            assert!(matches!(
                evaluate(&task, &snapshot, 100, &safe()),
                Decision::NeedsAttention(_)
            ));
        }
    }
}
