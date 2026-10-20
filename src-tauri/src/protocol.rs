//! Observation-only task adapter, plus explicitly user-initiated official login.
//! Adding a write method requires a verified ownership/configuration protocol.
use serde_json::{json, Value};
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
    process::{Child, Command},
    sync::mpsc,
    time::timeout,
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_FRAME: usize = 16 * 1024 * 1024;

pub struct Rpc {
    child: Option<Child>,
    input: Box<dyn AsyncWrite + Unpin + Send>,
    output: mpsc::Receiver<Result<Value, String>>,
    reader: tokio::task::JoinHandle<()>,
    next_id: u64,
    request_timeout: Duration,
    unusable: bool,
    pub version: String,
}

impl Drop for Rpc {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

pub(crate) fn executable() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("CODEX_BIN").map(PathBuf::from) {
        return path.is_file().then_some(path);
    }
    if let Ok(path) = which::which("codex") {
        return Some(path);
    }
    let mut candidates = vec![
        PathBuf::from("/opt/homebrew/bin/codex"),
        PathBuf::from("/usr/local/bin/codex"),
        PathBuf::from("/Applications/Codex.app/Contents/Resources/codex"),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        candidates.push(home.join(".local/bin/codex"));
        if let Ok(versions) = std::fs::read_dir(home.join(".nvm/versions/node")) {
            let mut versions: Vec<_> = versions.flatten().map(|p| p.path()).collect();
            versions.sort();
            candidates.extend(versions.into_iter().rev().map(|p| p.join("bin/codex")));
        }
    }
    #[cfg(windows)]
    if let Some(appdata) = std::env::var_os("APPDATA") {
        candidates.push(PathBuf::from(appdata).join("npm/codex.cmd"));
    }
    candidates.into_iter().find(|p| p.is_file())
}

pub(crate) fn command(path: &std::path::Path) -> Result<Command, String> {
    // npm launchers need Node on PATH when the application is launched from Finder.
    // On Windows, launch npm's JS entry with Node directly instead of a shell.
    let mut cmd = if path.extension().is_some_and(|e| e == "cmd" || e == "ps1") {
        let parent = path.parent().ok_or("Codex 路径无效")?;
        let script = parent.join("node_modules/@openai/codex/bin/codex.js");
        if !script.is_file() {
            return Err("未找到 Codex 运行组件，请重新安装官方 Codex CLI".into());
        }
        let mut cmd = Command::new("node");
        cmd.arg(script);
        cmd
    } else {
        Command::new(path)
    };
    if let Some(parent) = path.parent() {
        let mut paths = vec![parent.to_owned()];
        if let Some(existing) = std::env::var_os("PATH") {
            paths.extend(std::env::split_paths(&existing));
        }
        if let Ok(path) = std::env::join_paths(paths) {
            cmd.env("PATH", path);
        }
    }
    cmd.kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    Ok(cmd)
}

