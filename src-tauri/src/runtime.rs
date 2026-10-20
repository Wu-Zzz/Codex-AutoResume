use crate::{
    protocol::{self, Rpc},
    scheduler::{
        self, Ownership, QuotaSnapshot, SafetyGate, TaskRecord, TaskState, TaskView,
        TurnObservation, TurnStatus,
    },
    store::Store,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    path::Path,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::{Mutex, RwLock};

pub const COMPATIBILITY: &str =
    "开发预览：可查看本地任务；跨客户端安全接管尚未验证，当前不会自动发送继续。";
const TAKEOVER_BLOCKED: &str = "尚不能确认原客户端已停止并保留原权限，自动续跑未启动";

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub connection: String,
    pub connection_message: String,
    pub codex_version: Option<String>,
    pub account_label: Option<String>,
    pub compatibility_message: Option<String>,
    pub paused_all: bool,
    pub checked_at: Option<i64>,
    pub tasks: Vec<TaskView>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            connection: "checking".into(),
            connection_message: "正在连接本机 Codex".into(),
            codex_version: None,
            account_label: None,
            compatibility_message: Some(COMPATIBILITY.into()),
            paused_all: false,
            checked_at: None,
            tasks: vec![],
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    id: String,
    title: String,
    cwd: String,
    updated_at: i64,
    status: String,
    eligible: bool,
    reason: Option<String>,
}

pub struct AppState {
    pub runtime: Mutex<Runtime>,
    pub snapshot: RwLock<Snapshot>,
}

pub struct Runtime {
    rpc: Option<Rpc>,
    store: Store,
    tasks: Vec<TaskRecord>,
    paused: bool,
    snapshot: Snapshot,
}

impl AppState {
    pub fn open(path: &Path) -> Result<Arc<Self>, String> {
        let mut store = Store::open(path).map_err(|e| e.to_string())?;
        store
            .recover_after_restart(now())
            .map_err(|e| e.to_string())?;
        let tasks = store.load_tasks().map_err(|e| e.to_string())?;
        let paused = store.is_paused().map_err(|e| e.to_string())?;
        let snapshot = Snapshot {
            tasks: tasks.iter().map(TaskView::from).collect(),
            paused_all: paused,
            ..Snapshot::default()
        };
        Ok(Arc::new(Self {
            runtime: Mutex::new(Runtime {
                rpc: None,
                store,
                tasks,
                paused,
                snapshot: snapshot.clone(),
            }),
            snapshot: RwLock::new(snapshot),
        }))
    }

    pub async fn publish(&self, runtime: &mut Runtime) {
        let mut snapshot = runtime.snapshot.clone();
        snapshot.tasks = runtime.tasks.iter().map(TaskView::from).collect();
        snapshot.paused_all = runtime.paused;
        *self.snapshot.write().await = snapshot;
    }

    pub async fn refresh(&self) {
        let mut runtime = self.runtime.lock().await;
        if let Err(error) = runtime.refresh().await {
            runtime.snapshot.connection = if error.starts_with("missing:") {
                "missing"
            } else {
                "error"
            }
            .into();
            runtime.snapshot.connection_message = error.trim_start_matches("missing:").into();
            runtime.snapshot.account_label = None;
            runtime.rpc = None;
            for task in &mut runtime.tasks {
                scheduler::block(task, "连接中断，恢复后将重新核对，不会重放操作", now());
            }
            for task in &runtime.tasks {
                if runtime.store.save_task(task).is_err() {
                    runtime.snapshot.connection_message =
                        "连接中断，且本地状态保存失败，请检查磁盘空间".into();
                    break;
                }
            }
        }
        self.publish(&mut runtime).await;
    }
}

fn candidate(thread: &Value) -> Result<Candidate, String> {
    let id = thread
        .get("id")
        .and_then(Value::as_str)
        .ok_or("Codex 会话标识缺失")?;
    let title = thread
        .get("name")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("未命名任务");
    Ok(Candidate {
        id: id.into(),
        title: title.chars().take(160).collect(),
        cwd: thread
            .get("cwd")
            .and_then(Value::as_str)
            .unwrap_or("")
            .into(),
        updated_at: thread.get("updatedAt").and_then(Value::as_i64).unwrap_or(0),
        status: "接管状态未知".into(),
        eligible: false,
        reason: Some(TAKEOVER_BLOCKED.into()),
    })
}

