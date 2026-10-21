use crate::{
    protocol::{self, Rpc},
    quota::{self, QuotaTelemetry},
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

pub const COMPATIBILITY: &str = "原聊天尚未接入。请在连接设置中启用正常启动接入，再从平时的图标重开 Codex。接入后，每个聊天仍需单独开启续跑。桌面连接目前为实验功能。";
const TAKEOVER_BLOCKED: &str = "此条目来自历史列表，仅监测；原聊天续跑需使用桌面连接";

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
    pub desktop_connected: bool,
    pub paused_all: bool,
    pub checked_at: Option<i64>,
    pub quota: Option<QuotaTelemetry>,
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
            desktop_connected: false,
            paused_all: false,
            checked_at: None,
            quota: None,
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
    pub managed: crate::managed::ManagedHub,
    pub desktop: crate::desktop_bridge::DesktopHub,
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
            desktop: crate::desktop_bridge::DesktopHub::new(
                path.parent().ok_or("应用数据目录无效")?,
            ),
            managed: crate::managed::ManagedHub::open(
                path.parent().ok_or("应用数据目录无效")?,
                path,
            )?,
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
            runtime.connection_failed(&error);
        }
        self.publish(&mut runtime).await;
    }

    pub async fn combined_snapshot(&self) -> Result<Snapshot, String> {
        let mut snapshot = self.snapshot.read().await.clone();
        let managed = self.managed.views().await?;
        if managed.iter().any(|task| task.connected) {
            snapshot.quota = managed
                .iter()
                .filter(|task| task.connected)
                .filter_map(|task| task.quota.clone())
                .max_by_key(|quota| quota.observed_at);
        }
        snapshot
            .tasks
            .extend(managed.into_iter().map(|task| TaskView {
                mode: "managed".into(),
                can_enable: task.can_enable,
                id: task.id,
                title: task.title,
                cwd: task.cwd,
                enabled: task.auto_resume,
                state: task.state,
                detail: task.detail,
                resets_at: task.resets_at,
                events: task.events,
            }));
        let desktop = self.desktop.snapshot().await;
        snapshot.desktop_connected = desktop.connected;
        if desktop.connected {
            snapshot.connection = "ready".into();
            snapshot.connection_message = "已连接原 Codex 聊天 · 实验连接".into();
            snapshot.codex_version = None;
            snapshot.account_label = None;
            snapshot.quota = desktop.quota;
            snapshot.checked_at = Some(now());
            snapshot
                .tasks
                .retain(|task| !desktop.tasks.iter().any(|native| native.id == task.id));
            snapshot.tasks.extend(desktop.tasks);
            snapshot.compatibility_message = Some(
                "已接入原 Codex 聊天。在 Codex 中打开聊天后，可单独开启续跑；只处理接入后确认的额度中断。桌面连接目前为实验功能。".into(),
            );
        }
        Ok(snapshot)
    }

    pub async fn is_managed(&self, id: &str) -> Result<bool, String> {
        Ok(self.managed.views().await?.iter().any(|task| task.id == id))
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
        status: "历史监测".into(),
        eligible: false,
        reason: Some(TAKEOVER_BLOCKED.into()),
    })
}

impl Runtime {
    fn connection_failed(&mut self, error: &str) {
        self.snapshot.connection = if error.starts_with("missing:") {
            "missing"
        } else {
            "error"
        }
        .into();
        self.snapshot.connection_message = error.trim_start_matches("missing:").into();
        self.snapshot.account_label = None;
        self.snapshot.quota = None;
        self.rpc = None;
        for task in &mut self.tasks {
            scheduler::block(task, "连接中断，恢复后将重新核对，不会重放操作", now());
        }
        for task in &self.tasks {
            if self.store.save_task(task).is_err() {
                self.snapshot.connection_message =
                    "连接中断，且本地状态保存失败，请检查磁盘空间".into();
                break;
            }
        }
    }

    async fn rpc(&mut self) -> Result<&mut Rpc, String> {
        if self.rpc.is_none() {
            let rpc = Rpc::connect().await?;
            self.snapshot.codex_version = Some(rpc.version.clone());
            self.rpc = Some(rpc);
        }
        self.rpc.as_mut().ok_or("Codex 未连接".into())
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let response = match self.rpc().await {
            Ok(rpc) => rpc.request(method, params).await,
            Err(error) => Err(error),
        };
        if let Err(error) = &response {
            if self.rpc.as_ref().is_none_or(|rpc| !rpc.is_usable()) {
                // Clear a failed transport immediately, including UI-initiated
                // reads. Never retry the request on a replacement connection.
                self.connection_failed(error);
            }
        }
        response
    }

