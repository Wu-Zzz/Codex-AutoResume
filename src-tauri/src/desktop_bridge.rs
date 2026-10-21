//! Private control plane around the original Desktop's stdio connection.
//! Native traffic is never used as a source of credentials or persisted prompts.
use crate::{
    desktop_guard::{DesktopGuard, GuardRequest},
    desktop_launch::BridgeConfig,
    runtime::now,
    scheduler::TaskView,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    ffi::OsString,
    fs,
    io::{BufRead, Write},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    process::Command,
    sync::{mpsc, oneshot, Semaphore},
    time::{interval, timeout, Instant, MissedTickBehavior},
};
use uuid::Uuid;

const CONTROL_TIMEOUT: Duration = Duration::from_secs(3);
const GUI_LEASE: Duration = Duration::from_secs(20);
const MAX_CONTROL_BYTES: u64 = 64 * 1024;
const ENABLE_FILE_ENV: &str = "AUTORESUME_DESKTOP_ENABLE_FILE";
const ENABLE_TOKEN_ENV: &str = "AUTORESUME_DESKTOP_ENABLE_TOKEN";
const MAX_ENABLE_TOKEN_BYTES: u64 = 128;

/// Installation consent belongs to one prepared runtime. Disabling or replacing
/// its token permanently retires this bridge's automatic control, while native
/// Desktop traffic continues over the same connection.
enum InstallationGate {
    Manual,
    Enabled { path: PathBuf, token: String },
    Revoked,
}

impl InstallationGate {
    fn from_environment() -> Self {
        Self::new(
            std::env::var_os(ENABLE_FILE_ENV),
            std::env::var_os(ENABLE_TOKEN_ENV),
        )
    }

    fn new(path: Option<OsString>, token: Option<OsString>) -> Self {
        match (path, token) {
            (None, None) => Self::Manual,
            (Some(path), Some(token)) => {
                let path = PathBuf::from(path);
                let Ok(token) = token.into_string() else {
                    return Self::Revoked;
                };
                if !path.is_absolute()
                    || token.is_empty()
                    || token.len() > MAX_ENABLE_TOKEN_BYTES as usize
                    || token.chars().any(char::is_control)
                {
                    return Self::Revoked;
                }
                let mut gate = Self::Enabled { path, token };
                gate.check();
                gate
            }
            _ => Self::Revoked,
        }
    }

    fn check(&mut self) -> bool {
        let allowed = match self {
            Self::Manual => true,
            Self::Enabled { path, token } => enable_token_matches(path, token),
            Self::Revoked => false,
        };
        if !allowed {
            *self = Self::Revoked;
        }
        allowed
    }

    fn enforce(&mut self, guard: &mut DesktopGuard, now: i64) -> bool {
        if self.check() {
            true
        } else {
            // Native events may still update observations after revocation.
            // Clear readiness and pending automatic work before exposing a
            // control snapshot or considering another automatic send.
            guard.disconnected(now);
            false
        }
    }
}

fn enable_token_matches(path: &Path, token: &str) -> bool {
    let verify = || -> Option<bool> {
        let before = fs::symlink_metadata(path).ok()?;
        if !private_enable_file(&before) {
            return Some(false);
        }
        let file = fs::File::open(path).ok()?;
        let opened = file.metadata().ok()?;
        if !same_enable_file(&before, &opened) {
            return Some(false);
        }
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(
            &mut std::io::Read::take(file, MAX_ENABLE_TOKEN_BYTES + 1),
            &mut bytes,
        )
        .ok()?;
        let after = fs::symlink_metadata(path).ok()?;
        Some(same_enable_file(&opened, &after) && bytes == token.as_bytes())
    };
    verify().unwrap_or(false)
}

fn private_enable_file(metadata: &fs::Metadata) -> bool {
    let regular = metadata.is_file()
        && !metadata.file_type().is_symlink()
        && metadata.len() <= MAX_ENABLE_TOKEN_BYTES;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        regular && metadata.permissions().mode() & 0o077 == 0
    }
    #[cfg(not(unix))]
    regular
}

