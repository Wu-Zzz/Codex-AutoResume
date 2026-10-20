//! A private, warm Codex execution session. Nothing in the discovery adapter can
//! grant this module ownership. Only a successful thread/start through this
//! generation's authenticated native-TUI gateway establishes a managed thread.
use crate::{
    protocol, quota,
    scheduler::{
        self, Decision, Ownership, QuotaSnapshot, SafetyGate, TaskRecord, TaskState, TaskView,
        TurnObservation, TurnStatus,
    },
    store::{ClaimResult, ManagedMetadata, Store},
};
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    net::TcpListener,
    sync::{mpsc, oneshot, Mutex, OwnedSemaphorePermit, Semaphore},
};
use tokio_tungstenite::{
    accept_hdr_async_with_config,
    tungstenite::{
        handshake::server::{Request, Response},
        http::StatusCode,
        protocol::WebSocketConfig,
        Message,
    },
};
use uuid::Uuid;

const MAX_FRAME: usize = 16 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_RETIRED_READS: usize = 256;
const CONTINUE: &str = "继续刚才因额度限制中断的原任务。先核对上一步执行状态，保持原任务范围、权限和配置；如需审批或补充信息，请停下等待用户。";
pub const TOKEN_ENV: &str = "CODEX_AUTORESUME_CHANNEL_TOKEN";

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// Contains a local IPC capability, never an OpenAI credential. Do not serialize
/// this structure to the webview or log it. The terminal launcher owns delivery.
#[derive(Clone)]
pub struct LaunchSpec {
    pub id: String,
    pub executable: PathBuf,
    pub cwd: PathBuf,
    pub endpoint: String,
    pub token_env: String,
    pub token: String,
    pub codex_home: PathBuf,
    pub thread_id: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedTaskView {
    /// Stable local launch id, including before Codex creates its thread.
    pub id: String,
    pub thread_id: Option<String>,
    pub title: String,
    pub cwd: String,
    pub auto_resume: bool,
    pub connected: bool,
    pub can_enable: bool,
    pub state: TaskState,
    pub detail: String,
    pub resets_at: Option<i64>,
    pub events: Vec<scheduler::TaskEvent>,
    pub quota: Option<quota::QuotaTelemetry>,
}

impl ManagedTaskView {
    fn stored(task: &TaskRecord, meta: &ManagedMetadata) -> Self {
        let view = TaskView::from(task);
        Self {
            id: meta.launch_id.clone(),
            thread_id: Some(task.id.clone()),
            title: view.title,
            cwd: view.cwd,
            auto_resume: meta.auto_resume,
            connected: false,
            can_enable: false,
            state: view.state,
            detail: view.detail,
            resets_at: view.resets_at,
            events: view.events,
            quota: None,
        }
    }
}

type Reply<T> = oneshot::Sender<Result<T, String>>;
enum Control {
    LaunchConfirmed,
    View(Reply<ManagedTaskView>),
    Enable(bool, Reply<()>),
    Pause(bool, Reply<()>),
    Remove(Reply<()>),
}
enum Event {
    Connected {
        epoch: String,
        output: mpsc::Sender<Value>,
    },
    Disconnected(String),
    Client(String, Value),
    Backend(Result<Value, String>),
}

pub struct ManagedHub {
    home: PathBuf,
    db_path: PathBuf,
    handles: Mutex<HashMap<String, mpsc::Sender<Control>>>,
    automatic_slot: Arc<Semaphore>,
    paused: Arc<AtomicBool>,
}

impl ManagedHub {
    pub fn open(app_data_dir: &Path, db_path: &Path) -> Result<Self, String> {
        let home = app_data_dir.join("managed-codex");
        std::fs::create_dir_all(&home).map_err(|_| "无法创建托管 Codex 目录")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700))
                .map_err(|_| "无法保护托管 Codex 目录")?;
        }
        let mut store = Store::open(db_path).map_err(|e| e.to_string())?;
        // Persisted consent is not a live lease. No cold resume occurs here.
        for (mut task, mut meta) in store.load_managed_tasks().map_err(|e| e.to_string())? {
            meta.auto_resume = false;
            task.enabled = false;
            task.needs_reconciliation = true;
            task.failed_turn_id = None;
            task.resets_at = None;
            task.set_status(
                TaskState::NeedsAttention,
                "托管进程已结束，请启动新任务；未自动重放",
                now(),
            );
            store
                .save_managed_task(&task, &meta)
                .map_err(|e| e.to_string())?;
        }
        let paused = store.is_paused().map_err(|e| e.to_string())?;
        Ok(Self {
            home,
            db_path: db_path.to_owned(),
            handles: Mutex::new(HashMap::new()),
            automatic_slot: Arc::new(Semaphore::new(1)),
            paused: Arc::new(AtomicBool::new(paused)),
        })
    }

    pub async fn start(&self, cwd: String, auto_resume: bool) -> Result<LaunchSpec, String> {
        let cwd = std::fs::canonicalize(cwd).map_err(|_| "请选择存在的项目文件夹")?;
        if !cwd.is_dir() {
            return Err("请选择项目文件夹".into());
        }
        let executable = protocol::executable().ok_or("未找到官方 Codex CLI")?;
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(|_| "无法创建本地连接")?;
        let endpoint = format!(
            "ws://{}",
            listener.local_addr().map_err(|_| "无法读取本地连接")?
        );
        let spec = LaunchSpec {
            id: Uuid::new_v4().to_string(),
            executable: executable.clone(),
            cwd: cwd.clone(),
            endpoint,
            token_env: TOKEN_ENV.into(),
            token: Uuid::new_v4().to_string(),
            codex_home: self.home.clone(),
            thread_id: None,
        };
        let mut command = protocol::command(&executable)?;
        command
            .args(["app-server", "--listen", "stdio://"])
            .env("CODEX_HOME", &self.home)
            .current_dir(&cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut core = Core::new(
            spec.clone(),
            auto_resume,
            Store::open(&self.db_path).map_err(|e| e.to_string())?,
        );
        let (events, mut receiver) = mpsc::channel(128);
        let (control, mut commands) = mpsc::channel(32);
        let gateway = tokio::spawn(gateway(listener, spec.token.clone(), events.clone()));
        let slot = Arc::clone(&self.automatic_slot);
        let paused = Arc::clone(&self.paused);
        tokio::spawn(async move {
            // Native remote TUI skips onboarding. The terminal launcher first
            // runs official login in this private home. Do not create a backend
            // with cached unauthenticated state before that login has finished.
            let Some(client) = wait_for_client(&mut core, &mut receiver, &mut commands).await
            else {
                gateway.abort();
                return;
            };
            match command.spawn() {
                Ok(mut child) => {
                    if let (Some(input), Some(output)) = (child.stdin.take(), child.stdout.take()) {
                        let reader = tokio::spawn(read_backend(output, events));
                        run_actor(core, input, receiver, commands, slot, paused, Some(client))
                            .await;
                        reader.abort();
                    }
                    let _ = child.kill().await;
                }
                Err(_) => {
                    core.revoke("无法启动托管 Codex，请检查官方 CLI 安装");
                    dead_actor(core, commands).await;
                }
            }
            gateway.abort();
        });
        self.handles.lock().await.insert(spec.id.clone(), control);
        Ok(spec)
    }

    pub async fn views(&self) -> Result<Vec<ManagedTaskView>, String> {
        let store = Store::open(&self.db_path).map_err(|e| e.to_string())?;
        let mut views: HashMap<_, _> = store
            .load_managed_tasks()
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|(t, m)| (m.launch_id.clone(), ManagedTaskView::stored(&t, &m)))
            .collect();
        let handles = self.handles.lock().await.clone();
        for (id, handle) in handles {
            let (reply, recv) = oneshot::channel();
            if handle.send(Control::View(reply)).await.is_ok() {
                if let Ok(Ok(view)) = recv.await {
                    views.insert(id, view);
                }
            }
        }
        let mut views: Vec<_> = views.into_values().collect();
        views.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(views)
    }

    pub async fn confirm_launch(&self, id: &str) -> Result<(), String> {
        self.handle(id)
            .await?
            .send(Control::LaunchConfirmed)
            .await
            .map_err(|_| "托管连接已结束".into())
    }

    pub async fn set_auto_resume(&self, id: &str, enabled: bool) -> Result<(), String> {
        let handle = self.handle(id).await?;
        let (reply, recv) = oneshot::channel();
        handle
            .send(Control::Enable(enabled, reply))
            .await
            .map_err(|_| "托管进程已结束")?;
        recv.await.map_err(|_| "托管进程已结束")?
    }

    pub async fn pause_all(&self, paused: bool) -> Result<(), String> {
        Store::open(&self.db_path)
            .map_err(|e| e.to_string())?
            .set_paused(paused)
            .map_err(|e| e.to_string())?;
        self.paused.store(paused, Ordering::SeqCst);
        for handle in self.handles.lock().await.values() {
            let (reply, recv) = oneshot::channel();
            if handle.send(Control::Pause(paused, reply)).await.is_ok() {
                let _ = recv.await;
            }
        }
        Ok(())
    }

    pub async fn remove(&self, id: &str) -> Result<(), String> {
        let handle = self.handles.lock().await.get(id).cloned();
        if let Some(handle) = handle {
            let (reply, recv) = oneshot::channel();
            if handle.send(Control::Remove(reply)).await.is_ok() {
                recv.await.map_err(|_| "托管进程已结束")??;
            }
            self.handles.lock().await.remove(id);
        }
        Store::open(&self.db_path)
            .map_err(|e| e.to_string())?
            .remove_managed_task(id)
            .map_err(|e| e.to_string())
    }

    async fn handle(&self, id: &str) -> Result<mpsc::Sender<Control>, String> {
        self.handles
            .lock()
            .await
            .get(id)
            .cloned()
            .ok_or("托管进程已结束；请启动新任务".into())
    }
}

#[derive(Clone)]
enum Purpose {
    Client {
        epoch: String,
        original_id: Value,
        method: String,
    },
    StartAccount {
        epoch: String,
        original_id: Value,
        params: Value,
    },
    Account,
    Quota,
    TelemetryAccount,
    Telemetry,
    Check {
        failed: String,
        revision: u64,
    },
    Continue {
        failed: String,
    },
}

impl Purpose {
    fn is_read_only(&self) -> bool {
        match self {
            Self::Client { method, .. } => read_only_client_method(method),
            // StartAccount sends only account/read. A late reply must never
            // trigger the thread/start that its timed-out caller was awaiting.
            Self::StartAccount { .. }
            | Self::Account
            | Self::Quota
            | Self::TelemetryAccount
            | Self::Telemetry
            | Self::Check { .. } => true,
            Self::Continue { .. } => false,
        }
    }
}

struct Pending {
    purpose: Purpose,
    deadline: Instant,
}
enum Action {
    Backend(Value),
    Client(Value),
}

struct Core {
    spec: LaunchSpec,
    store: Store,
    task: Option<TaskRecord>,
    meta: Option<ManagedMetadata>,
    initial_consent: bool,
    pending: HashMap<String, Pending>,
    retired_reads: VecDeque<String>,
    server_requests: HashSet<String>,
    client_epoch: Option<String>,
    initialize_result: Option<Value>,
    initial_params: Option<Value>,
    turn_configuration: Option<String>,
    initialized: bool,
    created: bool,
    valid: bool,
    launch_detail: String,
    revision: u64,
    next_id: u64,
    quota: Option<QuotaSnapshot>,
    telemetry: Option<quota::QuotaTelemetry>,
    next_telemetry: i64,
    next_check: i64,
    active: bool,
    live_status_idle: bool,
    account_chatgpt: bool,
    automatic_permit: Option<OwnedSemaphorePermit>,
}