    async fn refresh(&mut self) -> Result<(), String> {
        let account = self
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
            self.snapshot.quota = None;
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
        // Account telemetry remains current while task monitoring is paused.
        // This tolerant display view never supplies the execution safety gate.
        self.snapshot.quota = None;
        match self.request("account/rateLimits/read", json!({})).await {
            Ok(response) => {
                self.snapshot.quota = quota::parse_telemetry(&response, now());
                if self.snapshot.quota.is_none() {
                    self.snapshot.connection_message = "会话已连接，额度数据暂不可用".into();
                }
            }
            Err(error) => {
                if self.rpc.is_none() {
                    return Err(error);
                }
                self.snapshot.connection_message = "会话已连接，额度暂时无法读取".into();
            }
        }
        if self.paused {
            return Ok(());
        }
        for index in 0..self.tasks.len() {
            if !self.tasks[index].enabled {
                continue;
            }
            let id = self.tasks[index].id.clone();
            let response = self
                .request("thread/read", json!({"threadId":id,"includeTurns":true}))
                .await;
            if self.rpc.is_none() {
                return Err(response.err().unwrap_or_else(|| "Codex 连接已关闭".into()));
            }
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
            let response = self.request("thread/list", json!({"cursor":cursor,"limit":100,"sortKey":"updated_at","sourceKinds":["cli","vscode","appServer","exec"],"useStateDbOnly":true})).await?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{split, AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf},
        time::{timeout, Duration},
    };

    type Reader = BufReader<ReadHalf<DuplexStream>>;
    type Writer = WriteHalf<DuplexStream>;

    fn transport() -> (Rpc, Reader, Writer) {
        let (client, server) = tokio::io::duplex(64 * 1024);
        let (client_read, client_write) = split(client);
        let (server_read, server_write) = split(server);
        (
            Rpc::from_io(None, client_write, client_read, "fixture".into()),
            BufReader::new(server_read),
            server_write,
        )
    }

    async fn request(reader: &mut Reader, expected_method: &str) -> Value {
        let mut line = String::new();
        let bytes = timeout(Duration::from_secs(2), reader.read_line(&mut line))
            .await
            .expect("fixture request timed out")
            .unwrap();
        assert_ne!(bytes, 0);
        let value: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["method"], expected_method);
        value
    }

    async fn respond(writer: &mut Writer, request: &Value, result: Value) {
        let response = json!({"id":request["id"],"result":result});
        writer
            .write_all(format!("{response}\n").as_bytes())
            .await
            .unwrap();
    }

    async fn reject(writer: &mut Writer, request: &Value) {
        let response =
            json!({"id":request["id"],"error":{"code":-32600,"message":"fixture rejection"}});
        writer
            .write_all(format!("{response}\n").as_bytes())
            .await
            .unwrap();
    }

    async fn account(reader: &mut Reader, writer: &mut Writer) {
        let request = request(reader, "account/read").await;
        respond(
            writer,
            &request,
            json!({"account":{"type":"chatgpt","planType":"plus"}}),
        )
        .await;
    }

    async fn finish_task(reader: &mut Reader, writer: &mut Writer, id: &str) {
        let request = request(reader, "thread/read").await;
        assert_eq!(request["params"]["threadId"], id);
        respond(
            writer,
            &request,
            json!({"thread":{"id":id,"turns":[{"id":"completed-turn","status":"completed","error":null}]}}),
        )
        .await;
    }

    async fn state(rpc: Rpc) -> Arc<AppState> {
        let state = AppState::open(Path::new(":memory:")).unwrap();
        {
            let mut runtime = state.runtime.lock().await;
            runtime.rpc = Some(rpc);
            for id in ["first", "second"] {
                let mut task = TaskRecord::new(id.into(), id.into(), "/fixture".into());
                scheduler::set_enabled(&mut task, true, 1);
                runtime.store.save_task(&task).unwrap();
                runtime.tasks.push(task);
            }
        }
        state
    }

    fn telemetry_response() -> Value {
        json!({"rateLimitsByLimitId":{"codex":{
            "limitId":"codex",
            "primary":{"usedPercent":37,"windowDurationMins":300,"resetsAt":250}
        }}})
    }