fn same_enable_file(before: &fs::Metadata, after: &fs::Metadata) -> bool {
    let same = private_enable_file(before)
        && private_enable_file(after)
        && before.len() == after.len()
        && before.modified().ok() == after.modified().ok();
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        same && before.dev() == after.dev() && before.ino() == after.ino()
    }
    #[cfg(not(unix))]
    same
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Registration {
    version: u32,
    generation: String,
    port: u16,
    secret: String,
    thread_ids: Vec<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
enum ControlCommand {
    Snapshot,
    SetEnabled { thread_id: String, enabled: bool },
    SetPaused { paused: bool },
    RevokeAll,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlRequest {
    secret: String,
    generation: String,
    command: ControlCommand,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ControlResponse {
    generation: String,
    tasks: Vec<TaskView>,
    quota: Option<crate::quota::QuotaTelemetry>,
    error: Option<String>,
}

struct ControlEvent {
    request: ControlCommand,
    reply: oneshot::Sender<ControlResponse>,
}

/// Client request IDs are replaced, not merely prefixed internal IDs. A native
/// caller can otherwise accidentally choose the same ID as an internal probe.
#[derive(Default)]
struct RpcMux {
    serial: u64,
    pending: HashMap<String, (Value, Value)>,
    generation: String,
}

impl RpcMux {
    fn new(generation: &str) -> Self {
        Self {
            generation: generation.into(),
            ..Self::default()
        }
    }

    fn client(&mut self, message: &mut Value) {
        if message.get("method").is_none() {
            // Replies to server tool/approval requests keep the original ID.
            return;
        }
        let Some(id) = message.get("id").cloned() else {
            return;
        };
        self.serial += 1;
        let wire_id = format!("desktop:{}:{}", self.generation, self.serial);
        let request = json!({
            "method": message.get("method"),
            "params": {"threadId": message.pointer("/params/threadId")}
        });
        self.pending.insert(wire_id.clone(), (id, request));
        message["id"] = json!(wire_id);
    }

    fn internal(&self, request: GuardRequest) -> Value {
        json!({
            "id": format!("autoresume:{}:{}", self.generation, request.id),
            "method": request.method, "params": request.params
        })
    }

    fn internal_response(&self, message: &Value) -> Option<u64> {
        if message.get("method").is_some() {
            return None;
        }
        message
            .get("id")?
            .as_str()?
            .strip_prefix(&format!("autoresume:{}:", self.generation))?
            .parse()
            .ok()
    }

    fn native_response(&mut self, message: &mut Value) -> Option<Value> {
        if message.get("method").is_some() {
            return None;
        }
        let wire_id = message.get("id")?.as_str()?;
        let (id, request) = self.pending.remove(wire_id)?;
        message["id"] = id;
        Some(request)
    }
}

/// Called by the console wrapper; no Tauri window or GUI runtime is started.
pub fn run(config: BridgeConfig, args: Vec<OsString>) -> Result<i32, String> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?
        .block_on(run_async(config, args))
}

async fn run_async(config: BridgeConfig, args: Vec<OsString>) -> Result<i32, String> {
    let mut installation_gate = InstallationGate::from_environment();
    let generation = Uuid::new_v4().to_string();
    let directory = config.data_dir.join("desktop-bridge");
    private_directory(&directory)?;
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|e| e.to_string())?;
    let mut registration = Registration {
        version: 1,
        generation: generation.clone(),
        port: listener.local_addr().map_err(|e| e.to_string())?.port(),
        secret: format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple()),
        thread_ids: vec![],
    };
    let registry = directory.join(format!("{generation}.json"));
    private_file(
        &registry,
        &serde_json::to_vec(&registration).map_err(|e| e.to_string())?,
    )?;
    let _cleanup = RegistryFile(registry.clone());
    let mut guard = DesktopGuard::open(&config.data_dir.join("desktop-state.sqlite"))?;
    let mut ownership_gate = OwnershipGate::open(&config.data_dir)?;
    let mut child = Command::new(&config.guarded_app_server)
        .args(args)
        .env("CODEX_CLI_PATH", &config.official_cli)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("无法启动实验桌面执行组件：{e}"))?;
    let mut child_input = child.stdin.take().ok_or("执行组件没有输入通道")?;
    let mut child_output =
        BufReader::new(child.stdout.take().ok_or("执行组件没有输出通道")?).lines();

    // A detached OS reader avoids Tokio's uncancellable stdin task preventing
    // process exit when the backend closes first.
    let (native_tx, mut native_rx) = mpsc::channel::<Result<String, String>>(8);
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            if native_tx
                .blocking_send(line.map_err(|e| e.to_string()))
                .is_err()
            {
                break;
            }
        }
    });
    let (control_tx, mut control_rx) = mpsc::channel(16);
    let accept_task = tokio::spawn(accept_controls(listener, registration.clone(), control_tx));
    let mut mux = RpcMux::new(&generation);
    let mut timer = interval(Duration::from_secs(1));
    timer.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut last_control: Option<Instant> = None;
    let mut lease_expired = false;
    let mut protocol_usable = true;

    let result = async {
        loop {
            // Native input takes priority over timer-driven sends when both are
            // queued. Core's conditional RPC also checks independently spawned turns.
            tokio::select! {
                biased;
                native = native_rx.recv() => {
                    let Some(native) = native else { break; };
                    let native = native?;
                    match serde_json::from_str::<Value>(&native) {
                        Ok(mut message) if message.is_object() => {
                            // Serialize a second runtime's declaration and native
                            // load against the first runtime's check-and-send.
                            let ownership = if message["method"] == "thread/resume" {
                                Some(ownership_gate.lock()?)
                            } else {
                                None
                            };
                            if message["method"] == "thread/resume" {
                                if let Some(id) = message.pointer("/params/threadId").and_then(Value::as_str) {
                                    register_thread(&registry, &mut registration, id)?;
                                }
                            }
                            guard.native_client(&message, now());
                            mux.client(&mut message);
                            write_message(&mut child_input, &message).await?;
                            drop(ownership);
                        }
                        _ => {
                            protocol_usable = false;
                            guard.disconnected(now());
                            child_input.write_all(native.as_bytes()).await.map_err(|e| e.to_string())?;
                            child_input.write_all(b"\n").await.map_err(|e| e.to_string())?;
                            child_input.flush().await.map_err(|e| e.to_string())?;
                        }
                    }
                }
                line = child_output.next_line() => {
                    let Some(line) = line.map_err(|e| e.to_string())? else { break; };
                    match serde_json::from_str::<Value>(&line) {
                        Ok(mut message) if message.is_object() => {
                            if let Some(id) = mux.internal_response(&message) {
                                guard.reply(id, &message, now());
                                continue;
                            }
                            let original = mux.native_response(&mut message);
                            guard.native_server(&message, original.as_ref(), now());
                            if original.as_ref().is_some_and(|request| matches!(
                                request["method"].as_str(), Some("thread/start" | "thread/resume" | "thread/fork")
                            )) {
                                let _ownership = ownership_gate.lock()?;
                                for task in guard.snapshot() {
                                    register_thread(&registry, &mut registration, &task.id)?;
                                }
                            }
                            let mut stdout = std::io::stdout().lock();
                            serde_json::to_writer(&mut stdout, &message).map_err(|e| e.to_string())?;
                            stdout.write_all(b"\n").map_err(|e| e.to_string())?;
                            stdout.flush().map_err(|e| e.to_string())?;
                        }
                        _ => {
                            protocol_usable = false;
                            guard.disconnected(now());
                            let mut stdout = std::io::stdout().lock();
                            writeln!(stdout, "{line}").map_err(|e| e.to_string())?;
                            stdout.flush().map_err(|e| e.to_string())?;
                        }
                    }
                }
                Some(control) = control_rx.recv() => {
                    let installation_enabled = installation_gate.enforce(&mut guard, now());
                    last_control = Some(Instant::now());
                    lease_expired = false;
                    let error = match control.request {
                        ControlCommand::Snapshot => None,
                        ControlCommand::SetEnabled { thread_id, enabled } => {
                            if enabled && !installation_enabled {
                                Some("此桌面接入已关闭或更新，请重新打开 Codex 后再开启续跑".into())
                            } else if enabled && !protocol_usable {
                                Some("协议连接未经确认，请重新打开桌面连接".into())
                            } else {
                                guard.set_enabled(&thread_id, enabled, now()).err()
                            }
                        }
                        ControlCommand::SetPaused { paused } => {
                            if !paused && !installation_enabled {
                                Some("此桌面接入已失效，不能恢复自动续跑".into())
                            } else {
                                guard.set_paused(paused, now()).err()
                            }
                        }
                        ControlCommand::RevokeAll => { guard.control_lost(now()); None }
                    };
                    let _ = control.reply.send(ControlResponse {
                        generation: generation.clone(), tasks: guard.snapshot(), quota: guard.telemetry(), error,
                    });
                }
                _ = timer.tick() => {
                    let installation_enabled = installation_gate.enforce(&mut guard, now());
                    if last_control.is_some_and(|seen| seen.elapsed() > GUI_LEASE) && !lease_expired {
                        guard.control_lost(now());
                        lease_expired = true;
                    }
                    if protocol_usable && installation_enabled {
                        for request in guard.tick(now()) {
                            let mut ownership = None;
                            if request.method == "turn/continueIfFailed" {
                                ownership = Some(ownership_gate.lock()?);
                                let id = request.params["threadId"].as_str().unwrap_or_default();
                                if !ownership_unambiguous(&directory, &generation, id) {
                                    guard.not_dispatched(request.id, now());
                                    continue;
                                }
                                if !installation_gate.check() {
                                    guard.not_dispatched(request.id, now());
                                    guard.disconnected(now());
                                    continue;
                                }
                            }
                            // Guard persisted any execution intent before returning it.
                            write_message(&mut child_input, &mux.internal(request)).await?;
                            drop(ownership);
                        }
                    }
                }
            }
        }
        Ok::<(), String>(())
    }.await;
    guard.disconnected(now());
    accept_task.abort();
    drop(child_input);
    let status = match timeout(Duration::from_secs(5), child.wait()).await {
        Ok(status) => status.map_err(|e| e.to_string())?,
        Err(_) => {
            let _ = child.kill().await;
            child.wait().await.map_err(|e| e.to_string())?
        }
    };
    result?;
    Ok(status.code().unwrap_or(1))
}