impl Rpc {
    pub async fn connect() -> Result<Self, String> {
        let path = executable().ok_or("missing:未找到 Codex，请先安装官方 Codex CLI")?;
        let output = timeout(REQUEST_TIMEOUT, command(&path)?.arg("--version").output())
            .await
            .map_err(|_| "Codex 版本检查超时")?
            .map_err(|_| "无法运行 Codex，请检查安装")?;
        if !output.status.success() {
            return Err("Codex 版本检查失败".into());
        }
        let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        let mut child = command(&path)?
            .arg("app-server")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "无法启动 Codex App Server")?;
        let input = child.stdin.take().ok_or("无法连接 Codex 输入")?;
        let stdout = child.stdout.take().ok_or("无法连接 Codex 输出")?;
        let mut rpc = Self::from_io(Some(child), input, stdout, version);
        rpc.handshake().await?;
        Ok(rpc)
    }

    // Both a child process and test fixtures use the same bounded JSONL transport.
    pub(crate) fn from_io(
        child: Option<Child>,
        input: impl AsyncWrite + Unpin + Send + 'static,
        stdout: impl AsyncRead + Unpin + Send + 'static,
        version: String,
    ) -> Self {
        let (send, output) = mpsc::channel(32);
        let reader = tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            loop {
                // Enforce the cap before growing the buffer; oversized history fails closed.
                let mut line = Vec::new();
                let mut oversized = false;
                loop {
                    let chunk = match reader.fill_buf().await {
                        Ok(chunk) => chunk,
                        Err(_) => {
                            let _ = send.send(Err("Codex 连接读取失败".into())).await;
                            return;
                        }
                    };
                    if chunk.is_empty() {
                        break;
                    }
                    let end = chunk.iter().position(|b| *b == b'\n').map(|i| i + 1);
                    let count = end.unwrap_or(chunk.len());
                    if line.len() + count > MAX_FRAME {
                        oversized = true;
                        break;
                    }
                    line.extend_from_slice(&chunk[..count]);
                    reader.consume(count);
                    if end.is_some() {
                        break;
                    }
                }
                if oversized {
                    let _ = send
                        .send(Err("会话数据过大，请在原客户端查看".into()))
                        .await;
                    return;
                }
                if line.is_empty() {
                    break;
                }
                let value =
                    serde_json::from_slice(&line).map_err(|_| "Codex 返回了无法识别的数据".into());
                if send.send(value).await.is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            input: Box::new(input),
            output,
            reader,
            next_id: 0,
            request_timeout: REQUEST_TIMEOUT,
            unusable: false,
            version,
        }
    }

    async fn handshake(&mut self) -> Result<(), String> {
        self.request("initialize", json!({"clientInfo":{"name":"codex_autoresume","title":"Codex AutoResume","version":env!("CARGO_PKG_VERSION")}})).await?;
        self.send(json!({"method":"initialized","params":{}})).await
    }

    async fn send(&mut self, value: Value) -> Result<(), String> {
        let mut bytes = serde_json::to_vec(&value).map_err(|_| "请求编码失败")?;
        bytes.push(b'\n');
        self.input
            .write_all(&bytes)
            .await
            .map_err(|_| "Codex 连接已断开".to_string())
    }

    pub fn is_usable(&self) -> bool {
        !self.unusable
    }

    pub async fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        if self.unusable {
            return Err("Codex 连接已关闭，请重新连接".into());
        }
        if !matches!(
            method,
            "initialize"
                | "account/read"
                | "account/rateLimits/read"
                | "thread/list"
                | "thread/read"
                | "account/login/start"
        ) {
            return Err("当前版本未开放任务执行：尚不能验证跨客户端接管安全性".into());
        }
        self.next_id += 1;
        let id = self.next_id;
        let result = timeout(self.request_timeout, async {
            self.send(json!({"id":id,"method":method,"params":params})).await?;
            loop {
                let value = self.output.recv().await.ok_or("Codex 连接已关闭")??;
                if !value.is_object() {
                    return Err("Codex 返回结果格式不兼容".into());
                }
                if let Some(method) = value.get("method") {
                    if !method.is_string() {
                        return Err("Codex 返回结果格式不兼容".into());
                    }
                }
                if value.get("method").is_some() && value.get("id").is_some() {
                    // Never approve a server request or synthesize user input.
                    self.send(json!({"id":value["id"],"error":{"code":-32601,"message":"AutoResume observation client does not handle execution requests"}})).await?;
                    continue;
                }
                if value.get("method").is_some() { continue; }
                if value.get("id").is_none() {
                    return Err("Codex 返回结果格式不兼容".into());
                }
                if value.get("id").and_then(Value::as_u64) != Some(id) { continue; }
                if value.get("result").is_some() && value.get("error").is_some() {
                    return Err("Codex 返回结果格式不兼容".into());
                }
                if let Some(error) = value.get("error") {
                    // Do not leak server errors containing user paths, content or credentials to logs.
                    let code = error.get("code").and_then(Value::as_i64)
                        .ok_or("Codex 返回结果格式不兼容")?;
                    if error.get("message").and_then(Value::as_str).is_none() {
                        return Err("Codex 返回结果格式不兼容".into());
                    }
                    // A correlated RPC rejection is a complete response. The
                    // transport is healthy and unrelated reads may continue.
                    return Ok(Err(format!("Codex 请求失败（{method}，错误 {code}）")));
                }
                return value.get("result").cloned().map(Ok).ok_or("Codex 返回结果格式不兼容".into());
            }
        }).await.unwrap_or_else(|_| Err("Codex 请求超时，请重新连接".into()));
        match result {
            Ok(response) => response,
            Err(error) => {
                // A timeout or lost response is not permission to replay an operation.
                // Retire this transport, including the login exception, before returning.
                self.unusable = true;
                self.reader.abort();
                self.child.take(); // kill_on_drop was set when spawning the process.
                let _ = self.input.shutdown().await;
                Err(error)
            }
        }
    }
}