    #[tokio::test]
    async fn quota_rpc_rejection_does_not_disconnect_or_prevent_task_reads() {
        let (rpc, mut reader, mut writer) = transport();
        let state = state(rpc).await;
        state.runtime.lock().await.snapshot.quota =
            quota::parse_telemetry(&telemetry_response(), 1);
        let server = tokio::spawn(async move {
            account(&mut reader, &mut writer).await;
            let quota = request(&mut reader, "account/rateLimits/read").await;
            reject(&mut writer, &quota).await;
            finish_task(&mut reader, &mut writer, "first").await;
            finish_task(&mut reader, &mut writer, "second").await;
        });
        state.refresh().await;
        let snapshot = state.snapshot.read().await;
        assert_eq!(snapshot.connection, "ready");
        assert!(snapshot.connection_message.contains("额度暂时无法读取"));
        assert!(snapshot.quota.is_none());
        assert!(snapshot
            .tasks
            .iter()
            .all(|task| task.state == TaskState::Completed));
        assert!(state.runtime.lock().await.rpc.as_ref().unwrap().is_usable());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn missing_thread_does_not_disconnect_other_tasks() {
        let (rpc, mut reader, mut writer) = transport();
        let state = state(rpc).await;
        let server = tokio::spawn(async move {
            account(&mut reader, &mut writer).await;
            let quota = request(&mut reader, "account/rateLimits/read").await;
            respond(&mut writer, &quota, json!({})).await;
            let missing = request(&mut reader, "thread/read").await;
            assert_eq!(missing["params"]["threadId"], "first");
            reject(&mut writer, &missing).await;
            finish_task(&mut reader, &mut writer, "second").await;
        });
        state.refresh().await;
        let snapshot = state.snapshot.read().await;
        assert_eq!(snapshot.connection, "ready");
        assert_eq!(snapshot.tasks[0].state, TaskState::NeedsAttention);
        assert_eq!(snapshot.tasks[1].state, TaskState::Completed);
        assert!(state.runtime.lock().await.rpc.as_ref().unwrap().is_usable());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn broken_quota_transport_is_cleared_before_the_next_refresh() {
        let (rpc, mut reader, mut writer) = transport();
        let state = state(rpc).await;
        state.runtime.lock().await.snapshot.quota =
            quota::parse_telemetry(&telemetry_response(), 1);
        let server = tokio::spawn(async move {
            account(&mut reader, &mut writer).await;
            request(&mut reader, "account/rateLimits/read").await;
            writer.shutdown().await.unwrap();
            drop(writer);
            let mut next = String::new();
            assert_eq!(reader.read_line(&mut next).await.unwrap(), 0);
        });
        state.refresh().await;
        assert!(state.runtime.lock().await.rpc.is_none());
        assert_eq!(state.snapshot.read().await.connection, "error");
        assert!(state.snapshot.read().await.quota.is_none());
        server.await.unwrap();

        // A fresh transport reconciles from account/history reads. The failed
        // quota request is not replayed by the transport itself.
        let (rpc, mut reader, mut writer) = transport();
        state.runtime.lock().await.rpc = Some(rpc);
        let server = tokio::spawn(async move {
            account(&mut reader, &mut writer).await;
            let quota = request(&mut reader, "account/rateLimits/read").await;
            respond(&mut writer, &quota, json!({})).await;
            finish_task(&mut reader, &mut writer, "first").await;
            finish_task(&mut reader, &mut writer, "second").await;
        });
        state.refresh().await;
        let snapshot = state.snapshot.read().await;
        assert_eq!(snapshot.connection, "ready");
        assert!(snapshot
            .tasks
            .iter()
            .all(|task| task.state == TaskState::Completed));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn interactive_list_transport_failure_immediately_clears_connection() {
        let (rpc, mut reader, writer) = transport();
        let state = state(rpc).await;
        let server = tokio::spawn(async move {
            request(&mut reader, "thread/list").await;
            drop(writer);
        });
        let mut runtime = state.runtime.lock().await;
        assert!(runtime.list_candidates().await.is_err());
        assert!(runtime.rpc.is_none());
        assert_eq!(runtime.snapshot.connection, "error");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn paused_monitoring_still_refreshes_quota_without_reading_tasks() {
        let (rpc, mut reader, mut writer) = transport();
        let state = state(rpc).await;
        state.runtime.lock().await.paused = true;
        let server = tokio::spawn(async move {
            account(&mut reader, &mut writer).await;
            let quota = request(&mut reader, "account/rateLimits/read").await;
            respond(&mut writer, &quota, telemetry_response()).await;
        });
        state.refresh().await;
        let snapshot = state.snapshot.read().await;
        assert_eq!(snapshot.connection, "ready");
        assert!(snapshot.paused_all);
        let quota = snapshot.quota.as_ref().expect("fresh quota while paused");
        assert_eq!(quota.buckets[0].windows[0].used_percent, Some(37));
        assert!(quota.observed_at > 1);
        assert!(snapshot
            .tasks
            .iter()
            .all(|task| task.state == TaskState::Reconciling));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn login_loss_clears_previously_visible_quota() {
        let (rpc, mut reader, mut writer) = transport();
        let state = state(rpc).await;
        state.runtime.lock().await.snapshot.quota =
            quota::parse_telemetry(&telemetry_response(), 1);
        let server = tokio::spawn(async move {
            let account = request(&mut reader, "account/read").await;
            respond(&mut writer, &account, json!({"account":null})).await;
        });
        state.refresh().await;
        let snapshot = state.snapshot.read().await;
        assert_eq!(snapshot.connection, "login_required");
        assert!(snapshot.quota.is_none());
        server.await.unwrap();
    }
}