async fn write_message<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    value: &Value,
) -> Result<(), String> {
    let mut bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    writer.write_all(&bytes).await.map_err(|e| e.to_string())?;
    writer.flush().await.map_err(|e| e.to_string())
}

async fn accept_controls(
    listener: TcpListener,
    registration: Registration,
    tx: mpsc::Sender<ControlEvent>,
) {
    let slots = std::sync::Arc::new(Semaphore::new(16));
    while let Ok((stream, address)) = listener.accept().await {
        if !address.ip().is_loopback() {
            continue;
        }
        let Ok(permit) = slots.clone().try_acquire_owned() else {
            continue;
        };
        let registration = registration.clone();
        let tx = tx.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let _ = timeout(CONTROL_TIMEOUT, serve_control(stream, &registration, tx)).await;
        });
    }
}

async fn serve_control(
    stream: TcpStream,
    registration: &Registration,
    tx: mpsc::Sender<ControlEvent>,
) -> Result<(), String> {
    let mut stream = BufReader::new(stream);
    let mut line = String::new();
    (&mut stream)
        .take(MAX_CONTROL_BYTES)
        .read_line(&mut line)
        .await
        .map_err(|e| e.to_string())?;
    if !line.ends_with('\n') {
        return Err("invalid control frame".into());
    }
    let request: ControlRequest =
        serde_json::from_str(&line).map_err(|_| "invalid control request")?;
    if request.generation != registration.generation || request.secret != registration.secret {
        return Err("invalid control capability".into());
    }
    let (reply, response) = oneshot::channel();
    tx.send(ControlEvent {
        request: request.command,
        reply,
    })
    .await
    .map_err(|_| "bridge closed")?;
    write_message(
        stream.get_mut(),
        &serde_json::to_value(response.await.map_err(|_| "bridge closed")?)
            .map_err(|e| e.to_string())?,
    )
    .await
}