impl Core {
    fn new(spec: LaunchSpec, initial_consent: bool, store: Store) -> Self {
        Self {
            spec,
            store,
            task: None,
            meta: None,
            initial_consent,
            pending: HashMap::new(),
            retired_reads: VecDeque::new(),
            server_requests: HashSet::new(),
            client_epoch: None,
            initialize_result: None,
            initial_params: None,
            turn_configuration: None,
            initialized: false,
            created: false,
            valid: true,
            launch_detail: "正在打开官方 Codex 终端".into(),
            revision: 0,
            next_id: 1,
            quota: None,
            telemetry: None,
            next_telemetry: 0,
            next_check: 0,
            active: false,
            live_status_idle: false,
            account_chatgpt: false,
            automatic_permit: None,
        }
    }

    fn save(&mut self) -> Result<(), String> {
        if let (Some(task), Some(meta)) = (&self.task, &self.meta) {
            self.store
                .save_managed_task(task, meta)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn revoke(&mut self, reason: &str) {
        self.valid = false;
        self.launch_detail = reason.into();
        self.initial_consent = false;
        self.revision += 1;
        self.quota = None;
        if !self.active {
            self.automatic_permit = None;
        }
        if let Some(meta) = &mut self.meta {
            meta.auto_resume = false;
        }
        if let Some(task) = &mut self.task {
            task.enabled = false;
            task.failed_turn_id = None;
            task.resets_at = None;
            task.needs_reconciliation = true;
            task.set_status(TaskState::NeedsAttention, reason, now());
        }
    }

    fn pause(&mut self, reason: &str) {
        self.launch_detail = reason.into();
        self.initial_consent = false;
        self.revision += 1;
        self.quota = None;
        // Turning monitoring off does not stop an already running Codex turn.
        // Keep its queue slot until the terminal event (or an approval wait).
        if !self.active {
            self.automatic_permit = None;
        }
        if let Some(meta) = &mut self.meta {
            meta.auto_resume = false;
        }
        if let Some(task) = &mut self.task {
            task.enabled = false;
            task.failed_turn_id = None;
            task.resets_at = None;
            task.set_status(TaskState::Paused, reason, now());
        }
    }

    fn confirm_launch(&mut self) {
        // A pause or disconnect that occurred during startup takes precedence.
        if self.valid && self.revision == 0 {
            self.launch_detail = "终端已启动，请完成官方登录并输入任务".into();
        }
    }

    fn view(&self) -> ManagedTaskView {
        if let (Some(task), Some(meta)) = (&self.task, &self.meta) {
            let mut view = ManagedTaskView::stored(task, meta);
            view.connected = self.client_epoch.is_some();
            view.quota = self.telemetry.clone();
            view.can_enable = self.valid
                && self.initialized
                && self.client_epoch.is_some()
                && self.server_requests.is_empty()
                && !task.needs_reconciliation;
            return view;
        }
        ManagedTaskView {
            id: self.spec.id.clone(),
            thread_id: None,
            title: project_name(&self.spec.cwd),
            cwd: self.spec.cwd.to_string_lossy().into(),
            auto_resume: self.initial_consent,
            connected: self.client_epoch.is_some(),
            can_enable: false,
            state: if self.valid {
                TaskState::Reconciling
            } else {
                TaskState::NeedsAttention
            },
            detail: if self.valid && self.client_epoch.is_some() {
                "官方终端已连接，等待开始任务".into()
            } else {
                self.launch_detail.clone()
            },
            resets_at: None,
            events: vec![],
            quota: self.telemetry.clone(),
        }
    }

    fn enable(&mut self, enabled: bool) -> Result<(), String> {
        if enabled && !self.view().can_enable {
            return Err("需要仍在连接的官方终端和已核对的本轮托管上下文".into());
        }
        self.initial_consent = enabled;
        self.revision += 1;
        if let Some(meta) = &mut self.meta {
            meta.auto_resume = enabled;
        }
        if let Some(task) = &mut self.task {
            task.enabled = enabled;
            // Enabling never resurrects a failure from before explicit consent.
            task.failed_turn_id = None;
            task.resets_at = None;
            task.set_status(
                if self.active {
                    TaskState::Running
                } else if enabled {
                    TaskState::Monitoring
                } else {
                    TaskState::Paused
                },
                if enabled {
                    "已开启自动续跑，将从下一次明确额度中断开始守护"
                } else {
                    "自动续跑已关闭"
                },
                now(),
            );
        }
        self.save()
    }

    fn request(&mut self, method: &str, params: Value, purpose: Purpose) -> Action {
        let id = format!("ar:{}:{}", self.spec.id, self.next_id);
        self.next_id += 1;
        self.pending.insert(
            id.clone(),
            Pending {
                purpose,
                deadline: Instant::now() + REQUEST_TIMEOUT,
            },
        );
        Action::Backend(json!({"id":id,"method":method,"params":params}))
    }

    fn client(&mut self, epoch: &str, value: Value) -> Vec<Action> {
        if self.client_epoch.as_deref() != Some(epoch) {
            return vec![];
        }
        let id = value.get("id").cloned();
        let method = value.get("method").and_then(Value::as_str);
        // A response must match a server request shown in this same native UI.
        if method.is_none() {
            if let Some(id) = &id {
                if self.server_requests.remove(&id.to_string())
                    && (value.get("result").is_some() ^ value.get("error").is_some())
                {
                    return vec![Action::Backend(value)];
                }
            }
            self.revoke("收到无法识别的终端响应，自动续跑已关闭");
            return vec![];
        }
        let method = method.unwrap_or_default();
        if method == "initialized" && id.is_none() {
            if self.initialize_result.is_some() && !self.initialized {
                self.initialized = true;
                return vec![Action::Backend(json!({"method":"initialized","params":{}}))];
            }
            return vec![];
        }
        let Some(id) = id else {
            self.revoke("收到未知客户端通知");
            return vec![];
        };
        let reject = |message: &str| vec![Action::Client(rpc_error(id.clone(), message))];
        let mut params = value.get("params").cloned().unwrap_or_else(|| json!({}));
        if params.is_null() {
            params = json!({});
        }
        if !params.is_object() {
            return reject("请求参数必须为对象");
        }
        if method == "initialize" {
            if let Some(result) = &self.initialize_result {
                return vec![Action::Client(json!({"id":id,"result":result}))];
            }
            if self.pending.values().any(
                |p| matches!(&p.purpose, Purpose::Client { method, .. } if method == "initialize"),
            ) {
                return reject("连接初始化仍在进行");
            }
            params["clientInfo"] = json!({"name":"codex_autoresume","title":"Codex AutoResume","version":env!("CARGO_PKG_VERSION")});
        } else {
            if !self.initialized {
                return reject("请先初始化连接");
            }
            if !allowed_client_method(method) {
                self.revoke("此终端操作不在已验证协议范围内，自动续跑已关闭");
                return reject("当前托管模式不支持此操作");
            }
            if method == "account/login/start"
                && !matches!(
                    params.get("type").and_then(Value::as_str),
                    Some("chatgpt" | "chatgptDeviceCode")
                )
            {
                return reject("托管模式仅使用官方 ChatGPT 登录，不接收凭据或切换付费方式");
            }
            if let Some(thread_id) = params.get("threadId").and_then(Value::as_str) {
                if self.task.as_ref().map(|t| t.id.as_str()) != Some(thread_id) {
                    return reject("只能操作本次托管进程创建的会话；不接管历史会话");
                }
            } else if method.starts_with("turn/") || method == "thread/resume" {
                return reject("缺少本次托管会话标识");
            }
            if method == "thread/start" {
                if self.created || self.pending.values().any(|p| matches!(&p.purpose,
                    Purpose::StartAccount {..}) || matches!(&p.purpose, Purpose::Client {method,..} if method=="thread/start")) {
                    return reject("每个托管终端只管理一个新任务");
                }
                if params.get("ephemeral").and_then(Value::as_bool) == Some(true)
                    || params.get("dynamicTools").is_some_and(|v| !v.is_null())
                    || params.get("environments").is_some_and(|v| !v.is_null())
                {
                    return reject("此托管模式不支持临时会话、动态工具或远程执行环境");
                }
                // Only the explicit project selection is enforced; permissions,
                // model and instructions pass through the native client unchanged.
                if params
                    .get("cwd")
                    .and_then(Value::as_str)
                    .is_some_and(|s| Path::new(s) != self.spec.cwd)
                {
                    return reject("终端工作目录与所选项目不同");
                }
                params["cwd"] = Value::String(self.spec.cwd.to_string_lossy().into_owned());
                if !known_start_params(&params) {
                    self.revoke("新任务包含未验证参数");
                    return reject("新任务包含未验证参数");
                }
                self.initial_params = Some(params.clone());
            }
            if method == "thread/resume" {
                // Warm rejoin only. No overrides: cold resume may run goals.
                if !self.valid || !self.created {
                    return reject("此托管进程的执行上下文已失效");
                }
                if !warm_resume_matches(&params, self.initial_params.as_ref()) {
                    return reject("重新打开终端包含配置覆盖，请启动新任务");
                }
            }
            if method == "thread/unsubscribe" {
                self.pause("终端已停止守护，自动续跑已关闭");
                return vec![Action::Client(json!({"id":id,"result":{}}))];
            }
            if matches!(method, "turn/start" | "turn/steer") {
                self.revision += 1;
                self.quota = None;
                if let Some(task) = &mut self.task {
                    task.failed_turn_id = None;
                    task.resets_at = None;
                }
                if (method == "turn/start" && !known_turn_params(&params))
                    || (method == "turn/steer" && !known_steer_params(&params))
                {
                    self.revoke("终端提交包含未验证配置，自动续跑已关闭");
                    return reject("提交包含未验证参数");
                }
                if method == "turn/steer"
                    && ["additionalContext", "responsesapiClientMetadata"]
                        .iter()
                        .any(|key| params.get(key).is_some_and(|value| !value.is_null()))
                {
                    self.revoke("此轮包含专用上下文设置，自动续跑已关闭");
                }
                if method == "turn/start" {
                    if [
                        "outputSchema",
                        "additionalContext",
                        "responsesapiClientMetadata",
                    ]
                    .iter()
                    .any(|key| params.get(key).is_some_and(|value| !value.is_null()))
                    {
                        self.revoke("此轮包含专用输出或上下文设置，自动续跑已关闭");
                    }
                    let mut configuration = params.clone();
                    if let Some(object) = configuration.as_object_mut() {
                        for key in ["input", "clientUserMessageId", "threadId"] {
                            object.remove(key);
                        }
                    }
                    let next = fingerprint(&configuration);
                    if self
                        .turn_configuration
                        .as_ref()
                        .is_some_and(|previous| previous != &next)
                    {
                        self.pause("用户执行配置已变化，请重新开启自动续跑");
                    }
                    self.turn_configuration = Some(next.clone());
                    if let Some(meta) = &mut self.meta {
                        meta.configuration_fingerprint = next;
                    }
                }
                self.active = true; // User submission reserves the writer before its response.
            }
            if matches!(method, "turn/interrupt" | "account/logout") {
                self.pause("用户已停止任务或退出登录，自动续跑已关闭");
            }
            if method == "thread/settings/update"
                || method.starts_with("thread/goal/") && method != "thread/goal/get"
            {
                self.revoke("任务配置或目标已更改，请启动新的托管任务");
            }
        }
        if method == "thread/start" {
            return vec![self.request(
                "account/read",
                json!({"refreshToken":false}),
                Purpose::StartAccount {
                    epoch: epoch.into(),
                    original_id: id,
                    params,
                },
            )];
        }
        vec![self.request(
            method,
            params,
            Purpose::Client {
                epoch: epoch.into(),
                original_id: id,
                method: method.into(),
            },
        )]
    }

    fn backend(
        &mut self,
        value: Value,
        paused: bool,
        slot: &Arc<Semaphore>,
    ) -> Result<Vec<Action>, String> {
        if value.get("method").is_some() {
            if let Some(id) = value.get("id") {
                let method = value
                    .get("method")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !matches!(
                    method,
                    "item/commandExecution/requestApproval"
                        | "item/fileChange/requestApproval"
                        | "item/tool/requestUserInput"
                        | "mcpServer/elicitation/request"
                        | "item/permissions/requestApproval"
                        | "applyPatchApproval"
                        | "execCommandApproval"
                ) {
                    self.revoke("此任务请求了未支持的客户端能力，自动续跑已关闭");
                    return Ok(vec![Action::Backend(rpc_error(
                        id.clone(),
                        "此托管客户端不提供凭据、动态工具或未知能力",
                    ))]);
                }
            }
            self.notification(&value)?;
            return Ok(if self.client_epoch.is_some() {
                vec![Action::Client(value)]
            } else {
                vec![]
            });
        }
        let Some(id) = value.get("id").and_then(Value::as_str) else {
            self.revoke("Codex 返回了无法识别的响应");
            return Ok(vec![]);
        };
        let Some(pending) = self.pending.remove(id) else {
            if let Some(index) = self.retired_reads.iter().position(|retired| retired == id) {
                // The caller already received its timeout. Consume this exact
                // ID once and discard every stale field without changing gates.
                self.retired_reads.remove(index);
                return Ok(vec![]);
            }
            self.revoke("Codex 返回了无法匹配的响应，已停止自动续跑");
            return Ok(vec![]);
        };
        let result = value.get("result");
        match pending.purpose {
            Purpose::StartAccount {
                epoch,
                original_id,
                params,
            } => {
                self.account_chatgpt = result
                    .and_then(|r| r.pointer("/account/type"))
                    .and_then(Value::as_str)
                    == Some("chatgpt");
                if self.client_epoch.as_deref() != Some(&epoch) {
                    return Ok(vec![]);
                }
                if !self.account_chatgpt {
                    self.pause("请先在官方终端使用 ChatGPT 登录，不切换付费方式");
                    return Ok(vec![Action::Client(rpc_error(
                        original_id,
                        "此托管模式仅支持官方 ChatGPT 登录",
                    ))]);
                }
                Ok(vec![self.request(
                    "thread/start",
                    params,
                    Purpose::Client {
                        epoch,
                        original_id,
                        method: "thread/start".into(),
                    },
                )])
            }
            Purpose::Client {
                epoch,
                original_id,
                method,
            } => {
                if let Some(result) = result {
                    match method.as_str() {
                        "initialize" => self.initialize_result = Some(result.clone()),
                        "thread/start" => self.created(result)?,
                        "turn/start" => {
                            if let Some(turn) = result.get("turn") {
                                self.observe(turn)?;
                            } else {
                                self.revoke("Codex 未确认用户轮次");
                            }
                        }
                        "account/logout" => self.account_chatgpt = false,
                        "account/read" => {
                            self.account_chatgpt =
                                result.pointer("/account/type").and_then(Value::as_str)
                                    == Some("chatgpt");
                            if !self.account_chatgpt {
                                self.telemetry = None;
                            }
                        }
                        "account/rateLimits/read" => self.update_telemetry(result),
                        _ => {}
                    }
                } else if matches!(
                    method.as_str(),
                    "thread/start" | "turn/start" | "turn/steer"
                ) {
                    self.revoke("用户提交结果未能确认，自动续跑已关闭");
                }
                let mut response = value;
                response["id"] = original_id;
                Ok(if self.client_epoch.as_deref() == Some(&epoch) {
                    vec![Action::Client(response)]
                } else {
                    vec![]
                })
            }
            Purpose::Account => {
                self.account_chatgpt = result
                    .and_then(|r| r.pointer("/account/type"))
                    .and_then(Value::as_str)
                    == Some("chatgpt");
                if !self.account_chatgpt {
                    self.revoke("请在官方终端完成 ChatGPT 登录；不切换其他付费方式");
                    return Ok(vec![]);
                }
                Ok(vec![self.request(
                    "account/rateLimits/read",
                    json!({}),
                    Purpose::Quota,
                )])
            }
            Purpose::TelemetryAccount => {
                self.account_chatgpt = result
                    .and_then(|r| r.pointer("/account/type"))
                    .and_then(Value::as_str)
                    == Some("chatgpt");
                if self.account_chatgpt {
                    Ok(vec![self.request(
                        "account/rateLimits/read",
                        json!({}),
                        Purpose::Telemetry,
                    )])
                } else {
                    self.telemetry = None;
                    if self.created {
                        self.pause("托管账号未登录 ChatGPT，请在官方终端处理");
                    }
                    Ok(vec![])
                }
            }
            Purpose::Telemetry => {
                self.telemetry = None;
                if let Some(result) = result {
                    self.update_telemetry(result);
                }
                Ok(vec![])
            }
            Purpose::Quota => {
                let Some(result) = result else {
                    self.next_check = now() + 60;
                    return Ok(vec![]);
                };
                self.update_telemetry(result);
                let quota = match quota::parse_response(result, now()) {
                    Ok(q) => q,
                    Err(_) => {
                        self.revoke("额度返回值无法识别，自动续跑已关闭");
                        return Ok(vec![]);
                    }
                };
                self.quota = Some(quota.clone());
                let Some(task) = &mut self.task else {
                    return Ok(vec![]);
                };
                task.required_limit_ids = quota.buckets.iter().map(|b| b.id.clone()).collect();
                let decision = scheduler::evaluate(
                    task,
                    &quota,
                    now(),
                    &SafetyGate {
                        ownership: if self.valid {
                            Ownership::ExclusiveStopped
                        } else {
                            Ownership::Unknown
                        },
                        execution_configuration_preserved: self.valid,
                    },
                );
                scheduler::apply_decision(task, &decision, now());
                self.next_check = match decision {
                    Decision::WaitUntil(Some(reset)) => reset.saturating_add(2),
                    _ => now() + 60,
                };
                if let Decision::Submit { failed_turn_id } = decision {
                    if self.eligible(paused) {
                        let thread = self.task.as_ref().unwrap().id.clone();
                        return Ok(vec![self.request(
                            "thread/read",
                            json!({"threadId":thread,"includeTurns":true}),
                            Purpose::Check {
                                failed: failed_turn_id,
                                revision: self.revision,
                            },
                        )]);
                    }
                }
                Ok(vec![])
            }
            Purpose::Check { failed, revision } => {
                if !self.eligible(paused) || revision != self.revision {
                    return Ok(vec![]);
                }
                let Some(result) = result else {
                    self.revoke("恢复前无法核对当前会话");
                    return Ok(vec![]);
                };
                if !checked_failure(result, self.task.as_ref().unwrap(), &failed) {
                    self.revoke("会话已变化，已停止自动续跑");
                    return Ok(vec![]);
                }
                let Some(quota) = &self.quota else {
                    return Ok(vec![]);
                };
                if !matches!(
                    scheduler::evaluate(
                        self.task.as_ref().unwrap(),
                        quota,
                        now(),
                        &SafetyGate {
                            ownership: Ownership::ExclusiveStopped,
                            execution_configuration_preserved: true,
                        }
                    ),
                    Decision::Submit { .. }
                ) {
                    return Ok(vec![]);
                }
                let Ok(permit) = Arc::clone(slot).try_acquire_owned() else {
                    self.next_check = now() + 5;
                    return Ok(vec![]);
                };
                let thread = self.task.as_ref().unwrap().id.clone();
                self.save()?;
                if let ClaimResult::Claimed(intent) = self
                    .store
                    .claim_submission(&thread, &failed, now())
                    .map_err(|e| e.to_string())?
                {
                    self.task = self.store.get_task(&thread).map_err(|e| e.to_string())?;
                    self.automatic_permit = Some(permit);
                    self.active = true; // Reserve before writing, including before turn/started.
                    let client_id = format!("autoresume:{}:{}", self.spec.id, intent.id);
                    return Ok(vec![self.request(
                        "turn/start",
                        json!({
                            "threadId":thread, "clientUserMessageId":client_id,
                            "input":[{"type":"text","text":CONTINUE,"text_elements":[]}]
                        }),
                        Purpose::Continue { failed },
                    )]);
                }
                self.revoke("已存在续跑意图，未重复提交");
                Ok(vec![])
            }
            Purpose::Continue { failed } => {
                let thread = self.task.as_ref().ok_or("托管会话丢失")?.id.clone();
                if let Some(turn) = result.and_then(|r| r.get("turn")) {
                    if let Some(turn_id) = turn
                        .get("id")
                        .and_then(Value::as_str)
                        .filter(|id| !id.is_empty() && *id != failed)
                    {
                        // Notifications may arrive before the turn/start reply.
                        // A late inProgress acknowledgement must not erase an
                        // already observed terminal result from the same turn.
                        let latest = self
                            .task
                            .as_ref()
                            .filter(|task| task.observed_turn_id.as_deref() == Some(turn_id))
                            .and_then(|task| {
                                task.observed_turn_status
                                    .clone()
                                    .map(|status| TurnObservation {
                                        id: turn_id.into(),
                                        status,
                                        error_code: task.observed_error_code.clone(),
                                    })
                            });
                        self.store
                            .mark_submitted(&thread, &failed, turn_id, now())
                            .map_err(|e| e.to_string())?;
                        let observation =
                            latest.unwrap_or(turn_observation(turn).ok_or("续跑响应无法识别")?);
                        self.task = Some(
                            self.store
                                .reconcile_observation(&thread, &observation, now())
                                .map_err(|e| e.to_string())?,
                        );
                        self.active = observation.status == TurnStatus::InProgress;
                        return Ok(vec![]);
                    }
                }
                self.store
                    .mark_ambiguous(&thread, &failed, now())
                    .map_err(|e| e.to_string())?;
                self.task = self.store.get_task(&thread).map_err(|e| e.to_string())?;
                self.revoke("续跑提交结果不明，未自动重试");
                Ok(vec![])
            }
        }
    }

    fn created(&mut self, result: &Value) -> Result<(), String> {
        if self.created {
            self.revoke("出现重复任务创建响应");
            return Ok(());
        }
        let Some(thread) = result.get("thread") else {
            self.revoke("Codex 未返回新会话");
            return Ok(());
        };
        let Some(id) = thread
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        else {
            self.revoke("Codex 未返回新会话标识");
            return Ok(());
        };
        let valid = valid_configuration(result)
            && result
                .get("cwd")
                .and_then(Value::as_str)
                .is_some_and(|cwd| Path::new(cwd) == self.spec.cwd);
        let mut task = TaskRecord::new(
            id.into(),
            project_name(&self.spec.cwd),
            self.spec.cwd.to_string_lossy().into(),
        );
        task.enabled = self.initial_consent;
        task.needs_reconciliation = false;
        task.set_status(
            TaskState::Monitoring,
            "已连接官方终端，等待用户开始任务",
            now(),
        );
        let mut config = result.clone();
        config.as_object_mut().unwrap().remove("thread");
        self.meta = Some(ManagedMetadata {
            launch_id: self.spec.id.clone(),
            thread_id: id.into(),
            generation: self.spec.id.clone(),
            auto_resume: self.initial_consent,
            configuration_fingerprint: fingerprint(
                &json!({"effective":config, "request":self.initial_params}),
            ),
        });
        self.spec.thread_id = Some(id.into());
        self.task = Some(task);
        self.created = true;
        self.live_status_idle = true;
        if !valid {
            self.revoke("无法确认新任务的执行配置，自动续跑已关闭");
        }
        self.save()
    }

    fn update_telemetry(&mut self, result: &Value) {
        self.telemetry = quota::parse_telemetry(result, now()).map(|mut telemetry| {
            telemetry.source = "managed";
            telemetry
        });
    }

    fn notification(&mut self, value: &Value) -> Result<(), String> {
        let method = value
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let params = value.get("params").unwrap_or(&Value::Null);
        if let Some(id) = value.get("id") {
            // Server requests require explicit native-UI answers; this module
            // never manufactures approval, user input, tools or credentials.
            self.pause("Codex 需要审批或补充信息，请在官方终端处理");
            self.automatic_permit = None;
            self.server_requests.insert(id.to_string());
            return Ok(());
        }
        if !known_notification(method) {
            self.revoke("Codex 返回未验证的事件，自动续跑已关闭");
            return Ok(());
        }
        let relevant = params
            .get("threadId")
            .and_then(Value::as_str)
            .is_some_and(|id| self.task.as_ref().is_some_and(|task| task.id == id));
        if method == "account/updated" {
            self.account_chatgpt =
                params.get("authMode").and_then(Value::as_str) == Some("chatgpt");
            self.telemetry = None;
            self.next_telemetry = 0;
            if self.created {
                self.pause("登录状态已变化，请重新开启自动续跑");
            }
        }
        if method == "account/rateLimits/updated" {
            self.quota = None; // A sparse update can never establish recovery.
            self.next_check = self.next_check.min(now() + 2);
        }
        if method == "serverRequest/resolved" {
            if let Some(id) = params.get("requestId") {
                self.server_requests.remove(&id.to_string());
            }
        }
        if !relevant {
            return Ok(());
        }
        match method {
            "turn/started" | "turn/completed" => {
                if let Some(turn) = params.get("turn") {
                    self.observe(turn)?;
                } else {
                    self.revoke("Codex 轮次事件缺失");
                }
            }
            "thread/status/changed" => {
                let status = params.get("status").unwrap_or(&Value::Null);
                self.live_status_idle = status.get("type").and_then(Value::as_str) == Some("idle");
                match status.get("type").and_then(Value::as_str) {
                    Some("idle") => {}
                    Some("active") => match status.get("activeFlags").and_then(Value::as_array) {
                        Some(flags) if flags.is_empty() => {}
                        Some(flags)
                            if flags.iter().all(|flag| {
                                matches!(
                                    flag.as_str(),
                                    Some("waitingOnApproval" | "waitingOnUserInput")
                                )
                            }) =>
                        {
                            self.pause("Codex 需要审批或用户输入，请在官方终端处理");
                            self.automatic_permit = None;
                        }
                        _ => self.revoke("无法确认活动状态，自动续跑已关闭"),
                    },
                    _ => self.revoke("无法确认托管会话执行状态"),
                }
            }
            "thread/settings/updated" => {
                // The native user may change configuration during their turn.
                // Such an action invalidates unattended continuation; we never
                // rewrite it back to a stale snapshot.
                if self
                    .task
                    .as_ref()
                    .is_some_and(|t| t.failed_turn_id.is_some())
                {
                    self.revoke("额度等待期间执行配置已更改");
                }
            }
            "model/rerouted"
            | "thread/goal/updated"
            | "thread/goal/cleared"
            | "thread/closed"
            | "thread/archived"
            | "thread/deleted"
            | "thread/environment/disconnected"
            | "thread/environment/connected" => self.revoke("执行上下文已变化，自动续跑已关闭"),
            _ => {}
        }
        Ok(())
    }

    fn observe(&mut self, turn: &Value) -> Result<(), String> {
        let Some(observation) = turn_observation(turn) else {
            self.revoke("Codex 返回了无法识别的轮次");
            return Ok(());
        };
        self.active = observation.status == TurnStatus::InProgress;
        if !self.active {
            self.automatic_permit = None;
        }
        if observation.status == TurnStatus::Interrupted {
            self.pause("用户已停止任务，自动续跑已关闭");
        }
        if let Some(task) = &mut self.task {
            scheduler::observe_turn(task, &observation, now());
            // Monitor activity even when automatic execution is switched off.
            if !task.enabled && observation.status == TurnStatus::InProgress {
                task.set_status(
                    TaskState::Running,
                    "官方 Codex 正在执行；自动续跑已关闭",
                    now(),
                );
            } else if observation.status == TurnStatus::Completed {
                task.set_status(TaskState::Completed, "本轮已结束", now());
            }
        }
        self.next_check = now() + 2;
        Ok(())
    }

    fn eligible(&self, paused: bool) -> bool {
        !paused
            && self.valid
            && self.initialized
            && self.client_epoch.is_some()
            && self.server_requests.is_empty()
            && !self.active
            && self.live_status_idle
            && self
                .meta
                .as_ref()
                .is_some_and(|m| m.auto_resume && m.generation == self.spec.id)
            && self.task.as_ref().is_some_and(|t| {
                t.enabled
                    && !t.needs_reconciliation
                    && t.state == TaskState::WaitingQuota
                    && t.failed_turn_id.is_some()
            })
    }

    fn tick(&mut self, paused: bool) -> Result<Vec<Action>, String> {
        let expired: Vec<_> = self
            .pending
            .iter()
            .filter(|(_, p)| p.deadline <= Instant::now())
            .map(|(id, _)| id.clone())
            .collect();
        let mut actions = vec![];
        for id in expired {
            let pending = self.pending.remove(&id).unwrap();
            if pending.purpose.is_read_only() {
                // Bound memory across long-lived sessions. An evicted ID is
                // unknown again and therefore retains the fail-closed default.
                if self.retired_reads.len() == MAX_RETIRED_READS {
                    self.retired_reads.pop_front();
                }
                self.retired_reads.push_back(id);
            }
            match pending.purpose {
                Purpose::Continue { failed } => {
                    if let Some(task) = &self.task {
                        self.store
                            .mark_ambiguous(&task.id, &failed, now())
                            .map_err(|e| e.to_string())?;
                    }
                    self.revoke("续跑提交确认超时，未自动重试");
                }
                Purpose::Client {
                    epoch,
                    original_id,
                    method,
                } => {
                    if self.client_epoch.as_deref() == Some(&epoch) {
                        actions.push(Action::Client(rpc_error(original_id, "Codex 响应超时")));
                    }
                    if matches!(
                        method.as_str(),
                        "thread/start" | "turn/start" | "turn/steer"
                    ) {
                        self.revoke("用户提交确认超时，自动续跑已关闭");
                    }
                }
                Purpose::StartAccount {
                    epoch, original_id, ..
                } => {
                    if self.client_epoch.as_deref() == Some(&epoch) {
                        actions.push(Action::Client(rpc_error(original_id, "无法核对托管账号")));
                    }
                    self.pause("账号检查超时，未启动任务");
                }
                _ => self.next_check = now() + 60,
            }
        }
        if self.eligible(paused)
            && now() >= self.next_check
            && !self
                .pending
                .values()
                .any(|p| !matches!(p.purpose, Purpose::Client { .. }))
        {
            self.next_check = now() + 60;
            actions.push(self.request(
                "account/read",
                json!({"refreshToken":false}),
                Purpose::Account,
            ));
        } else if self.initialized
            && self.client_epoch.is_some()
            && now() >= self.next_telemetry
            && !self
                .pending
                .values()
                .any(|p| !matches!(p.purpose, Purpose::Client { .. }))
        {
            self.next_telemetry = now() + 60;
            actions.push(self.request(
                "account/read",
                json!({"refreshToken":false}),
                Purpose::TelemetryAccount,
            ));
        }
        Ok(actions)
    }
}

fn project_name(path: &Path) -> String {
    path.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "托管任务".into())
}
fn fingerprint(value: &Value) -> String {
    format!("{:x}", Sha256::digest(value.to_string().as_bytes()))
}
fn rpc_error(id: Value, message: &str) -> Value {
    json!({"id":id,"error":{"code":-32600,"message":message}})
}
fn valid_configuration(value: &Value) -> bool {
    const FIELDS: &[&str] = &[
        "thread",
        "model",
        "modelProvider",
        "serviceTier",
        "cwd",
        "runtimeWorkspaceRoots",
        "instructionSources",
        "approvalPolicy",
        "approvalsReviewer",
        "sandbox",
        "activePermissionProfile",
        "reasoningEffort",
        "multiAgentMode",
    ];
    let Some(object) = value.as_object() else {
        return false;
    };
    if object.keys().any(|key| !FIELDS.contains(&key.as_str())) {
        return false;
    }
    let strings = |value: &Value| {
        value
            .as_array()
            .is_some_and(|list| list.iter().all(Value::is_string))
    };
    let nullable_string = |value: &Value| value.is_null() || value.is_string();
    let approval = match value.get("approvalPolicy") {
        Some(Value::String(policy)) => {
            matches!(policy.as_str(), "untrusted" | "on-request" | "never")
        }
        Some(Value::Object(policy)) if policy.len() == 1 => policy
            .get("granular")
            .and_then(Value::as_object)
            .is_some_and(|p| {
                const FIELDS: &[&str] = &[
                    "sandbox_approval",
                    "rules",
                    "skill_approval",
                    "request_permissions",
                    "mcp_elicitations",
                ];
                p.len() == FIELDS.len()
                    && FIELDS
                        .iter()
                        .all(|field| p.get(*field).is_some_and(Value::is_boolean))
            }),
        _ => false,
    };
    let sandbox = value
        .get("sandbox")
        .and_then(Value::as_object)
        .is_some_and(|p| {
            let fields: &[&str] = match p.get("type").and_then(Value::as_str) {
                Some("dangerFullAccess") => &["type"],
                Some("readOnly") if p.get("networkAccess").is_some_and(Value::is_boolean) => {
                    &["type", "networkAccess"]
                }
                Some("externalSandbox")
                    if matches!(
                        p.get("networkAccess").and_then(Value::as_str),
                        Some("enabled" | "restricted")
                    ) =>
                {
                    &["type", "networkAccess"]
                }
                Some("workspaceWrite")
                    if p.get("writableRoots").is_some_and(strings)
                        && ["networkAccess", "excludeTmpdirEnvVar", "excludeSlashTmp"]
                            .iter()
                            .all(|k| p.get(*k).is_some_and(Value::is_boolean)) =>
                {
                    &[
                        "type",
                        "writableRoots",
                        "networkAccess",
                        "excludeTmpdirEnvVar",
                        "excludeSlashTmp",
                    ]
                }
                _ => return false,
            };
            p.len() == fields.len() && fields.iter().all(|key| p.contains_key(*key))
        });
    let profile = value.get("activePermissionProfile").is_none_or(|profile| {
        profile.is_null()
            || profile.as_object().is_some_and(|p| {
                p.len() == 2
                    && p.get("id").is_some_and(Value::is_string)
                    && p.get("extends").is_some_and(nullable_string)
            })
    });
    approval
        && sandbox
        && profile
        && ["model", "modelProvider", "cwd"].iter().all(|key| {
            value
                .get(key)
                .and_then(Value::as_str)
                .is_some_and(|s| !s.is_empty())
        })
        && value.get("instructionSources").is_some_and(strings)
        && value.get("runtimeWorkspaceRoots").is_none_or(strings)
        && matches!(
            value.get("approvalsReviewer").and_then(Value::as_str),
            Some("user" | "auto_review" | "guardian_subagent")
        )
        && value.get("reasoningEffort").is_some_and(nullable_string)
        && value.get("serviceTier").is_some_and(nullable_string)
        && value
            .get("multiAgentMode")
            .is_none_or(|v| v.as_str() == Some("explicitRequestOnly"))
}
fn known_turn_params(value: &Value) -> bool {
    const KNOWN: &[&str] = &[
        "threadId",
        "clientUserMessageId",
        "input",
        "responsesapiClientMetadata",
        "additionalContext",
        "cwd",
        "approvalPolicy",
        "approvalsReviewer",
        "sandboxPolicy",
        "model",
        "serviceTier",
        "effort",
        "summary",
        "personality",
        "outputSchema",
        "collaborationMode",
        "runtimeWorkspaceRoots",
        "permissions",
        "environments",
        "multiAgentMode",
    ];
    value
        .as_object()
        .is_some_and(|p| p.keys().all(|key| KNOWN.contains(&key.as_str())))
}
fn known_steer_params(value: &Value) -> bool {
    const KNOWN: &[&str] = &[
        "threadId",
        "clientUserMessageId",
        "input",
        "responsesapiClientMetadata",
        "additionalContext",
        "expectedTurnId",
    ];
    value
        .as_object()
        .is_some_and(|params| params.keys().all(|key| KNOWN.contains(&key.as_str())))
}
fn known_start_params(value: &Value) -> bool {
    const KNOWN: &[&str] = &[
        "model",
        "modelProvider",
        "allowProviderModelFallback",
        "serviceTier",
        "cwd",
        "runtimeWorkspaceRoots",
        "approvalPolicy",
        "approvalsReviewer",
        "sandbox",
        "permissions",
        "config",
        "serviceName",
        "baseInstructions",
        "developerInstructions",
        "personality",
        "multiAgentMode",
        "ephemeral",
        "historyMode",
        "sessionStartSource",
        "threadSource",
        "environments",
        "dynamicTools",
        "selectedCapabilityRoots",
        "mockExperimentalField",
        "experimentalRawEvents",
    ];
    value
        .as_object()
        .is_some_and(|p| p.keys().all(|key| KNOWN.contains(&key.as_str())))
}
fn warm_resume_matches(params: &Value, initial: Option<&Value>) -> bool {
    let (Some(params), Some(initial)) = (params.as_object(), initial.and_then(Value::as_object))
    else {
        return false;
    };
    params.iter().all(|(key, value)| {
        matches!(
            key.as_str(),
            "threadId" | "excludeTurns" | "initialTurnsPage"
        ) || (key != "history"
            && key != "path"
            && (value.is_null() || initial.get(key) == Some(value)))
    })
}
fn read_only_client_method(method: &str) -> bool {
    matches!(
        method,
        "thread/read"
            | "thread/list"
            | "thread/loaded/list"
            | "thread/turns/list"
            | "thread/items/list"
            | "thread/goal/get"
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
    )
}

fn allowed_client_method(method: &str) -> bool {
    read_only_client_method(method)
        || matches!(
            method,
            "thread/start"
                | "thread/resume"
                | "thread/unsubscribe"
                | "thread/name/set"
                | "turn/start"
                | "turn/steer"
                | "turn/interrupt"
                | "thread/settings/update"
                | "thread/goal/set"
                | "thread/goal/clear"
                | "account/login/start"
                | "account/login/cancel"
                | "account/logout"
                | "mcpServer/oauth/login"
        )
}
fn known_notification(method: &str) -> bool {
    matches!(
        method,
        "error"
            | "thread/started"
            | "thread/status/changed"
            | "thread/archived"
            | "thread/deleted"
            | "thread/unarchived"
            | "thread/closed"
            | "skills/changed"
            | "thread/name/updated"
            | "thread/goal/updated"
            | "thread/goal/cleared"
            | "thread/environment/connected"
            | "thread/environment/disconnected"
            | "thread/settings/updated"
            | "thread/tokenUsage/updated"
            | "turn/started"
            | "hook/started"
            | "turn/completed"
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
            | "account/updated"
            | "account/rateLimits/updated"
            | "app/list/updated"
            | "remoteControl/status/changed"
            | "externalAgentConfig/import/progress"
            | "externalAgentConfig/import/completed"
            | "fs/changed"
            | "item/reasoning/summaryTextDelta"
            | "item/reasoning/summaryPartAdded"
            | "item/reasoning/textDelta"
            | "thread/compacted"
            | "model/rerouted"
            | "model/verification"
            | "turn/moderationMetadata"
            | "model/safetyBuffering/updated"
            | "warning"
            | "guardianWarning"
            | "deprecationNotice"
            | "configWarning"
            | "fuzzyFileSearch/sessionUpdated"
            | "fuzzyFileSearch/sessionCompleted"
            | "thread/realtime/started"
            | "thread/realtime/itemAdded"
            | "thread/realtime/transcript/delta"
            | "thread/realtime/transcript/done"
            | "thread/realtime/outputAudio/delta"
            | "thread/realtime/sdp"
            | "thread/realtime/error"
            | "thread/realtime/closed"
            | "windows/worldWritableWarning"
            | "windowsSandbox/setupCompleted"
            | "account/login/completed"
    )
}
fn turn_observation(turn: &Value) -> Option<TurnObservation> {
    const KNOWN: &[&str] = &[
        "id",
        "items",
        "itemsView",
        "status",
        "error",
        "startedAt",
        "completedAt",
        "durationMs",
    ];
    if turn
        .as_object()?
        .keys()
        .any(|key| !KNOWN.contains(&key.as_str()))
    {
        return None;
    }
    let id = turn.get("id")?.as_str()?.to_owned();
    if id.is_empty() {
        return None;
    }
    let status = match turn.get("status")?.as_str()? {
        "inProgress" => TurnStatus::InProgress,
        "completed" => TurnStatus::Completed,
        "interrupted" => TurnStatus::Interrupted,
        "failed" => TurnStatus::Failed,
        _ => return None,
    };
    Some(TurnObservation {
        id,
        status,
        error_code: turn
            .pointer("/error/codexErrorInfo")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}
fn checked_failure(result: &Value, task: &TaskRecord, failed: &str) -> bool {
    let Some(thread) = result.get("thread") else {
        return false;
    };
    thread.get("id").and_then(Value::as_str) == Some(task.id.as_str())
        && thread.pointer("/status/type").and_then(Value::as_str) == Some("idle")
        && thread
            .get("turns")
            .and_then(Value::as_array)
            .and_then(|ts| ts.last())
            .and_then(turn_observation)
            .is_some_and(|t| {
                t.id == failed
                    && t.status == TurnStatus::Failed
                    && t.error_code.as_deref() == Some("usageLimitExceeded")
            })
}

async fn run_actor(
    mut core: Core,
    mut input: impl AsyncWrite + Unpin,
    mut events: mpsc::Receiver<Event>,
    mut controls: mpsc::Receiver<Control>,
    slot: Arc<Semaphore>,
    paused: Arc<AtomicBool>,
    mut client: Option<mpsc::Sender<Value>>,
) {
    let mut timer = tokio::time::interval(Duration::from_secs(1));
    loop {
        let mut stop = false;
        let actions: Result<Vec<Action>, String> = tokio::select! {
            command = controls.recv() => {
                match command {
                    Some(Control::LaunchConfirmed) => core.confirm_launch(),
                    Some(Control::View(reply)) => { let _ = reply.send(Ok(core.view())); }
                    Some(Control::Enable(enabled, reply)) => { let _ = reply.send(core.enable(enabled)); }
                    Some(Control::Pause(is_paused, reply)) => {
                        core.revision += 1;
                        if is_paused { core.quota = None; }
                        let _ = reply.send(Ok(()));
                    }
                    Some(Control::Remove(reply)) => {
                        if core.active || core.pending.values().any(|p| matches!(&p.purpose, Purpose::StartAccount {..}) || matches!(&p.purpose,
                            Purpose::Client {method,..} if matches!(method.as_str(), "thread/start" | "turn/start" | "turn/steer"))) {
                            let _ = reply.send(Err("任务仍在执行或提交中，请先在官方终端停止".into()));
                        }
                        else { core.pause("已移出托管"); let _ = reply.send(Ok(())); stop = true; }
                    }
                    None => stop = true,
                }
                Ok(vec![])
            }
            event = events.recv() => match event {
                Some(Event::Connected {epoch, output}) => {
                    core.client_epoch = Some(epoch); client = Some(output); Ok(vec![])
                }
                Some(Event::Disconnected(epoch)) => {
                    if core.client_epoch.as_deref() == Some(&epoch) {
                        core.client_epoch = None; client = None;
                        core.pause("官方终端已断开，自动续跑已关闭");
                    }
                    Ok(vec![])
                }
                Some(Event::Client(epoch, value)) => Ok(core.client(&epoch, value)),
                Some(Event::Backend(Ok(value))) => core.backend(value, paused.load(Ordering::SeqCst), &slot),
                Some(Event::Backend(Err(_))) | None => Err("托管 Codex 连接中断，未自动重启或重放".into()),
            },
            _ = timer.tick() => core.tick(paused.load(Ordering::SeqCst)),
        };
        if stop {
            break;
        }
        let actions = match actions {
            Ok(actions) => actions,
            Err(error) => {
                core.revoke(&error);
                let _ = core.save();
                break;
            }
        };
        // All state and durable intent changes precede the first external byte.
        if core.save().is_err() {
            core.revoke("无法保存本地状态，已停止自动续跑");
            break;
        }
        let mut failed = false;
        for action in actions {
            match action {
                Action::Client(value) => {
                    if let Some(client) = &client {
                        if client.try_send(value).is_err() {
                            core.pause("终端接收队列已断开，自动续跑已关闭");
                        }
                    }
                }
                Action::Backend(value) => {
                    let line = format!("{value}\n");
                    if !matches!(
                        tokio::time::timeout(
                            Duration::from_secs(5),
                            input.write_all(line.as_bytes())
                        )
                        .await,
                        Ok(Ok(()))
                    ) {
                        core.revoke("提交连接中断，结果不明；未自动重试");
                        failed = true;
                        break;
                    }
                }
            }
        }
        if failed {
            let _ = core.save();
            break;
        }
    }
}

async fn wait_for_client(
    core: &mut Core,
    events: &mut mpsc::Receiver<Event>,
    controls: &mut mpsc::Receiver<Control>,
) -> Option<mpsc::Sender<Value>> {
    loop {
        tokio::select! {
            event = events.recv() => {
                if let Event::Connected {epoch, output} = event? {
                    core.client_epoch = Some(epoch);
                    return Some(output);
                }
            }
            command = controls.recv() => match command {
                Some(Control::LaunchConfirmed) => core.confirm_launch(),
                Some(Control::View(reply)) => { let _ = reply.send(Ok(core.view())); }
                Some(Control::Enable(_, reply)) => { let _ = reply.send(Err("请先在官方终端开始任务".into())); }
                Some(Control::Pause(_, reply)) => { let _ = reply.send(Ok(())); }
                Some(Control::Remove(reply)) => { let _ = reply.send(Ok(())); return None; }
                None => return None,
            },
        }
    }
}

async fn dead_actor(core: Core, mut controls: mpsc::Receiver<Control>) {
    while let Some(command) = controls.recv().await {
        match command {
            Control::LaunchConfirmed => {}
            Control::View(reply) => {
                let _ = reply.send(Ok(core.view()));
            }
            Control::Enable(_, reply) | Control::Pause(_, reply) => {
                let _ = reply.send(Err("托管进程已结束".into()));
            }
            Control::Remove(reply) => {
                let _ = reply.send(Ok(()));
                break;
            }
        }
    }
}

async fn read_backend(output: impl AsyncRead + Unpin, send: mpsc::Sender<Event>) {
    let mut reader = BufReader::new(output);
    loop {
        let mut line = Vec::new();
        let read = (&mut reader)
            .take((MAX_FRAME + 1) as u64)
            .read_until(b'\n', &mut line)
            .await;
        let parsed = match read {
            Ok(n) if n > 0 && n <= MAX_FRAME && line.last() == Some(&b'\n') => {
                serde_json::from_slice(&line).map_err(|_| "Codex 返回了无法识别的数据".into())
            }
            _ => Err("Codex 连接已断开或返回数据过大".into()),
        };
        let failed = parsed.is_err();
        if send.send(Event::Backend(parsed)).await.is_err() || failed {
            break;
        }
    }
}

async fn gateway(listener: TcpListener, token: String, send: mpsc::Sender<Event>) {
    let occupied = Arc::new(AtomicBool::new(false));
    while let Ok((stream, peer)) = listener.accept().await {
        if !peer.ip().is_loopback() {
            continue;
        }
        let token = token.clone();
        let send = send.clone();
        let occupied = occupied.clone();
        tokio::spawn(async move {
            let claimed = Arc::new(AtomicBool::new(false));
            let callback_claimed = claimed.clone();
            let callback_occupied = occupied.clone();
            // Tungstenite's handshake API requires its unboxed HTTP error response.
            #[allow(clippy::result_large_err)]
            let callback = move |request: &Request, response: Response| {
                let valid = authorized_gateway(request, &token);
                let status = if !valid {
                    Some(StatusCode::UNAUTHORIZED)
                } else if callback_occupied
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_err()
                {
                    Some(StatusCode::CONFLICT)
                } else {
                    callback_claimed.store(true, Ordering::SeqCst);
                    None
                };
                if let Some(status) = status {
                    let mut error = tokio_tungstenite::tungstenite::http::Response::new(Some(
                        "Local managed session unavailable".into(),
                    ));
                    *error.status_mut() = status;
                    return Err(error);
                }
                Ok(response)
            };
            let mut config = WebSocketConfig::default();
            config.max_message_size = Some(MAX_FRAME);
            config.max_frame_size = Some(MAX_FRAME);
            let handshake = tokio::time::timeout(
                Duration::from_secs(5),
                accept_hdr_async_with_config(stream, callback, Some(config)),
            )
            .await;
            let Ok(Ok(socket)) = handshake else {
                if claimed.load(Ordering::SeqCst) {
                    occupied.store(false, Ordering::SeqCst);
                }
                return;
            };
            let epoch = Uuid::new_v4().to_string();
            let (output, mut outgoing) = mpsc::channel::<Value>(128);
            if send
                .send(Event::Connected {
                    epoch: epoch.clone(),
                    output,
                })
                .await
                .is_err()
            {
                occupied.store(false, Ordering::SeqCst);
                return;
            }
            let (mut writer, mut reader) = socket.split();
            loop {
                tokio::select! {
                    message = reader.next() => {
                        match message {
                            Some(Ok(Message::Text(text))) => {
                                let Ok(value) = serde_json::from_str::<Value>(&text) else { break; };
                                if send.send(Event::Client(epoch.clone(), value)).await.is_err() { break; }
                            }
                            Some(Ok(Message::Ping(bytes))) => {
                                if writer.send(Message::Pong(bytes)).await.is_err() { break; }
                            }
                            Some(Ok(Message::Pong(_))) => {}
                            _ => break,
                        }
                    }
                    value = outgoing.recv() => {
                        let Some(value) = value else { break; };
                        if !matches!(tokio::time::timeout(Duration::from_secs(5),
                            writer.send(Message::Text(value.to_string().into()))).await, Ok(Ok(()))) { break; }
                    }
                }
            }
            let _ = send.send(Event::Disconnected(epoch)).await;
            occupied.store(false, Ordering::SeqCst);
        });
    }
}

fn authorized_gateway(request: &Request, token: &str) -> bool {
    request.headers().get("origin").is_none()
        && request
            .headers()
            .get("authorization")
            .and_then(|h| h.to_str().ok())
            == Some(format!("Bearer {token}").as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_tungstenite::{connect_async, tungstenite::client::IntoClientRequest};

    fn configuration(thread: &str, cwd: &Path) -> Value {
        json!({
            "thread":{"id":thread,"status":{"type":"idle"},"turns":[]},
            "model":"test-model","modelProvider":"openai","serviceTier":null,
            "cwd":cwd.to_string_lossy(),"runtimeWorkspaceRoots":[cwd.to_string_lossy()],
            "instructionSources":[],"approvalPolicy":"on-request","approvalsReviewer":"user",
            "sandbox":{"type":"workspaceWrite","writableRoots":[cwd.to_string_lossy()],
                "networkAccess":false,"excludeTmpdirEnvVar":false,"excludeSlashTmp":false},
            "activePermissionProfile":{"id":":workspace","extends":null},
            "reasoningEffort":"medium","multiAgentMode":"explicitRequestOnly"
        })
    }

    fn launch_fixture(consent: bool) -> (tempfile::TempDir, Core) {
        let dir = tempfile::tempdir().unwrap();
        let spec = LaunchSpec {
            id: Uuid::new_v4().to_string(),
            executable: PathBuf::from("/fixture/codex"),
            cwd: dir.path().to_owned(),
            endpoint: "ws://127.0.0.1:1".into(),
            token_env: TOKEN_ENV.into(),
            token: "fixture-capability".into(),
            codex_home: dir.path().join("private-codex"),
            thread_id: None,
        };
        let core = Core::new(
            spec,
            consent,
            Store::open(dir.path().join("state.db")).unwrap(),
        );
        (dir, core)
    }

    fn fixture(consent: bool) -> (tempfile::TempDir, Core) {
        let (dir, mut core) = launch_fixture(consent);
        core.client_epoch = Some("native".into());
        core.initialized = true;
        core.initialize_result = Some(json!({"userAgent":"fixture"}));
        core.initial_params =
            Some(json!({"cwd":dir.path().to_string_lossy(),"approvalPolicy":"on-request"}));
        core.created(&configuration("owned-thread", dir.path()))
            .unwrap();
        (dir, core)
    }

    #[test]
    fn failed_startup_cannot_be_reactivated_by_late_launch_confirmation() {
        let (_dir, mut core) = launch_fixture(true);
        assert!(!core.view().can_enable);
        core.confirm_launch();
        assert!(!core.view().can_enable);
        core.revoke("启动失败，未建立托管连接");
        core.confirm_launch();
        let view = core.view();
        assert_eq!(view.state, TaskState::NeedsAttention);
        assert_eq!(view.detail, "启动失败，未建立托管连接");
        assert!(!view.auto_resume);
        assert!(!view.connected);
        assert!(!view.can_enable);
        assert!(core.enable(true).is_err());
        assert!(core.task.is_none());
        assert!(core.pending.is_empty());
    }

    #[test]
    fn late_launch_confirmation_preserves_a_pre_thread_disconnect_reason() {
        let (_dir, mut core) = launch_fixture(true);
        core.pause("官方终端已断开，自动续跑已关闭");
        core.confirm_launch();
        assert_eq!(core.view().detail, "官方终端已断开，自动续跑已关闭");
        assert!(!core.view().auto_resume);
        assert!(!core.view().can_enable);
    }

    fn turn(id: &str, status: &str, code: Option<&str>) -> Value {
        json!({"id":id,"status":status,"items":[],"itemsView":"full",
            "error":code.map(|code| json!({"message":"fixture","codexErrorInfo":code,"additionalDetails":null})),
            "startedAt":1,"completedAt":2,"durationMs":1000})
    }
    fn terminal(core: &mut Core, status: &str, code: Option<&str>) {
        core.notification(&json!({"method":"turn/completed","params":{
            "threadId":"owned-thread","turn":turn("failed-1",status,code)
        }}))
        .unwrap();
        core.notification(&json!({"method":"thread/status/changed","params":{
            "threadId":"owned-thread","status":{"type":"idle"}
        }}))
        .unwrap();
        core.next_check = 0;
        core.next_telemetry = i64::MAX;
        core.save().unwrap();
    }
    fn quota_response(used: i64) -> Value {
        let bucket = json!({"limitId":"codex","limitName":null,
            "primary":{"usedPercent":used,"windowDurationMins":300,"resetsAt":now()+600},
            "secondary":null,"credits":null,"individualLimit":null,"spendControlReached":false,
            "planType":"pro","rateLimitReachedType":null});
        json!({"rateLimits":bucket,"rateLimitsByLimitId":{"codex":bucket},"rateLimitResetCredits":null})
    }
    fn backend_request(actions: Vec<Action>, method: &str) -> Value {
        assert_eq!(actions.len(), 1);
        match actions.into_iter().next().unwrap() {
            Action::Backend(value) => {
                assert_eq!(value["method"], method);
                value
            }
            Action::Client(_) => panic!("expected backend request"),
        }
    }
    fn respond(
        core: &mut Core,
        request: &Value,
        result: Value,
        slot: &Arc<Semaphore>,
    ) -> Vec<Action> {
        core.backend(json!({"id":request["id"],"result":result}), false, slot)
            .unwrap()
    }
    fn check_request(core: &mut Core, slot: &Arc<Semaphore>, used: i64) -> Vec<Action> {
        let account = backend_request(core.tick(false).unwrap(), "account/read");
        let quota = backend_request(
            respond(core, &account, json!({"account":{"type":"chatgpt"}}), slot),
            "account/rateLimits/read",
        );
        respond(core, &quota, quota_response(used), slot)
    }
    fn matching_history() -> Value {
        json!({"thread":{"id":"owned-thread","status":{"type":"idle"},
            "turns":[turn("failed-1","failed",Some("usageLimitExceeded"))]}})
    }
    fn continuation(core: &mut Core, slot: &Arc<Semaphore>) -> Value {
        let check = backend_request(check_request(core, slot, 0), "thread/read");
        backend_request(
            respond(core, &check, matching_history(), slot),
            "turn/start",
        )
    }
    fn client_error(actions: Vec<Action>) -> bool {
        matches!(actions.as_slice(), [Action::Client(v)] if v.get("error").is_some())
    }

    fn expire_request(core: &mut Core, request: &Value) -> Vec<Action> {
        core.pending
            .get_mut(request["id"].as_str().unwrap())
            .unwrap()
            .deadline = Instant::now() - Duration::from_secs(1);
        core.next_check = i64::MAX;
        core.next_telemetry = i64::MAX;
        core.tick(false).unwrap()
    }

    #[test]
    fn late_client_reads_are_discarded_once_without_changing_consent_or_observations() {
        for method in [
            "model/list",
            "app/list",
            "mcpServerStatus/list",
            "account/read",
            "account/rateLimits/read",
        ] {
            let (_dir, mut core) = fixture(true);
            let slot = Arc::new(Semaphore::new(1));
            core.account_chatgpt = true;
            core.quota = Some(quota::parse_response(&quota_response(100), now()).unwrap());
            core.update_telemetry(&quota_response(100));
            let request = backend_request(
                core.client("native", json!({"id":7,"method":method,"params":{}})),
                method,
            );
            assert!(client_error(expire_request(&mut core, &request)));
            let task = core.task.clone();
            let quota = core.quota.clone();
            let telemetry = core.telemetry.clone();
            let result = if method == "account/read" {
                json!({"account":{"type":"apiKey"}})
            } else {
                quota_response(0)
            };
            assert!(respond(&mut core, &request, result.clone(), &slot).is_empty());
            assert!(core.valid);
            assert!(core.initial_consent);
            assert!(core.account_chatgpt);
            assert_eq!(core.task, task);
            assert_eq!(core.quota, quota);
            assert_eq!(core.telemetry, telemetry);
            assert!(core.retired_reads.is_empty());
            // A second response is a duplicate, not another exempt late read.
            assert!(respond(&mut core, &request, result, &slot).is_empty());
            assert!(!core.valid);
            assert!(!core.view().auto_resume);
        }
    }

    #[test]
    fn late_internal_reads_cannot_supply_quota_or_trigger_a_continuation() {
        let cases = [
            ("account/read", Purpose::Account),
            ("account/read", Purpose::TelemetryAccount),
            ("account/rateLimits/read", Purpose::Quota),
            ("account/rateLimits/read", Purpose::Telemetry),
            (
                "thread/read",
                Purpose::Check {
                    failed: "failed-1".into(),
                    revision: 0,
                },
            ),
        ];
        for (method, purpose) in cases {
            let (_dir, mut core) = fixture(true);
            let slot = Arc::new(Semaphore::new(1));
            terminal(&mut core, "failed", Some("usageLimitExceeded"));
            core.quota = Some(quota::parse_response(&quota_response(0), now()).unwrap());
            core.update_telemetry(&quota_response(100));
            let request = backend_request(vec![core.request(method, json!({}), purpose)], method);
            assert!(expire_request(&mut core, &request).is_empty());
            let quota = core.quota.clone();
            let telemetry = core.telemetry.clone();
            let revision = core.revision;
            let result = match method {
                "account/read" => json!({"account":{"type":"chatgpt"}}),
                "thread/read" => matching_history(),
                _ => quota_response(0),
            };
            assert!(respond(&mut core, &request, result, &slot).is_empty());
            assert!(core.valid);
            assert!(core.view().auto_resume);
            assert_eq!(core.revision, revision);
            assert_eq!(core.quota, quota);
            assert_eq!(core.telemetry, telemetry);
            assert!(!core.account_chatgpt);
            assert!(core.pending.is_empty());
            assert!(core
                .store
                .get_intent("owned-thread", "failed-1")
                .unwrap()
                .is_none());
        }
    }

    #[test]
    fn expired_start_account_read_never_starts_a_deferred_thread() {
        let (_dir, mut core) = launch_fixture(true);
        core.client_epoch = Some("native".into());
        core.initialized = true;
        let request = backend_request(
            core.client(
                "native",
                json!({"id":7,"method":"thread/start","params":{}}),
            ),
            "account/read",
        );
        assert!(client_error(expire_request(&mut core, &request)));
        assert!(respond(
            &mut core,
            &request,
            json!({"account":{"type":"chatgpt"}}),
            &Arc::new(Semaphore::new(1))
        )
        .is_empty());
        assert!(!core.created);
        assert!(!core.initial_consent);
        assert!(!core.account_chatgpt);
        assert!(core.task.is_none());
        assert!(core.pending.is_empty());
    }

    #[test]
    fn late_mutations_and_unknown_response_ids_remain_fail_closed() {
        for method in [
            "initialize",
            "account/login/start",
            "account/logout",
            "thread/settings/update",
            "thread/start",
            "thread/resume",
            "turn/start",
            "turn/steer",
            "mcpServer/oauth/login",
            "future/read",
        ] {
            let (_dir, mut core) = fixture(true);
            let request = backend_request(
                vec![core.request(
                    method,
                    json!({}),
                    Purpose::Client {
                        epoch: "native".into(),
                        original_id: json!(7),
                        method: method.into(),
                    },
                )],
                method,
            );
            assert!(client_error(expire_request(&mut core, &request)));
            assert!(core.retired_reads.is_empty());
            assert!(
                respond(&mut core, &request, json!({}), &Arc::new(Semaphore::new(1))).is_empty()
            );
            assert!(!core.valid);
            assert!(!core.view().auto_resume);
            assert!(core.enable(true).is_err());
        }
        let (_dir, mut core) = fixture(true);
        let unknown_id = format!("ar:{}:999999", core.spec.id);
        assert!(core
            .backend(
                json!({"id":unknown_id,"result":{}}),
                false,
                &Arc::new(Semaphore::new(1))
            )
            .unwrap()
            .is_empty());
        assert!(!core.valid);
    }

    #[test]
    fn retired_reads_are_bounded_and_evicted_ids_remain_unknown() {
        let (_dir, mut core) = fixture(true);
        let mut requests = vec![];
        for _ in 0..=MAX_RETIRED_READS {
            let request = backend_request(
                vec![core.request("account/rateLimits/read", json!({}), Purpose::Telemetry)],
                "account/rateLimits/read",
            );
            assert!(expire_request(&mut core, &request).is_empty());
            assert!(core.retired_reads.len() <= MAX_RETIRED_READS);
            requests.push(request);
        }
        assert_eq!(core.retired_reads.len(), MAX_RETIRED_READS);
        let slot = Arc::new(Semaphore::new(1));
        assert!(respond(&mut core, requests.last().unwrap(), json!({}), &slot).is_empty());
        assert!(core.valid);
        assert_eq!(core.retired_reads.len(), MAX_RETIRED_READS - 1);
        assert!(respond(&mut core, &requests[0], json!({}), &slot).is_empty());
        assert!(!core.valid);
        assert!(!core.view().auto_resume);
    }

    #[test]
    fn full_quota_cycle_persists_before_send_and_finishes_without_cajoling() {
        let (_dir, mut core) = fixture(true);
        let slot = Arc::new(Semaphore::new(1));
        terminal(&mut core, "failed", Some("usageLimitExceeded"));
        assert!(check_request(&mut core, &slot, 100).is_empty());
        assert_eq!(core.task.as_ref().unwrap().state, TaskState::WaitingQuota);
        assert!(core.task.as_ref().unwrap().resets_at.unwrap() > now());
        core.next_check = 0;
        let send = continuation(&mut core, &slot);
        let intent = core
            .store
            .get_intent("owned-thread", "failed-1")
            .unwrap()
            .unwrap();
        assert_eq!(intent.status, crate::store::IntentStatus::Claimed);
        assert_eq!(send["params"].as_object().unwrap().len(), 3);
        assert_eq!(send["params"]["threadId"], "owned-thread");
        for field in [
            "model",
            "sandboxPolicy",
            "approvalPolicy",
            "serviceTier",
            "cwd",
        ] {
            assert!(send["params"].get(field).is_none());
        }
        respond(
            &mut core,
            &send,
            json!({"turn":turn("recovered","inProgress",None)}),
            &slot,
        );
        assert_eq!(
            core.store
                .get_intent("owned-thread", "failed-1")
                .unwrap()
                .unwrap()
                .status,
            crate::store::IntentStatus::Reconciled
        );
        core.notification(&json!({"method":"turn/completed","params":{
            "threadId":"owned-thread","turn":turn("recovered","completed",None)
        }}))
        .unwrap();
        assert_eq!(core.task.as_ref().unwrap().state, TaskState::Completed);
        core.next_check = 0;
        assert!(core.tick(false).unwrap().is_empty());
        assert_eq!(slot.available_permits(), 1);
    }

    #[test]
    fn only_terminal_structured_quota_failure_enters_execution_pipeline() {
        for (status, error) in [
            ("completed", None),
            ("interrupted", None),
            ("failed", None),
            ("failed", Some("http429")),
            ("inProgress", Some("usageLimitExceeded")),
        ] {
            let (_dir, mut core) = fixture(true);
            terminal(&mut core, status, error);
            assert!(!core.eligible(false), "{status:?} {error:?}");
            assert!(core.tick(false).unwrap().is_empty());
        }
    }

    #[test]
    fn existing_monitoring_consent_is_not_execution_consent() {
        let (dir, mut core) = fixture(false);
        terminal(&mut core, "failed", Some("usageLimitExceeded"));
        assert!(!core.eligible(false));
        core.enable(true).unwrap();
        assert!(core.task.as_ref().unwrap().failed_turn_id.is_none());
        assert!(core.tick(false).unwrap().is_empty());
        let store = Store::open(dir.path().join("state.db")).unwrap();
        assert!(store.load_tasks().unwrap().is_empty());
        assert_eq!(store.load_managed_tasks().unwrap().len(), 1);
    }

    #[test]
    fn latest_user_turn_invalidates_inflight_check_before_any_send() {
        let (_dir, mut core) = fixture(true);
        let slot = Arc::new(Semaphore::new(1));
        terminal(&mut core, "failed", Some("usageLimitExceeded"));
        let check = backend_request(check_request(&mut core, &slot, 0), "thread/read");
        let user = core.client("native",json!({"id":7,"method":"turn/start","params":{
            "threadId":"owned-thread","input":[{"type":"text","text":"new task","text_elements":[]}]
        }}));
        assert_eq!(
            backend_request(user, "turn/start")["params"]["input"][0]["text"],
            "new task"
        );
        assert!(respond(&mut core, &check, matching_history(), &slot).is_empty());
        assert!(core
            .store
            .get_intent("owned-thread", "failed-1")
            .unwrap()
            .is_none());
    }

    #[test]
    fn active_or_changed_history_never_receives_continue() {
        for history in [
            json!({"thread":{"id":"owned-thread","status":{"type":"active","activeFlags":[]},"turns":[]}}),
            json!({"thread":{"id":"owned-thread","status":{"type":"idle"},"turns":[turn("new","completed",None)]}}),
            json!({"thread":{"id":"other","status":{"type":"idle"},"turns":[turn("failed-1","failed",Some("usageLimitExceeded"))]}}),
        ] {
            let (_dir, mut core) = fixture(true);
            let slot = Arc::new(Semaphore::new(1));
            terminal(&mut core, "failed", Some("usageLimitExceeded"));
            let check = backend_request(check_request(&mut core, &slot, 0), "thread/read");
            assert!(respond(&mut core, &check, history, &slot).is_empty());
            assert!(!core.valid);
            assert!(core
                .store
                .get_intent("owned-thread", "failed-1")
                .unwrap()
                .is_none());
        }
    }

    #[test]
    fn lost_ack_keeps_durable_tombstone_and_cannot_retry() {
        let (dir, mut core) = fixture(true);
        let slot = Arc::new(Semaphore::new(1));
        terminal(&mut core, "failed", Some("usageLimitExceeded"));
        let send = continuation(&mut core, &slot);
        core.pending
            .get_mut(send["id"].as_str().unwrap())
            .unwrap()
            .deadline = Instant::now() - Duration::from_secs(1);
        core.tick(false).unwrap();
        core.save().unwrap();
        let intent = core
            .store
            .get_intent("owned-thread", "failed-1")
            .unwrap()
            .unwrap();
        assert_eq!(intent.status, crate::store::IntentStatus::Ambiguous);
        assert!(!core.valid);
        assert!(core.enable(true).is_err());
        assert!(!core
            .retired_reads
            .iter()
            .any(|id| Some(id.as_str()) == send["id"].as_str()));
        assert!(respond(
            &mut core,
            &send,
            json!({"turn":turn("late-accepted", "inProgress", None)}),
            &slot
        )
        .is_empty());
        assert!(!core.valid);
        assert!(core.tick(false).unwrap().is_empty());
        assert_eq!(
            core.store
                .get_intent("owned-thread", "failed-1")
                .unwrap()
                .unwrap(),
            intent
        );
        let hub = ManagedHub::open(dir.path(), &dir.path().join("state.db")).unwrap();
        drop(hub);
        let store = Store::open(dir.path().join("state.db")).unwrap();
        let (task, meta) = &store.load_managed_tasks().unwrap()[0];
        assert!(!meta.auto_resume);
        assert_eq!(task.state, TaskState::NeedsAttention);
        assert!(store
            .get_intent("owned-thread", "failed-1")
            .unwrap()
            .is_some());
    }

    #[test]
    fn terminal_event_before_ack_is_not_overwritten_by_in_progress_response() {
        let (_dir, mut core) = fixture(true);
        let slot = Arc::new(Semaphore::new(1));
        terminal(&mut core, "failed", Some("usageLimitExceeded"));
        let send = continuation(&mut core, &slot);
        core.notification(&json!({"method":"turn/completed","params":{
            "threadId":"owned-thread","turn":turn("recovered","completed",None)
        }}))
        .unwrap();
        respond(
            &mut core,
            &send,
            json!({"turn":turn("recovered","inProgress",None)}),
            &slot,
        );
        assert_eq!(core.task.as_ref().unwrap().state, TaskState::Completed);
        assert!(!core.active);
    }

    #[test]
    fn automatic_turns_across_managers_are_serialized() {
        let (_a, mut first) = fixture(true);
        let (_b, mut second) = fixture(true);
        let slot = Arc::new(Semaphore::new(1));
        terminal(&mut first, "failed", Some("usageLimitExceeded"));
        terminal(&mut second, "failed", Some("usageLimitExceeded"));
        let _send = continuation(&mut first, &slot);
        assert_eq!(slot.available_permits(), 0);
        let check = backend_request(check_request(&mut second, &slot, 0), "thread/read");
        assert!(respond(&mut second, &check, matching_history(), &slot).is_empty());
        assert!(second
            .store
            .get_intent("owned-thread", "failed-1")
            .unwrap()
            .is_none());
        first.notification(&json!({"id":9,"method":"item/tool/requestUserInput","params":{"threadId":"owned-thread"}})).unwrap();
        assert_eq!(slot.available_permits(), 1);
        second.next_check = 0;
        let _send = continuation(&mut second, &slot);
        assert_eq!(slot.available_permits(), 0);
    }

    #[test]
    fn disabling_or_disconnecting_does_not_release_a_running_automatic_turn() {
        let (_dir, mut core) = fixture(true);
        let slot = Arc::new(Semaphore::new(1));
        terminal(&mut core, "failed", Some("usageLimitExceeded"));
        let _send = continuation(&mut core, &slot);
        core.pause("终端已断开");
        assert_eq!(slot.available_permits(), 0);
        core.revoke("执行上下文已失效");
        assert_eq!(slot.available_permits(), 0);
        core.notification(&json!({"method":"turn/completed","params":{
            "threadId":"owned-thread","turn":turn("recovered","completed",None)
        }}))
        .unwrap();
        assert_eq!(slot.available_permits(), 1);
    }

    #[test]
    fn unknown_active_flags_keep_the_running_queue_slot() {
        let (_dir, mut core) = fixture(true);
        let slot = Arc::new(Semaphore::new(1));
        terminal(&mut core, "failed", Some("usageLimitExceeded"));
        let _send = continuation(&mut core, &slot);
        core.notification(&json!({"method":"thread/status/changed","params":{
            "threadId":"owned-thread","status":{"type":"active","activeFlags":["future-state"]}
        }}))
        .unwrap();
        assert!(!core.valid);
        assert_eq!(slot.available_permits(), 0);
    }

    #[test]
    fn unknown_steer_options_cannot_reach_the_owned_server() {
        let (_dir, mut core) = fixture(true);
        let actions = core.client("native", json!({"id":1,"method":"turn/steer","params":{
            "threadId":"owned-thread","expectedTurnId":"running","input":[],"futurePermissions":true
        }}));
        assert!(client_error(actions));
        assert!(!core.valid);
    }

    #[test]
    fn official_tui_serialized_turn_is_forwarded_with_null_optional_fields() {
        let (dir, mut core) = fixture(true);
        // codex-rs/tui/src/app_server_session.rs::turn_start constructs every
        // Option field below. Serde emits null except the omitted serviceTier.
        let params = json!({
            "threadId":"owned-thread","clientUserMessageId":null,
            "input":[{"type":"text","text":"fixture","text_elements":[]}],
            "responsesapiClientMetadata":null,"additionalContext":null,"environments":null,
            "cwd":dir.path().to_string_lossy(),
            "runtimeWorkspaceRoots":[dir.path().to_string_lossy()],
            "approvalPolicy":"on-request","approvalsReviewer":"user",
            "sandboxPolicy":{"type":"workspaceWrite","writableRoots":[],
                "networkAccess":false,"excludeTmpdirEnvVar":false,"excludeSlashTmp":false},
            "permissions":null,"model":"test-model","effort":"medium","summary":null,
            "personality":null,"outputSchema":null,"collaborationMode":null,"multiAgentMode":null
        });
        let send = backend_request(
            core.client(
                "native",
                json!({
                    "id":1,"method":"turn/start","params":params
                }),
            ),
            "turn/start",
        );
        assert_eq!(send["params"], params);
        assert!(core.valid);
        assert!(core.meta.as_ref().unwrap().auto_resume);
    }

    #[test]
    fn per_turn_context_is_forwarded_but_cannot_be_silently_lost_on_continuation() {
        for field in [
            "outputSchema",
            "responsesapiClientMetadata",
            "additionalContext",
        ] {
            let (_dir, mut core) = fixture(true);
            let mut params = json!({"threadId":"owned-thread","input":[]});
            params[field] = json!({"fixture":"non-null"});
            let send = backend_request(
                core.client(
                    "native",
                    json!({
                        "id":1,"method":"turn/start","params":params
                    }),
                ),
                "turn/start",
            );
            assert_eq!(send["params"], params);
            assert!(!core.valid);
            assert!(core.enable(true).is_err());
            assert!(!core.meta.as_ref().unwrap().auto_resume);
        }
    }

    #[test]
    fn approval_requires_native_user_response_and_revokes_auto_consent() {
        let (_dir, mut core) = fixture(true);
        let slot = Arc::new(Semaphore::new(1));
        terminal(&mut core, "failed", Some("usageLimitExceeded"));
        let request = json!({"id":77,"method":"item/commandExecution/requestApproval","params":{"threadId":"owned-thread"}});
        assert!(
            matches!(core.backend(request.clone(),false,&slot).unwrap().as_slice(), [Action::Client(v)] if v == &request)
        );
        assert!(!core.meta.as_ref().unwrap().auto_resume);
        assert!(core.enable(true).is_err());
        assert!(core.tick(false).unwrap().is_empty());
        let response = json!({"id":77,"result":{"decision":"decline"}});
        assert!(
            matches!(core.client("native",response.clone()).as_slice(),[Action::Backend(v)] if v == &response)
        );
        assert!(core.server_requests.is_empty());
        assert!(!core.meta.as_ref().unwrap().auto_resume);
    }

    #[test]
    fn credential_and_dynamic_tool_requests_are_not_exposed_to_native_client() {
        for method in [
            "account/chatgptAuthTokens/refresh",
            "attestation/generate",
            "item/tool/call",
            "future/serverRequest",
        ] {
            let (_dir, mut core) = fixture(true);
            let actions = core
                .backend(
                    json!({"id":9,"method":method,"params":{}}),
                    false,
                    &Arc::new(Semaphore::new(1)),
                )
                .unwrap();
            assert!(matches!(actions.as_slice(),[Action::Backend(v)] if v.get("error").is_some()));
            assert!(!core.valid);
        }
    }

    #[test]
    fn arbitrary_sessions_fork_and_unknown_methods_cannot_reach_backend() {
        for request in [
            json!({"id":1,"method":"thread/resume","params":{"threadId":"external"}}),
            json!({"id":1,"method":"thread/fork","params":{"threadId":"owned-thread"}}),
            json!({"id":1,"method":"turn/start","params":{"threadId":"external","input":[]}}),
            json!({"id":1,"method":"thread/start","params":{}}),
            json!({"id":1,"method":"future/write","params":{}}),
            json!({"id":1,"method":"account/rateLimitResetCredit/consume","params":{}}),
            json!({"id":1,"method":"account/login/start","params":{"type":"apiKey","apiKey":"synthetic"}}),
        ] {
            let (_dir, mut core) = fixture(true);
            assert!(client_error(core.client("native", request)));
        }
    }

    #[test]
    fn first_start_requires_official_chatgpt_account_before_creating_thread() {
        let (_dir, mut core) = fixture(true);
        core.created = false;
        core.task = None;
        core.meta = None;
        let request = backend_request(
            core.client(
                "native",
                json!({"id":1,"method":"thread/start","params":{}}),
            ),
            "account/read",
        );
        let actions = respond(
            &mut core,
            &request,
            json!({"account":{"type":"apiKey"}}),
            &Arc::new(Semaphore::new(1)),
        );
        assert!(client_error(actions));
        assert!(!core.created);
    }

    #[test]
    fn all_login_updates_even_chatgpt_to_chatgpt_revoke_execution() {
        let (_dir, mut core) = fixture(true);
        terminal(&mut core, "failed", Some("usageLimitExceeded"));
        core.notification(&json!({"method":"account/updated","params":{"authMode":"chatgpt"}}))
            .unwrap();
        assert!(!core.meta.as_ref().unwrap().auto_resume);
        assert!(!core.eligible(false));
    }

    #[test]
    fn unknown_configuration_and_turn_fields_fail_closed() {
        let (dir, mut core) = fixture(true);
        let mut response = configuration("new", dir.path());
        response["futurePermission"] = json!(true);
        assert!(!valid_configuration(&response));
        let mut response = configuration("new", dir.path());
        response["sandbox"]["futureRule"] = json!(true);
        assert!(!valid_configuration(&response));
        let mut unknown = turn("failed-1", "failed", Some("usageLimitExceeded"));
        unknown["futureState"] = json!("unknown");
        core.observe(&unknown).unwrap();
        assert!(!core.valid);
        let (_dir, mut core) = fixture(true);
        core.notification(&json!({"method":"future/notification","params":{}}))
            .unwrap();
        assert!(!core.valid);
    }

    #[test]
    fn stopped_disconnected_and_global_pause_never_submit() {
        let (_dir, mut core) = fixture(true);
        terminal(&mut core, "failed", Some("usageLimitExceeded"));
        assert!(core.eligible(false));
        assert!(!core.eligible(true));
        core.client_epoch = None;
        core.pause("断开");
        assert!(!core.eligible(false));
        assert!(core.enable(true).is_err());
    }

    #[test]
    fn replayed_frontend_epoch_and_unmatched_responses_cannot_write() {
        let (_dir, mut core) = fixture(true);
        assert!(core
            .client(
                "old-client",
                json!({"id":1,"method":"turn/start","params":{"threadId":"owned-thread"}})
            )
            .is_empty());
        assert!(core
            .client("native", json!({"id":999,"result":{"decision":"accept"}}))
            .is_empty());
        assert!(!core.valid);
    }

    #[test]
    fn gateway_rejects_browser_origin_and_wrong_capability() {
        let request = |token: &str, origin: bool| {
            let mut request = "ws://127.0.0.1:1".into_client_request().unwrap();
            request
                .headers_mut()
                .insert("authorization", format!("Bearer {token}").parse().unwrap());
            if origin {
                request
                    .headers_mut()
                    .insert("origin", "https://untrusted.invalid".parse().unwrap());
            }
            request
        };
        assert!(authorized_gateway(&request("secret", false), "secret"));
        assert!(!authorized_gateway(&request("wrong", false), "secret"));
        assert!(!authorized_gateway(&request("secret", true), "secret"));
    }

    #[tokio::test]
    async fn real_gateway_allows_one_authenticated_client_and_reports_disconnect() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let endpoint = format!("ws://{}", listener.local_addr().unwrap());
        let (send, mut events) = mpsc::channel(16);
        let task = tokio::spawn(gateway(listener, "fixture-token".into(), send));
        let make = |token: &str| {
            let mut request = endpoint.clone().into_client_request().unwrap();
            request
                .headers_mut()
                .insert("authorization", format!("Bearer {token}").parse().unwrap());
            request
        };
        assert!(connect_async(make("incorrect")).await.is_err());
        let (mut socket, _) = connect_async(make("fixture-token")).await.unwrap();
        let (epoch, _output) = match events.recv().await.unwrap() {
            Event::Connected { epoch, output } => (epoch, output),
            _ => panic!("missing connection event"),
        };
        assert!(connect_async(make("fixture-token")).await.is_err());
        socket
            .send(Message::Text(
                json!({"id":1,"method":"initialize","params":{}})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        assert!(matches!(events.recv().await.unwrap(),Event::Client(e,_) if e == epoch));
        socket.close(None).await.unwrap();
        assert!(
            matches!(tokio::time::timeout(Duration::from_secs(2),events.recv()).await.unwrap().unwrap(),Event::Disconnected(e) if e == epoch)
        );
        task.abort();
    }

    #[tokio::test]
    async fn backend_reader_handles_fragmented_json_and_rejects_oversized_frames() {
        let (mut input, output) = tokio::io::duplex(128);
        let (send, mut events) = mpsc::channel(4);
        let reader = tokio::spawn(read_backend(output, send));
        input.write_all(b"{\"id\":").await.unwrap();
        input.write_all(b"1,\"result\":{}}\n").await.unwrap();
        assert!(matches!(events.recv().await.unwrap(),Event::Backend(Ok(v)) if v["id"] == 1));
        drop(input);
        assert!(matches!(
            events.recv().await.unwrap(),
            Event::Backend(Err(_))
        ));
        reader.await.unwrap();
        let (mut input, output) = tokio::io::duplex(64 * 1024);
        let (send, mut events) = mpsc::channel(4);
        let reader = tokio::spawn(read_backend(output, send));
        let writer = tokio::spawn(async move {
            let _ = input.write_all(&vec![b'x'; MAX_FRAME + 1]).await;
        });
        assert!(matches!(
            events.recv().await.unwrap(),
            Event::Backend(Err(_))
        ));
        reader.await.unwrap();
        writer.await.unwrap();
    }
}