impl Runtime {
    async fn rpc(&mut self) -> Result<&mut Rpc, String> {
        if self.rpc.is_none() {
            let rpc = Rpc::connect().await?;
            self.snapshot.codex_version = Some(rpc.version.clone());
            self.rpc = Some(rpc);
        }
        self.rpc.as_mut().ok_or("Codex 未连接".into())
    }

    async fn refresh(&mut self) -> Result<(), String> {
        let account = self
            .rpc()
            .await?
            .request("account/read", json!({"refreshToken":false}))
            .await?;
        self.snapshot.checked_at = Some(now());
        let kind = account.pointer("/account/type").and_then(Value::as_str);
        if kind != Some("chatgpt") {
            self.snapshot.connection = "login_required".into();
            self.snapshot.connection_message = if kind.is_none() {
                "请在 Codex 登录 ChatGPT 账号"
            } else {
                "当前不是 ChatGPT 登录，自动续跑不会切换付费方式"
            }
            .into();
            self.snapshot.account_label = None;
            for task in &mut self.tasks {
                scheduler::block(task, "需要检查 ChatGPT 登录状态", now());
                self.store.save_task(task).map_err(|e| e.to_string())?;
            }
            return Ok(());
        }
        self.snapshot.account_label = Some(format!(
            "ChatGPT {}",
            account
                .pointer("/account/planType")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_uppercase()
        ));
        self.snapshot.connection = "ready".into();
        self.snapshot.connection_message = "已连接本机 Codex · 只读监测".into();
        if self.paused {
            return Ok(());
        }

        // Do not infer model-to-bucket associations from one account bucket. The
        // future executing adapter must supply verified required_limit_ids.
        let quota = self
            .rpc()
            .await?
            .request("account/rateLimits/read", json!({}))
            .await;
        if quota.is_err() {
            self.snapshot.connection_message = "会话已连接，额度暂时无法读取".into();
        }
        for index in 0..self.tasks.len() {
            if !self.tasks[index].enabled {
                continue;
            }
            let id = self.tasks[index].id.clone();
            let response = self
                .rpc()
                .await?
                .request("thread/read", json!({"threadId":id,"includeTurns":true}))
                .await;
            let task = &mut self.tasks[index];
            match response {
                Ok(response) => {
                    if response.pointer("/thread/id").and_then(Value::as_str) != Some(id.as_str()) {
                        scheduler::block(task, "返回的会话标识不一致，已停止监测", now());
                    } else if let Some((turn_id, status, is_quota)) =
                        protocol::terminal_failure(&response["thread"])
                    {
                        let status = match status {
                            "completed" => TurnStatus::Completed,
                            "interrupted" => TurnStatus::Interrupted,
                            "failed" => TurnStatus::Failed,
                            "inProgress" => TurnStatus::InProgress,
                            _ => TurnStatus::Unknown,
                        };
                        *task = self
                            .store
                            .reconcile_observation(
                                &id,
                                &TurnObservation {
                                    id: turn_id.into(),
                                    status,
                                    error_code: is_quota.then(|| "usageLimitExceeded".into()),
                                },
                                now(),
                            )
                            .map_err(|e| e.to_string())?;
                        // Stored history is not a cross-client runtime guarantee.
                        if task.failed_turn_id.is_some() {
                            let decision = scheduler::evaluate(
                                task,
                                &QuotaSnapshot {
                                    observed_at: now(),
                                    buckets: vec![],
                                },
                                now(),
                                &SafetyGate {
                                    ownership: Ownership::Unknown,
                                    execution_configuration_preserved: false,
                                },
                            );
                            scheduler::apply_decision(task, &decision, now());
                        } else if matches!(
                            task.state,
                            TaskState::Running | TaskState::Reconciling | TaskState::Monitoring
                        ) {
                            scheduler::block(
                                task,
                                "已读取历史记录，但无法确认原客户端当前执行状态",
                                now(),
                            );
                        }
                    } else {
                        scheduler::block(task, "此会话没有可核对的完整轮次记录", now());
                    }
                }
                Err(_) => scheduler::block(task, "此任务暂时无法读取，请在原客户端检查", now()),
            }
            self.store.save_task(task).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub async fn list_candidates(&mut self) -> Result<Vec<Candidate>, String> {
        let mut result = vec![];
        let mut cursor: Option<String> = None;
        let mut seen = std::collections::HashSet::new();
        loop {
            let response = self.rpc().await?.request("thread/list", json!({"cursor":cursor,"limit":100,"sortKey":"updated_at","sourceKinds":["cli","vscode","appServer","exec"],"useStateDbOnly":true})).await?;
            let rows = response
                .get("data")
                .and_then(Value::as_array)
                .ok_or("会话列表格式不兼容")?;
            for thread in rows {
                if !thread
                    .get("parentThreadId")
                    .unwrap_or(&Value::Null)
                    .is_null()
                    || thread.get("ephemeral").and_then(Value::as_bool) == Some(true)
                {
                    continue;
                }
                let item = candidate(thread)?;
                if !self.tasks.iter().any(|t| t.id == item.id) {
                    result.push(item);
                }
            }
            cursor = response
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let Some(next) = cursor.as_ref() else {
                break;
            };
            if !seen.insert(next.clone()) {
                return Err("Codex 会话分页异常，请重新连接".into());
            }
            if seen.len() >= 100 {
                return Err("会话列表过大，当前版本尚不支持此规模".into());
            }
        }
        Ok(result)
    }

    pub async fn add_task(&mut self, id: String) -> Result<(), String> {
        if self.tasks.iter().any(|task| task.id == id) {
            return Ok(());
        }
        let response = self
            .rpc()
            .await?
            .request("thread/read", json!({"threadId":id,"includeTurns":false}))
            .await?;
        let item = candidate(&response["thread"])?;
        if item.id != id {
            return Err("Codex 返回了不同的会话，未添加任务".into());
        }
        let task = TaskRecord::new(item.id, item.title, item.cwd);
        self.store.save_task(&task).map_err(|e| e.to_string())?;
        self.tasks.push(task);
        Ok(())
    }

    pub fn set_enabled(&mut self, id: &str, enabled: bool) -> Result<(), String> {
        let saved = self
            .tasks
            .iter_mut()
            .find(|t| t.id == id)
            .ok_or("未找到任务")?;
        let mut task = saved.clone();
        scheduler::set_enabled(&mut task, enabled, now());
        if enabled {
            scheduler::block(&mut task, TAKEOVER_BLOCKED, now());
        }
        self.store.save_task(&task).map_err(|e| e.to_string())?;
        *saved = task;
        Ok(())
    }

    pub fn set_paused(&mut self, paused: bool) -> Result<(), String> {
        self.store.set_paused(paused).map_err(|e| e.to_string())?;
        self.paused = paused;
        Ok(())
    }

    pub fn remove_task(&mut self, id: &str) -> Result<(), String> {
        self.store.remove_task(id).map_err(|e| e.to_string())?;
        self.tasks.retain(|t| t.id != id);
        Ok(())
    }

    pub fn reconnect(&mut self) -> Result<(), String> {
        self.rpc = None;
        self.store
            .recover_after_restart(now())
            .map_err(|e| e.to_string())?;
        self.tasks = self.store.load_tasks().map_err(|e| e.to_string())?;
        self.snapshot.connection = "checking".into();
        self.snapshot.connection_message = "正在重新连接".into();
        Ok(())
    }

    pub async fn login_url(&mut self) -> Result<String, String> {
        let result = self
            .rpc()
            .await?
            .request("account/login/start", json!({"type":"chatgpt"}))
            .await?;
        let raw = result
            .get("authUrl")
            .and_then(Value::as_str)
            .ok_or("未取得官方登录链接")?;
        let url = url::Url::parse(raw).map_err(|_| "登录链接格式无效")?;
        if url.scheme() != "https"
            || !matches!(url.host_str(), Some("auth.openai.com" | "chatgpt.com"))
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err("登录链接不是受支持的官方地址".into());
        }
        Ok(url.into())
    }
}