pub struct DesktopHub {
    directory: PathBuf,
}

pub struct DesktopSnapshot {
    pub connected: bool,
    pub tasks: Vec<TaskView>,
    pub quota: Option<crate::quota::QuotaTelemetry>,
}

impl DesktopHub {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            directory: data_dir.join("desktop-bridge"),
        }
    }

    async fn instances(&self) -> Vec<(Registration, ControlResponse)> {
        let Ok(files) = fs::read_dir(&self.directory) else {
            return vec![];
        };
        let mut pending = futures_util::stream::FuturesUnordered::new();
        let mut files: Vec<_> = files.flatten().collect();
        files.sort_by_key(|file| {
            std::cmp::Reverse(file.metadata().and_then(|meta| meta.modified()).ok())
        });
        for file in files.into_iter().take(128) {
            if file
                .path()
                .extension()
                .is_none_or(|extension| extension != "json")
            {
                continue;
            }
            let Ok(registration) = read_registration(&file.path()) else {
                continue;
            };
            pending.push(async move {
                match control(&registration, ControlCommand::Snapshot).await {
                    Ok(response) => Some((registration, response)),
                    Err(error) => {
                        if error == "desktop_bridge_connection_refused" {
                            let _ = fs::remove_file(file.path());
                        }
                        None
                    }
                }
            });
        }
        use futures_util::StreamExt;
        let mut result = Vec::new();
        while let Some(instance) = pending.next().await {
            if let Some(instance) = instance {
                result.push(instance);
            }
        }
        result
    }

    pub async fn views(&self) -> Vec<TaskView> {
        self.snapshot().await.tasks
    }

    pub async fn snapshot(&self) -> DesktopSnapshot {
        let instances = self.instances().await;
        let connected = !instances.is_empty();
        let mut counts = HashMap::new();
        for (_, response) in &instances {
            for task in &response.tasks {
                *counts.entry(task.id.clone()).or_insert(0) += 1;
            }
        }
        // More than one execution environment may mean different accounts.
        // Do not blend their quotas into one apparently authoritative card.
        let quota = if instances.len() == 1 {
            instances[0].1.quota.clone()
        } else {
            None
        };
        let mut views = HashMap::new();
        for (registration, response) in instances {
            for mut task in response.tasks {
                if counts[&task.id] > 1 {
                    let _ = control(
                        &registration,
                        ControlCommand::SetEnabled {
                            thread_id: task.id.clone(),
                            enabled: false,
                        },
                    )
                    .await;
                    task.enabled = false;
                    task.can_enable = false;
                    task.state = crate::scheduler::TaskState::NeedsAttention;
                    task.detail = "同一聊天连接到多个执行端，已关闭自动续跑".into();
                }
                views.insert(task.id.clone(), task);
            }
        }
        let mut views: Vec<_> = views.into_values().collect();
        views.sort_by(|a, b| a.id.cmp(&b.id));
        DesktopSnapshot {
            connected,
            tasks: views,
            quota,
        }
    }

    pub async fn set_enabled(&self, id: &str, enabled: bool) -> Result<bool, String> {
        let instances = self.instances().await;
        let owners: Vec<_> = instances
            .iter()
            .filter(|(_, response)| response.tasks.iter().any(|task| task.id == id))
            .collect();
        if owners.is_empty() {
            return Ok(false);
        }
        if owners.len() != 1 && enabled {
            for (registration, _) in owners {
                let _ = control(
                    registration,
                    ControlCommand::SetEnabled {
                        thread_id: id.into(),
                        enabled: false,
                    },
                )
                .await;
            }
            return Err("同一聊天有多个执行端，请先关闭重复连接".into());
        }
        for (registration, _) in owners {
            control(
                registration,
                ControlCommand::SetEnabled {
                    thread_id: id.into(),
                    enabled,
                },
            )
            .await?;
        }
        Ok(true)
    }

    pub fn set_default_pause(&self, paused: bool) -> Result<(), String> {
        crate::store::Store::open(
            self.directory
                .parent()
                .ok_or("桌面数据目录无效")?
                .join("desktop-state.sqlite"),
        )
        .map_err(|error| error.to_string())?
        .set_paused(paused)
        .map_err(|error| error.to_string())
    }

    pub async fn pause_all(&self, paused: bool) -> Result<(), String> {
        // Commit the shared gate before contacting individual actors. A slow
        // control response must not leave another bridge eligible to dispatch.
        self.set_default_pause(paused)?;
        for (registration, _) in self.instances().await {
            control(&registration, ControlCommand::SetPaused { paused }).await?;
        }
        Ok(())
    }

    pub async fn revoke_all(&self) {
        for (registration, _) in self.instances().await {
            let _ = control(&registration, ControlCommand::RevokeAll).await;
        }
    }
}

