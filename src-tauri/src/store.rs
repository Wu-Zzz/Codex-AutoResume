use crate::scheduler::{observe_turn, TaskRecord, TaskState, TurnObservation, TurnStatus};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::{path::Path, time::Duration};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("stored task: {0}")]
    Json(#[from] serde_json::Error),
    #[error("task not found: {0}")]
    TaskNotFound(String),
    #[error("task is no longer eligible for continuation")]
    NotEligible,
    #[error("submission intent not found")]
    IntentNotFound,
    #[error("submission result conflicts with the persisted intent")]
    ConflictingResult,
}

pub type Result<T> = std::result::Result<T, StoreError>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntentStatus {
    Claimed,
    Submitted,
    Ambiguous,
    Reconciled,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubmissionIntent {
    pub id: i64,
    pub thread_id: String,
    pub failed_turn_id: String,
    pub status: IntentStatus,
    pub remote_turn_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClaimResult {
    Claimed(SubmissionIntent),
    /// Existing means do not send, irrespective of its current status.
    Existing(SubmissionIntent),
}

pub struct Store {
    connection: Connection,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS app_settings (
                key TEXT PRIMARY KEY NOT NULL,
                value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS tasks (
                id TEXT PRIMARY KEY NOT NULL,
                record TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS submission_intents (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                thread_id TEXT NOT NULL,
                failed_turn_id TEXT NOT NULL,
                status TEXT NOT NULL CHECK(status IN ('claimed', 'submitted', 'ambiguous', 'reconciled')),
                remote_turn_id TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                UNIQUE(thread_id, failed_turn_id)
            );",
        )?;
        Ok(Self { connection })
    }

    pub fn save_task(&self, task: &TaskRecord) -> Result<()> {
        write_task(&self.connection, task)
    }

    pub fn set_paused(&self, paused: bool) -> Result<()> {
        self.connection.execute(
            "INSERT INTO app_settings (key, value) VALUES ('paused', ?1)
                ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [if paused { "true" } else { "false" }],
        )?;
        Ok(())
    }

    pub fn is_paused(&self) -> Result<bool> {
        read_paused(&self.connection)
    }

    pub fn get_task(&self, id: &str) -> Result<Option<TaskRecord>> {
        read_task(&self.connection, id)
    }

    pub fn load_tasks(&self) -> Result<Vec<TaskRecord>> {
        let mut statement = self
            .connection
            .prepare("SELECT record FROM tasks ORDER BY id")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        let mut tasks = Vec::new();
        for row in rows {
            tasks.push(serde_json::from_str(&row?)?);
        }
        Ok(tasks)
    }

    pub fn remove_task(&self, id: &str) -> Result<()> {
        // Keep intent tombstones: removing and re-adding a task must not replay a send.
        self.connection
            .execute("DELETE FROM tasks WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Atomic across database handles. Only the caller receiving Claimed may send.
    /// The claim itself makes the task require reconciliation, including a crash
    /// before the first byte is sent: local intent is not proof of remote delivery.
    pub fn claim_submission(
        &mut self,
        thread_id: &str,
        failed_turn_id: &str,
        now: i64,
    ) -> Result<ClaimResult> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(existing) = read_intent(&transaction, thread_id, failed_turn_id)? {
            transaction.commit()?;
            return Ok(ClaimResult::Existing(existing));
        }
        let mut task = read_task(&transaction, thread_id)?
            .ok_or_else(|| StoreError::TaskNotFound(thread_id.into()))?;
        if read_paused(&transaction)?
            || !task.enabled
            || task.needs_reconciliation
            || task.state != TaskState::WaitingQuota
            || task.failed_turn_id.as_deref() != Some(failed_turn_id)
            || task.observed_turn_id.as_deref() != Some(failed_turn_id)
        {
            return Err(StoreError::NotEligible);
        }
        // A second failed turn must not bypass an unresolved earlier send.
        let unresolved: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM submission_intents
                WHERE thread_id = ?1 AND status != 'reconciled')",
            [thread_id],
            |row| row.get(0),
        )?;
        if unresolved {
            return Err(StoreError::NotEligible);
        }
        transaction.execute(
            "INSERT INTO submission_intents
                (thread_id, failed_turn_id, status, created_at, updated_at)
                VALUES (?1, ?2, 'claimed', ?3, ?3)",
            params![thread_id, failed_turn_id, now],
        )?;
        task.needs_reconciliation = true;
        task.resets_at = None;
        task.set_status(
            TaskState::Reconciling,
            "已记录续跑意图，等待核对提交结果",
            now,
        );
        write_task(&transaction, &task)?;
        let intent = read_intent(&transaction, thread_id, failed_turn_id)?
            .ok_or(StoreError::IntentNotFound)?;
        transaction.commit()?;
        Ok(ClaimResult::Claimed(intent))
    }

    /// Call only with an explicit successful turn/start response and its turn ID.
    pub fn mark_submitted(
        &mut self,
        thread_id: &str,
        failed_turn_id: &str,
        remote_turn_id: &str,
        now: i64,
    ) -> Result<()> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let intent = read_intent(&transaction, thread_id, failed_turn_id)?
            .ok_or(StoreError::IntentNotFound)?;
        if remote_turn_id.is_empty()
            || remote_turn_id == failed_turn_id
            || intent
                .remote_turn_id
                .as_deref()
                .is_some_and(|existing| existing != remote_turn_id)
        {
            return Err(StoreError::ConflictingResult);
        }
        if intent.status == IntentStatus::Reconciled {
            transaction.commit()?;
            return Ok(());
        }
        transaction.execute(
            "UPDATE submission_intents SET status = 'submitted',
                remote_turn_id = ?3, updated_at = ?4
                WHERE thread_id = ?1 AND failed_turn_id = ?2",
            params![thread_id, failed_turn_id, remote_turn_id, now],
        )?;
        if let Some(mut task) = read_task(&transaction, thread_id)? {
            // Keep reconciliation required until the current server turn is read.
            task.needs_reconciliation = true;
            task.failed_turn_id = None;
            task.observed_turn_id = Some(remote_turn_id.into());
            if task.enabled {
                task.set_status(TaskState::Reconciling, "续跑已提交，正在核对执行状态", now);
            }
            write_task(&transaction, &task)?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn mark_ambiguous(
        &mut self,
        thread_id: &str,
        failed_turn_id: &str,
        now: i64,
    ) -> Result<()> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let intent = read_intent(&transaction, thread_id, failed_turn_id)?
            .ok_or(StoreError::IntentNotFound)?;
        if matches!(
            intent.status,
            IntentStatus::Claimed | IntentStatus::Ambiguous
        ) {
            transaction.execute(
                "UPDATE submission_intents SET status = 'ambiguous', updated_at = ?3
                    WHERE thread_id = ?1 AND failed_turn_id = ?2",
                params![thread_id, failed_turn_id, now],
            )?;
            if let Some(mut task) = read_task(&transaction, thread_id)? {
                task.needs_reconciliation = true;
                task.resets_at = None;
                if task.enabled {
                    task.set_status(
                        TaskState::NeedsAttention,
                        "提交结果不明，已停止自动重试",
                        now,
                    );
                }
                write_task(&transaction, &task)?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn get_intent(
        &self,
        thread_id: &str,
        failed_turn_id: &str,
    ) -> Result<Option<SubmissionIntent>> {
        read_intent(&self.connection, thread_id, failed_turn_id)
    }

    /// Invoke at process start and after a transport loss or wake event. It never sends.
    pub fn recover_after_restart(&mut self, now: i64) -> Result<()> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "UPDATE submission_intents SET status = 'ambiguous', updated_at = ?1
                WHERE status = 'claimed'",
            [now],
        )?;
        let tasks = {
            let mut statement = transaction.prepare("SELECT record FROM tasks")?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            let mut tasks = Vec::new();
            for row in rows {
                tasks.push(serde_json::from_str::<TaskRecord>(&row?)?);
            }
            tasks
        };
        for mut task in tasks {
            task.needs_reconciliation = true;
            task.resets_at = None;
            if task.enabled {
                task.set_status(
                    TaskState::Reconciling,
                    "重新连接后正在核对会话与提交结果",
                    now,
                );
            }
            write_task(&transaction, &task)?;
        }
        transaction.commit()?;
        Ok(())
    }

    /// Reconcile from a fresh thread/read. An unknown send result has no reliable
    /// remote turn ID and remains blocked even when the original turn is still last.
    pub fn reconcile_observation(
        &mut self,
        thread_id: &str,
        observation: &TurnObservation,
        now: i64,
    ) -> Result<TaskRecord> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut task = read_task(&transaction, thread_id)?
            .ok_or_else(|| StoreError::TaskNotFound(thread_id.into()))?;
        let pending = read_pending_intent(&transaction, thread_id)?;
        if let Some(intent) = pending {
            if intent.remote_turn_id.as_deref() == Some(observation.id.as_str())
                && observation.status != TurnStatus::Unknown
            {
                transaction.execute(
                    "UPDATE submission_intents SET status = 'reconciled', updated_at = ?2
                        WHERE id = ?1",
                    params![intent.id, now],
                )?;
                task.needs_reconciliation = false;
                observe_turn(&mut task, observation, now);
            } else {
                task.needs_reconciliation = true;
                task.failed_turn_id = None;
                task.resets_at = None;
                if task.enabled {
                    task.set_status(
                        TaskState::NeedsAttention,
                        "无法确认上次续跑结果，需要用户处理，未自动重试",
                        now,
                    );
                }
            }
        } else {
            task.needs_reconciliation = false;
            observe_turn(&mut task, observation, now);
        }
        write_task(&transaction, &task)?;
        transaction.commit()?;
        Ok(task)
    }
}

fn write_task(connection: &Connection, task: &TaskRecord) -> Result<()> {
    connection.execute(
        "INSERT INTO tasks (id, record) VALUES (?1, ?2)
            ON CONFLICT(id) DO UPDATE SET record = excluded.record",
        params![task.id, serde_json::to_string(task)?],
    )?;
    Ok(())
}

fn read_paused(connection: &Connection) -> Result<bool> {
    let value: Option<String> = connection
        .query_row(
            "SELECT value FROM app_settings WHERE key = 'paused'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    // A malformed persisted setting must not silently authorize work.
    Ok(!matches!(value.as_deref(), None | Some("false")))
}

fn read_task(connection: &Connection, id: &str) -> Result<Option<TaskRecord>> {
    let record: Option<String> = connection
        .query_row("SELECT record FROM tasks WHERE id = ?1", [id], |row| {
            row.get(0)
        })
        .optional()?;
    record
        .map(|value| serde_json::from_str(&value).map_err(StoreError::from))
        .transpose()
}

fn row_to_intent(row: &rusqlite::Row<'_>) -> rusqlite::Result<SubmissionIntent> {
    let status: String = row.get(3)?;
    let status = match status.as_str() {
        "claimed" => IntentStatus::Claimed,
        "submitted" => IntentStatus::Submitted,
        "ambiguous" => IntentStatus::Ambiguous,
        "reconciled" => IntentStatus::Reconciled,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    Ok(SubmissionIntent {
        id: row.get(0)?,
        thread_id: row.get(1)?,
        failed_turn_id: row.get(2)?,
        status,
        remote_turn_id: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

fn read_intent(
    connection: &Connection,
    thread_id: &str,
    failed_turn_id: &str,
) -> Result<Option<SubmissionIntent>> {
    Ok(connection
        .query_row(
            "SELECT id, thread_id, failed_turn_id, status, remote_turn_id, created_at, updated_at
                FROM submission_intents WHERE thread_id = ?1 AND failed_turn_id = ?2",
            params![thread_id, failed_turn_id],
            row_to_intent,
        )
        .optional()?)
}

fn read_pending_intent(
    connection: &Connection,
    thread_id: &str,
) -> Result<Option<SubmissionIntent>> {
    Ok(connection
        .query_row(
            "SELECT id, thread_id, failed_turn_id, status, remote_turn_id, created_at, updated_at
                FROM submission_intents WHERE thread_id = ?1 AND status != 'reconciled'
                ORDER BY id DESC LIMIT 1",
            [thread_id],
            row_to_intent,
        )
        .optional()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::{
        apply_decision, evaluate, set_enabled, Decision, Ownership, QuotaBucket, QuotaSnapshot,
        QuotaWindow, SafetyGate,
    };
    use std::{
        path::PathBuf,
        sync::{
            atomic::{AtomicU64, Ordering},
            Arc, Barrier,
        },
        time::{SystemTime, UNIX_EPOCH},
    };

    static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

    struct TestDb(PathBuf);

    impl TestDb {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            Self(std::env::temp_dir().join(format!(
                "codex-autoresume-test-{}-{}-{}.db",
                std::process::id(),
                nonce,
                NEXT_FILE.fetch_add(1, Ordering::Relaxed)
            )))
        }
        fn open(&self) -> Store {
            Store::open(&self.0).unwrap()
        }
    }

    impl Drop for TestDb {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
            let _ = std::fs::remove_file(format!("{}-wal", self.0.display()));
            let _ = std::fs::remove_file(format!("{}-shm", self.0.display()));
        }
    }

    fn failed_turn(id: &str) -> TurnObservation {
        TurnObservation {
            id: id.into(),
            status: TurnStatus::Failed,
            error_code: Some("usageLimitExceeded".into()),
        }
    }

    fn eligible() -> TaskRecord {
        let mut task = TaskRecord::new("thread".into(), "Example".into(), "/project".into());
        task.enabled = true;
        task.needs_reconciliation = false;
        task.required_limit_ids = vec!["codex".into()];
        observe_turn(&mut task, &failed_turn("failed"), 100);
        task
    }

    #[test]
    fn competing_database_handles_claim_at_most_once() {
        let db = TestDb::new();
        db.open().save_task(&eligible()).unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let threads: Vec<_> = (0..2)
            .map(|_| {
                let path = db.0.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let mut store = Store::open(path).unwrap();
                    barrier.wait();
                    store.claim_submission("thread", "failed", 100).unwrap()
                })
            })
            .collect();
        let results: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, ClaimResult::Claimed(_)))
                .count(),
            1
        );
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, ClaimResult::Existing(_)))
                .count(),
            1
        );
    }

    #[test]
    fn crash_after_claim_before_send_does_not_replay() {
        let db = TestDb::new();
        {
            let mut store = db.open();
            store.save_task(&eligible()).unwrap();
            assert!(matches!(
                store.claim_submission("thread", "failed", 100).unwrap(),
                ClaimResult::Claimed(_)
            ));
        }
        let mut recovered = db.open();
        recovered.recover_after_restart(200).unwrap();
        let task = recovered
            .reconcile_observation("thread", &failed_turn("failed"), 200)
            .unwrap();
        assert!(task.needs_reconciliation);
        assert_eq!(task.state, TaskState::NeedsAttention);
        assert_eq!(
            recovered
                .get_intent("thread", "failed")
                .unwrap()
                .unwrap()
                .status,
            IntentStatus::Ambiguous
        );
        assert!(matches!(
            recovered.claim_submission("thread", "failed", 200).unwrap(),
            ClaimResult::Existing(_)
        ));
    }

    #[test]
    fn response_lost_after_send_neither_old_nor_new_observation_allows_replay() {
        let db = TestDb::new();
        let mut store = db.open();
        store.save_task(&eligible()).unwrap();
        store.claim_submission("thread", "failed", 100).unwrap();
        store.mark_ambiguous("thread", "failed", 101).unwrap();
        for observation in [failed_turn("failed"), failed_turn("unattributed-new-turn")] {
            let task = store
                .reconcile_observation("thread", &observation, 200)
                .unwrap();
            assert!(task.needs_reconciliation);
            assert!(task.failed_turn_id.is_none());
            assert!(matches!(
                store.claim_submission("thread", "failed", 200).unwrap(),
                ClaimResult::Existing(_)
            ));
            assert!(matches!(
                store.claim_submission("thread", "unattributed-new-turn", 200),
                Err(StoreError::NotEligible)
            ));
        }
    }

    #[test]
    fn successful_send_survives_restart_and_reconciles_current_turn() {
        let db = TestDb::new();
        {
            let mut store = db.open();
            store.save_task(&eligible()).unwrap();
            store.claim_submission("thread", "failed", 100).unwrap();
            store
                .mark_submitted("thread", "failed", "resumed", 101)
                .unwrap();
        }
        let mut store = db.open();
        store.recover_after_restart(200).unwrap();
        assert!(
            store
                .get_task("thread")
                .unwrap()
                .unwrap()
                .needs_reconciliation
        );
        let task = store
            .reconcile_observation(
                "thread",
                &TurnObservation {
                    id: "resumed".into(),
                    status: TurnStatus::InProgress,
                    error_code: None,
                },
                201,
            )
            .unwrap();
        assert!(!task.needs_reconciliation);
        assert_eq!(task.state, TaskState::Running);
        assert_eq!(
            store
                .get_intent("thread", "failed")
                .unwrap()
                .unwrap()
                .status,
            IntentStatus::Reconciled
        );
        // A later explicit quota failure is a new, independently deduplicated intent.
        store
            .reconcile_observation("thread", &failed_turn("resumed"), 202)
            .unwrap();
        assert!(matches!(
            store.claim_submission("thread", "resumed", 203).unwrap(),
            ClaimResult::Claimed(_)
        ));
    }

    #[test]
    fn known_submission_requires_matching_non_unknown_server_turn() {
        let db = TestDb::new();
        let mut store = db.open();
        store.save_task(&eligible()).unwrap();
        store.claim_submission("thread", "failed", 100).unwrap();
        store
            .mark_submitted("thread", "failed", "resumed", 101)
            .unwrap();
        for observation in [
            failed_turn("failed"),
            TurnObservation {
                id: "resumed".into(),
                status: TurnStatus::Unknown,
                error_code: None,
            },
            failed_turn("unrelated"),
        ] {
            let task = store
                .reconcile_observation("thread", &observation, 102)
                .unwrap();
            assert!(task.needs_reconciliation);
            assert_eq!(
                store
                    .get_intent("thread", "failed")
                    .unwrap()
                    .unwrap()
                    .status,
                IntentStatus::Submitted
            );
        }
        assert!(matches!(
            store.mark_submitted("thread", "failed", "different", 103),
            Err(StoreError::ConflictingResult)
        ));
        let task = store
            .reconcile_observation(
                "thread",
                &TurnObservation {
                    id: "resumed".into(),
                    status: TurnStatus::Completed,
                    error_code: None,
                },
                104,
            )
            .unwrap();
        assert!(!task.needs_reconciliation);
        assert_eq!(task.state, TaskState::Completed);
    }

    #[test]
    fn pause_between_evaluation_and_claim_is_respected() {
        let db = TestDb::new();
        let mut store = db.open();
        let mut task = eligible();
        store.save_task(&task).unwrap();
        set_enabled(&mut task, false, 101);
        store.save_task(&task).unwrap();
        assert!(matches!(
            store.claim_submission("thread", "failed", 102),
            Err(StoreError::NotEligible)
        ));
        assert!(store.get_intent("thread", "failed").unwrap().is_none());
    }

    #[test]
    fn global_pause_is_persistent_and_checked_in_atomic_claim() {
        let db = TestDb::new();
        let mut store = db.open();
        store.save_task(&eligible()).unwrap();
        assert!(!store.is_paused().unwrap());
        db.open().set_paused(true).unwrap();
        assert!(store.is_paused().unwrap());
        assert!(matches!(
            store.claim_submission("thread", "failed", 102),
            Err(StoreError::NotEligible)
        ));
        store.set_paused(false).unwrap();
        assert!(matches!(
            store.claim_submission("thread", "failed", 103).unwrap(),
            ClaimResult::Claimed(_)
        ));
    }

    #[test]
    fn removing_and_readding_a_task_preserves_deduplication_tombstone() {
        let db = TestDb::new();
        let mut store = db.open();
        store.save_task(&eligible()).unwrap();
        store.claim_submission("thread", "failed", 100).unwrap();
        store.remove_task("thread").unwrap();
        store.save_task(&eligible()).unwrap();
        assert!(matches!(
            store.claim_submission("thread", "failed", 200).unwrap(),
            ClaimResult::Existing(_)
        ));
        // Even a different failed turn cannot evade an earlier unresolved send.
        let mut newer = eligible();
        observe_turn(&mut newer, &failed_turn("new-failure"), 200);
        store.save_task(&newer).unwrap();
        assert!(matches!(
            store.claim_submission("thread", "new-failure", 201),
            Err(StoreError::NotEligible)
        ));
    }

    #[test]
    fn fake_end_to_end_wait_fresh_check_claim_acknowledge_then_normal_end() {
        let db = TestDb::new();
        let mut store = db.open();
        let mut task = eligible();
        store.save_task(&task).unwrap();
        let safety = SafetyGate {
            ownership: Ownership::ExclusiveStopped,
            execution_configuration_preserved: true,
        };
        let mut quota = QuotaSnapshot {
            observed_at: 100,
            buckets: vec![QuotaBucket {
                id: "codex".into(),
                primary: Some(QuotaWindow {
                    used_percent: 100,
                    resets_at: Some(500),
                }),
                secondary: Some(QuotaWindow {
                    used_percent: 20,
                    resets_at: Some(900),
                }),
                credits: None,
                individual_limit: None,
                spend_control_reached: Some(false),
                rate_limit_reached_type: None,
            }],
        };
        let mut submissions = 0;
        let decision = evaluate(&task, &quota, 100, &safety);
        assert_eq!(decision, Decision::WaitUntil(Some(500)));
        apply_decision(&mut task, &decision, 100);
        store.save_task(&task).unwrap();
        // Timer fired; the old observation is not usable as authorization.
        assert_eq!(
            evaluate(&task, &quota, 500, &safety),
            Decision::WaitUntil(None)
        );
        quota.observed_at = 500;
        quota.buckets[0].primary.as_mut().unwrap().used_percent = 0;
        let Decision::Submit { failed_turn_id } = evaluate(&task, &quota, 500, &safety) else {
            panic!("fresh quota should allow a claim")
        };
        if let ClaimResult::Claimed(_) = store
            .claim_submission(&task.id, &failed_turn_id, 500)
            .unwrap()
        {
            // A fake transport: the only place a request can be emitted.
            submissions += 1;
            store
                .mark_submitted(&task.id, &failed_turn_id, "resumed", 501)
                .unwrap();
        }
        if let ClaimResult::Claimed(_) = store
            .claim_submission(&task.id, &failed_turn_id, 501)
            .unwrap()
        {
            submissions += 1;
        }
        assert_eq!(submissions, 1);
        let completed = store
            .reconcile_observation(
                &task.id,
                &TurnObservation {
                    id: "resumed".into(),
                    status: TurnStatus::Completed,
                    error_code: None,
                },
                502,
            )
            .unwrap();
        assert_eq!(completed.state, TaskState::Completed);
        assert_eq!(evaluate(&completed, &quota, 502, &safety), Decision::Noop);
    }
}