pub fn terminal_failure(thread: &Value) -> Option<(&str, &str, bool)> {
    let turn = thread.get("turns")?.as_array()?.last()?;
    Some((
        turn.get("id")?.as_str()?,
        turn.get("status")?.as_str()?,
        turn.pointer("/error/codexErrorInfo")
            .and_then(Value::as_str)
            == Some("usageLimitExceeded"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{split, DuplexStream, ReadHalf, WriteHalf};

    type FixtureReader = BufReader<ReadHalf<DuplexStream>>;
    type FixtureWriter = WriteHalf<DuplexStream>;

    #[cfg(unix)]
    #[tokio::test]
    async fn npm_launcher_finds_its_sibling_node_when_started_outside_a_shell() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let launcher = directory.path().join("codex");
        let node = directory.path().join("node");
        std::fs::write(&launcher, "#!/usr/bin/env node\n").unwrap();
        std::fs::write(&node, "#!/bin/sh\nprintf 'fixture-node\\n'\n").unwrap();
        for path in [&launcher, &node] {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let output = command(&launcher)
            .unwrap()
            .arg("--version")
            .output()
            .await
            .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"fixture-node\n");
    }

    fn fixture() -> (Rpc, FixtureReader, FixtureWriter) {
        let (client, server) = tokio::io::duplex(64 * 1024);
        let (client_read, client_write) = split(client);
        let (server_read, server_write) = split(server);
        (
            Rpc::from_io(None, client_write, client_read, "fixture".into()),
            BufReader::new(server_read),
            server_write,
        )
    }

    async fn read_message(reader: &mut FixtureReader) -> Value {
        let mut line = String::new();
        let bytes = timeout(Duration::from_secs(2), reader.read_line(&mut line))
            .await
            .expect("fixture read timed out")
            .expect("fixture read failed");
        assert_ne!(bytes, 0, "unexpected transport EOF");
        serde_json::from_str(&line).expect("client must emit valid JSONL")
    }

    async fn write_message(writer: &mut FixtureWriter, value: Value) {
        let mut bytes = serde_json::to_vec(&value).unwrap();
        bytes.push(b'\n');
        writer.write_all(&bytes).await.unwrap();
    }

    async fn expect_eof(reader: &mut FixtureReader) {
        let mut line = String::new();
        let bytes = timeout(Duration::from_secs(2), reader.read_line(&mut line))
            .await
            .expect("failed transport was not closed")
            .unwrap();
        assert_eq!(bytes, 0, "failed transport must not submit another request");
    }

    #[tokio::test]
    async fn handshake_waits_for_initialize_response_before_acknowledging() {
        let (mut rpc, mut reader, mut writer) = fixture();
        let server = tokio::spawn(async move {
            let request = read_message(&mut reader).await;
            assert_eq!(request["method"], "initialize");
            assert_eq!(request["params"]["clientInfo"]["name"], "codex_autoresume");
            let mut premature = String::new();
            assert!(
                timeout(Duration::from_millis(20), reader.read_line(&mut premature))
                    .await
                    .is_err()
            );
            write_message(
                &mut writer,
                json!({"id":request["id"],"result":{"userAgent":"fixture"}}),
            )
            .await;
            let acknowledged = read_message(&mut reader).await;
            assert_eq!(acknowledged["method"], "initialized");
            assert!(acknowledged.get("id").is_none());
        });
        rpc.handshake().await.unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn responses_are_correlated_by_id_across_fragmented_jsonl() {
        let (mut rpc, mut reader, mut writer) = fixture();
        let server = tokio::spawn(async move {
            let first = read_message(&mut reader).await;
            write_message(
                &mut writer,
                json!({"method":"account/updated","params":{"authMode":"chatgpt"}}),
            )
            .await;
            write_message(&mut writer, json!({"id":999,"result":{"wrong":true}})).await;
            let response = format!(
                "{}\n",
                json!({"id":first["id"],"result":{"account":{"type":"chatgpt"}}})
            );
            let midpoint = response.len() / 2;
            writer
                .write_all(&response.as_bytes()[..midpoint])
                .await
                .unwrap();
            tokio::task::yield_now().await;
            writer
                .write_all(&response.as_bytes()[midpoint..])
                .await
                .unwrap();
            let second = read_message(&mut reader).await;
            assert_ne!(first["id"], second["id"]);
            write_message(
                &mut writer,
                json!({"id":second["id"],"result":{"data":[],"nextCursor":null}}),
            )
            .await;
        });
        let first = rpc
            .request("account/read", json!({"refreshToken":false}))
            .await
            .unwrap();
        assert_eq!(first["account"]["type"], "chatgpt");
        let second = rpc
            .request("thread/list", json!({"useStateDbOnly":true}))
            .await
            .unwrap();
        assert_eq!(second["data"], json!([]));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn server_requests_are_rejected_without_approval_or_credentials() {
        let (mut rpc, mut reader, mut writer) = fixture();
        let server = tokio::spawn(async move {
            let pending = read_message(&mut reader).await;
            for (index, method) in [
                "item/commandExecution/requestApproval",
                "tool/requestUserInput",
                "account/chatgptAuthTokens/refresh",
            ]
            .iter()
            .enumerate()
            {
                let id = format!("server-{index}");
                write_message(&mut writer, json!({"id":id,"method":method,"params":{}})).await;
                let rejected = read_message(&mut reader).await;
                assert_eq!(rejected["id"], id);
                assert_eq!(rejected["error"]["code"], -32601);
                assert!(rejected.get("result").is_none());
            }
            write_message(
                &mut writer,
                json!({"id":pending["id"],"result":{"thread":{"id":"fixture-thread"}}}),
            )
            .await;
        });
        let result = rpc
            .request("thread/read", json!({"threadId":"fixture-thread"}))
            .await
            .unwrap();
        assert_eq!(result["thread"]["id"], "fixture-thread");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn malformed_frames_close_transport_and_do_not_leak_payloads() {
        for payload in [
            "{fixture-private-content broken\n",
            "[]\n",
            "{\"id\":1}\n",
            "{\"id\":1,\"result\":{},\"error\":{\"code\":1}}\n",
            "{\"method\":false,\"id\":1}\n",
            "{\"unexpected\":true}\n",
            "{\"id\":1,\"error\":{\"code\":\"invalid\",\"message\":\"private\"}}\n",
            "{\"id\":1,\"error\":{\"code\":-32000}}\n",
        ] {
            let (mut rpc, mut reader, mut writer) = fixture();
            let server = tokio::spawn(async move {
                read_message(&mut reader).await;
                writer.write_all(payload.as_bytes()).await.unwrap();
                expect_eof(&mut reader).await;
            });
            let error = rpc
                .request("account/read", json!({"refreshToken":false}))
                .await
                .unwrap_err();
            assert!(!error.contains("fixture-private-content"));
            assert!(rpc.unusable);
            assert!(rpc.request("account/read", json!({})).await.is_err());
            assert_eq!(rpc.next_id, 1);
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn eof_without_response_closes_transport_without_retry() {
        let (mut rpc, mut reader, mut writer) = fixture();
        let server = tokio::spawn(async move {
            read_message(&mut reader).await;
            writer.shutdown().await.unwrap();
            drop(writer);
            expect_eof(&mut reader).await;
        });
        assert!(rpc
            .request("thread/read", json!({"threadId":"fixture-thread"}))
            .await
            .is_err());
        assert!(rpc.unusable);
        assert!(rpc
            .request("thread/read", json!({"threadId":"fixture-thread"}))
            .await
            .is_err());
        assert_eq!(rpc.next_id, 1);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn timeout_retires_transport_and_never_replays_login() {
        let (mut rpc, mut reader, writer) = fixture();
        rpc.request_timeout = Duration::from_millis(25);
        let server = tokio::spawn(async move {
            let request = read_message(&mut reader).await;
            assert_eq!(request["method"], "account/login/start");
            // Hold stdout open so the client must reach its timeout, not EOF.
            expect_eof(&mut reader).await;
            drop(writer);
        });
        let error = rpc
            .request("account/login/start", json!({"type":"chatgpt"}))
            .await
            .unwrap_err();
        assert!(error.contains("超时"));
        assert!(rpc.unusable);
        assert!(rpc
            .request("account/login/start", json!({"type":"chatgpt"}))
            .await
            .is_err());
        assert_eq!(rpc.next_id, 1);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn execution_and_account_write_methods_never_reach_transport() {
        let (mut rpc, mut reader, _writer) = fixture();
        for method in [
            "thread/resume",
            "turn/start",
            "turn/interrupt",
            "config/value/write",
            "account/logout",
            "account/rateLimitResetCredit/consume",
        ] {
            assert!(rpc.request(method, json!({})).await.is_err());
        }
        assert_eq!(rpc.next_id, 0);
        drop(rpc);
        expect_eof(&mut reader).await;
    }

    #[tokio::test]
    async fn remote_error_exposes_only_method_and_code_and_keeps_transport_usable() {
        let (mut rpc, mut reader, mut writer) = fixture();
        let server = tokio::spawn(async move {
            let request = read_message(&mut reader).await;
            write_message(&mut writer, json!({"id":request["id"],"error":{"code":-32000,"message":"fixture-secret-content","data":{"token":"fixture-secret-token"}}})).await;
            let next = read_message(&mut reader).await;
            assert_eq!(next["method"], "thread/list");
            write_message(
                &mut writer,
                json!({"id":next["id"],"result":{"data":[],"nextCursor":null}}),
            )
            .await;
        });
        let error = rpc
            .request("account/read", json!({"refreshToken":false}))
            .await
            .unwrap_err();
        assert!(error.contains("account/read"));
        assert!(error.contains("-32000"));
        assert!(!error.contains("fixture-secret"));
        assert!(rpc.is_usable());
        assert_eq!(
            rpc.request("thread/list", json!({})).await.unwrap()["data"],
            json!([])
        );
        server.await.unwrap();
    }

    #[test]
    fn quota_message_text_is_not_evidence() {
        let thread = json!({"turns":[{"id":"1","status":"failed","error":{"message":"usageLimitExceeded","codexErrorInfo":"other"}}]});
        assert_eq!(terminal_failure(&thread), Some(("1", "failed", false)));
    }
    #[test]
    fn only_latest_turn_is_relevant() {
        let thread = json!({"turns":[{"id":"1","status":"failed","error":{"codexErrorInfo":"usageLimitExceeded"}},{"id":"2","status":"completed","error":null}]});
        assert_eq!(terminal_failure(&thread), Some(("2", "completed", false)));
    }
}