async fn control(
    registration: &Registration,
    command: ControlCommand,
) -> Result<ControlResponse, String> {
    timeout(CONTROL_TIMEOUT, async {
        let stream = TcpStream::connect(("127.0.0.1", registration.port))
            .await
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::ConnectionRefused {
                    "desktop_bridge_connection_refused"
                } else {
                    "桌面桥接已断开"
                }
            })?;
        let mut stream = BufReader::new(stream);
        let request = ControlRequest {
            secret: registration.secret.clone(),
            generation: registration.generation.clone(),
            command,
        };
        write_message(
            stream.get_mut(),
            &serde_json::to_value(request).map_err(|e| e.to_string())?,
        )
        .await?;
        let mut line = String::new();
        (&mut stream)
            .take(MAX_CONTROL_BYTES)
            .read_line(&mut line)
            .await
            .map_err(|e| e.to_string())?;
        let response: ControlResponse =
            serde_json::from_str(&line).map_err(|_| "桌面桥接响应无效")?;
        if response.generation != registration.generation {
            return Err("桌面桥接实例已变化".into());
        }
        if let Some(error) = response.error {
            return Err(error);
        }
        Ok(response)
    })
    .await
    .map_err(|_| "桌面桥接响应超时".to_owned())?
}

fn read_registration(path: &Path) -> Result<Registration, String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > MAX_CONTROL_BYTES {
        return Err("invalid registration".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err("registration is not private".into());
        }
    }
    let registration: Registration =
        serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    if registration.version != 1
        || registration.port == 0
        || registration.secret.len() != 64
        || Uuid::parse_str(&registration.generation).is_err()
    {
        return Err("invalid registration version".into());
    }
    Ok(registration)
}

fn register_thread(path: &Path, registration: &mut Registration, id: &str) -> Result<(), String> {
    if registration
        .thread_ids
        .iter()
        .any(|existing| existing == id)
    {
        return Ok(());
    }
    registration.thread_ids.push(id.into());
    let temporary = path.with_extension("pending");
    private_file(
        &temporary,
        &serde_json::to_vec(registration).map_err(|e| e.to_string())?,
    )?;
    fs::rename(&temporary, path).map_err(|e| e.to_string())
}

fn ownership_unambiguous(directory: &Path, generation: &str, thread: &str) -> bool {
    let Ok(files) = fs::read_dir(directory) else {
        return false;
    };
    for entry in files {
        let Ok(entry) = entry else {
            return false;
        };
        if entry
            .path()
            .extension()
            .is_none_or(|extension| extension != "json")
        {
            continue;
        }
        let Ok(registration) = read_registration(&entry.path()) else {
            return false;
        };
        if registration.generation != generation
            && registration.thread_ids.iter().any(|id| id == thread)
        {
            return false;
        }
    }
    true
}
fn private_directory(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
            return Err("桌面桥接目录无效".into())
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path).map_err(|e| e.to_string())?
        }
        Err(error) => return Err(error.to_string()),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn private_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|e| e.to_string())?;
    file.write_all(bytes).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())
}

struct RegistryFile(PathBuf);
impl Drop for RegistryFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Separate from the task database: native observation may persist task state
/// while this lock is held. SQLite releases this cross-process lock on crashes.
struct OwnershipGate(rusqlite::Connection);

impl OwnershipGate {
    fn open(data_dir: &Path) -> Result<Self, String> {
        let connection = rusqlite::Connection::open(data_dir.join("desktop-ownership.sqlite"))
            .map_err(|error| error.to_string())?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(|error| error.to_string())?;
        connection
            .execute_batch("CREATE TABLE IF NOT EXISTS bridge_lock (id INTEGER PRIMARY KEY)")
            .map_err(|error| error.to_string())?;
        Ok(Self(connection))
    }

    fn lock(&mut self) -> Result<rusqlite::Transaction<'_>, String> {
        self.0
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|_| "无法取得桌面连接协调锁，已停止桥接".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed_gate(path: &Path, token: &str) -> InstallationGate {
        InstallationGate::new(Some(path.as_os_str().into()), Some(token.into()))
    }

    #[test]
    fn installation_gate_requires_both_variables_or_explicit_manual_mode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("enabled");
        private_file(&path, b"runtime-one").unwrap();
        assert!(InstallationGate::new(None, None).check());
        assert!(!InstallationGate::new(Some(path.as_os_str().into()), None).check());
        assert!(!InstallationGate::new(None, Some("runtime-one".into())).check());
        assert!(!installed_gate(Path::new("relative"), "runtime-one").check());
        assert!(!installed_gate(&path, "").check());
        assert!(!installed_gate(&path, &"x".repeat(129)).check());
        assert!(installed_gate(&path, "runtime-one").check());
        // Whitespace cannot turn a different file payload into authorization.
        fs::write(&path, "runtime-one\n").unwrap();
        assert!(!installed_gate(&path, "runtime-one").check());
    }

    #[test]
    fn installation_revocation_never_revives_after_reenable_or_runtime_upgrade() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("enabled");
        private_file(&path, b"runtime-one").unwrap();
        let mut disabled = installed_gate(&path, "runtime-one");
        assert!(disabled.check());
        fs::remove_file(&path).unwrap();
        assert!(!disabled.check());
        private_file(&path, b"runtime-one").unwrap();
        assert!(!disabled.check());

        let mut upgraded = installed_gate(&path, "runtime-one");
        assert!(upgraded.check());
        fs::write(&path, "runtime-two").unwrap();
        assert!(!upgraded.check());
        assert!(installed_gate(&path, "runtime-two").check());
        fs::write(&path, "runtime-one").unwrap();
        assert!(!upgraded.check());
    }

    #[test]
    fn invalid_installation_cannot_be_fixed_inside_an_already_started_generation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("enabled");
        let mut missing = installed_gate(&path, "runtime-one");
        private_file(&path, b"runtime-one").unwrap();
        assert!(!missing.check());
        let mut current = installed_gate(&path, "runtime-one");
        fs::write(&path, vec![b'x'; 129]).unwrap();
        assert!(!current.check());
        fs::write(&path, "runtime-one").unwrap();
        assert!(!current.check());
        assert!(!installed_gate(dir.path(), "runtime-one").check());
    }

    #[cfg(unix)]
    #[test]
    fn installation_gate_refuses_symlinks_public_files_and_non_utf8_tokens() {
        use std::os::unix::{
            ffi::OsStringExt,
            fs::{symlink, PermissionsExt},
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("enabled");
        let link = dir.path().join("link");
        private_file(&path, b"runtime-one").unwrap();
        symlink(&path, &link).unwrap();
        assert!(!installed_gate(&link, "runtime-one").check());
        assert!(!InstallationGate::new(
            Some(path.as_os_str().into()),
            Some(OsString::from_vec(vec![0xff]))
        )
        .check());
        let mut current = installed_gate(&path, "runtime-one");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!current.check());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(!current.check());
    }

    #[test]
    fn installation_revocation_clears_consent_and_snapshot_readiness() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("enabled");
        private_file(&path, b"runtime-one").unwrap();
        let mut gate = installed_gate(&path, "runtime-one");
        let mut guard = DesktopGuard::open(&dir.path().join("state.sqlite")).unwrap();
        guard.native_client(&json!({"method":"initialized","params":{}}), 100);
        let request = guard.tick(100).pop().unwrap();
        guard.reply(
            request.id,
            &json!({"result":{"version":1,"conditionalContinuation":true}}),
            100,
        );
        guard.native_server(
            &json!({"id":1,"result":{"thread":{
                "id":"thread","name":"Test","cwd":"/test","turns":[]
            }}}),
            Some(&json!({"method":"thread/resume","params":{"threadId":"thread"}})),
            100,
        );
        guard.set_enabled("thread", true, 100).unwrap();
        assert!(gate.enforce(&mut guard, 100));
        assert!(guard.snapshot()[0].enabled);
        fs::remove_file(&path).unwrap();
        assert!(!gate.enforce(&mut guard, 101));
        let snapshot = guard.snapshot();
        assert!(!snapshot[0].enabled);
        assert!(!snapshot[0].can_enable);
        assert!(guard.tick(101).is_empty());
        private_file(&path, b"runtime-two").unwrap();
        // Even a later native lifecycle event cannot restore this gate.
        guard.native_client(&json!({"method":"initialized","params":{}}), 102);
        assert!(!gate.enforce(&mut guard, 102));
        assert!(guard.set_enabled("thread", true, 102).is_err());
        assert!(guard.tick(102).is_empty());
    }

    #[test]
    fn default_pause_is_durable_without_contacting_retired_actors() {
        let dir = tempfile::tempdir().unwrap();
        let hub = DesktopHub::new(dir.path());
        hub.set_default_pause(true).unwrap();
        let store = crate::store::Store::open(dir.path().join("desktop-state.sqlite")).unwrap();
        assert!(store.is_paused().unwrap());
        hub.set_default_pause(false).unwrap();
        assert!(!store.is_paused().unwrap());
        assert!(!dir.path().join("desktop-bridge").exists());
    }

    #[test]
    fn ids_preserve_native_types_and_never_collide_with_internal_probes() {
        let mut mux = RpcMux::new("g");
        let mut numeric = json!({"id":1,"method":"account/read","params":{}});
        let mut string =
            json!({"id":"autoresume:g:1","method":"thread/read","params":{"threadId":"t"}});
        mux.client(&mut numeric);
        mux.client(&mut string);
        assert_ne!(numeric["id"], string["id"]);
        let internal = mux.internal(GuardRequest {
            id: 1,
            method: "autoresume/capabilities".into(),
            params: json!({}),
        });
        assert_ne!(internal["id"], string["id"]);
        let mut first = json!({"id":numeric["id"],"result":{}});
        let mut second = json!({"id":string["id"],"result":{}});
        assert!(mux.native_response(&mut first).is_some());
        assert!(mux.native_response(&mut second).is_some());
        assert_eq!(first["id"], 1);
        assert_eq!(second["id"], "autoresume:g:1");
        assert_eq!(
            mux.internal_response(&json!({"id":internal["id"],"result":{}})),
            Some(1)
        );
    }

    #[test]
    fn server_approval_and_native_reply_keep_original_id_and_payload() {
        let mut mux = RpcMux::new("g");
        let mut request = json!({"id":"approval-1","method":"item/commandExecution/requestApproval","params":{"threadId":"t","command":"example"}});
        let original = request.clone();
        assert!(mux.native_response(&mut request).is_none());
        assert!(mux.internal_response(&request).is_none());
        assert_eq!(request, original);
        let mut reply = json!({"id":"approval-1","result":{"decision":"decline"}});
        let original = reply.clone();
        mux.client(&mut reply);
        assert_eq!(reply, original);
    }

    #[test]
    fn pending_request_does_not_retain_prompt_or_login_credentials() {
        let mut mux = RpcMux::new("g");
        let mut request = json!({"id":5,"method":"account/login/start","params":{"token":"test-secret","input":"private prompt"}});
        mux.client(&mut request);
        let mut response = json!({"id":request["id"],"result":{}});
        let remembered = mux.native_response(&mut response).unwrap();
        assert_eq!(
            remembered,
            json!({"method":"account/login/start","params":{"threadId":null}})
        );
    }

    #[tokio::test]
    async fn wrong_control_secret_cannot_reach_the_actor() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let registration = Registration {
            version: 1,
            generation: Uuid::new_v4().to_string(),
            port,
            secret: "a".repeat(64),
            thread_ids: vec![],
        };
        let (tx, mut rx) = mpsc::channel(1);
        let task = tokio::spawn(accept_controls(listener, registration.clone(), tx));
        let mut wrong = registration;
        wrong.secret = "b".repeat(64);
        assert!(control(&wrong, ControlCommand::Snapshot).await.is_err());
        assert!(rx.try_recv().is_err());
        task.abort();
    }

    #[tokio::test]
    async fn authenticated_control_roundtrips_and_checks_generation() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let registration = Registration {
            version: 1,
            generation: Uuid::new_v4().to_string(),
            port: listener.local_addr().unwrap().port(),
            secret: "a".repeat(64),
            thread_ids: vec![],
        };
        let (tx, mut rx) = mpsc::channel::<ControlEvent>(1);
        let task = tokio::spawn(accept_controls(listener, registration.clone(), tx));
        let generation = registration.generation.clone();
        let responder = tokio::spawn(async move {
            let event = rx.recv().await.unwrap();
            assert!(
                matches!(event.request,ControlCommand::SetEnabled {ref thread_id,enabled:true} if thread_id=="existing")
            );
            event
                .reply
                .send(ControlResponse {
                    generation,
                    tasks: vec![],
                    quota: None,
                    error: None,
                })
                .ok();
        });
        control(
            &registration,
            ControlCommand::SetEnabled {
                thread_id: "existing".into(),
                enabled: true,
            },
        )
        .await
        .unwrap();
        responder.await.unwrap();
        task.abort();
    }

    #[test]
    fn a_second_declared_owner_blocks_submission_before_gui_polling() {
        let directory = tempfile::tempdir().unwrap();
        let mut registration = Registration {
            version: 1,
            generation: Uuid::new_v4().to_string(),
            port: 12345,
            secret: "a".repeat(64),
            thread_ids: vec![],
        };
        let path = directory
            .path()
            .join(format!("{}.json", registration.generation));
        private_file(&path, &serde_json::to_vec(&registration).unwrap()).unwrap();
        let first_generation = Uuid::new_v4().to_string();
        assert!(ownership_unambiguous(
            directory.path(),
            &first_generation,
            "original"
        ));
        register_thread(&path, &mut registration, "original").unwrap();
        assert!(!ownership_unambiguous(
            directory.path(),
            &first_generation,
            "original"
        ));
        assert!(ownership_unambiguous(
            directory.path(),
            &registration.generation,
            "original"
        ));
        assert!(ownership_unambiguous(
            directory.path(),
            &first_generation,
            "other"
        ));
    }

    #[test]
    fn ownership_declaration_and_conditional_send_cannot_overlap() {
        let directory = tempfile::tempdir().unwrap();
        let mut first = OwnershipGate::open(directory.path()).unwrap();
        let mut second = OwnershipGate::open(directory.path()).unwrap();
        second.0.busy_timeout(Duration::ZERO).unwrap();
        let dispatch = first.lock().unwrap();
        assert!(second.lock().is_err());
        drop(dispatch);
        assert!(second.lock().is_ok());
    }

    #[tokio::test]
    async fn dead_registration_does_not_hide_a_live_instance() {
        let directory = tempfile::tempdir().unwrap();
        let registry = directory.path().join("desktop-bridge");
        private_directory(&registry).unwrap();
        let dead_listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let dead = Registration {
            version: 1,
            generation: Uuid::new_v4().to_string(),
            port: dead_listener.local_addr().unwrap().port(),
            secret: "a".repeat(64),
            thread_ids: vec![],
        };
        drop(dead_listener);
        let dead_path = registry.join(format!("{}.json", dead.generation));
        private_file(&dead_path, &serde_json::to_vec(&dead).unwrap()).unwrap();
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let live = Registration {
            version: 1,
            generation: Uuid::new_v4().to_string(),
            port: listener.local_addr().unwrap().port(),
            secret: "b".repeat(64),
            thread_ids: vec![],
        };
        private_file(
            &registry.join(format!("{}.json", live.generation)),
            &serde_json::to_vec(&live).unwrap(),
        )
        .unwrap();
        let (tx, mut rx) = mpsc::channel::<ControlEvent>(1);
        let accept = tokio::spawn(accept_controls(listener, live.clone(), tx));
        let generation = live.generation.clone();
        let reply = tokio::spawn(async move {
            let event = rx.recv().await.unwrap();
            event
                .reply
                .send(ControlResponse {
                    generation,
                    tasks: vec![],
                    quota: None,
                    error: None,
                })
                .ok();
        });
        let snapshot = DesktopHub::new(directory.path()).snapshot().await;
        // A connected Desktop with no open chats must not be shown as absent.
        assert!(snapshot.connected);
        assert!(snapshot.tasks.is_empty());
        assert!(snapshot.quota.is_none());
        assert!(!dead_path.exists());
        reply.await.unwrap();
        accept.abort();
        let _ = accept.await;
        assert!(!DesktopHub::new(directory.path()).snapshot().await.connected);
    }
}
