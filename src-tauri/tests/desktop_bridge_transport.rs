//! Real wrapper/stdio/control-plane integration with a synthetic local backend.
//! This never starts Codex, reads account state, or submits an actual model turn.
#![cfg(target_os = "macos")]

use serde_json::{json, Value};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    net::TcpStream,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;

const BACKEND: &str = r#"#!/usr/bin/python3
import json, os, sys

thread_id = 'original-desktop-chat'
held_quota = None

def emit(message):
    print(json.dumps(message), flush=True)

def reply(request, result):
    emit({'id':request['id'], 'result':result})

def turn(turn_id, status, code=None):
    return {'id':turn_id, 'status':status, 'items':[], 'itemsView':'full',
            'error':None if code is None else {'codexErrorInfo':code}}

def event(turn_id, status, code=None):
    emit({'method':'turn/started' if status == 'inProgress' else 'turn/completed',
          'params':{'threadId':thread_id, 'turn':turn(turn_id, status, code)}})

def quota():
    bucket = {'limitId':'codex','limitName':'Codex','normalModelSlug':None,
              'primary':{'usedPercent':0,'windowDurationMins':300,'resetsAt':None},
              'secondary':None,'credits':None,'individualLimit':None,
              'spendControlReached':False,'planType':'pro','rateLimitReachedType':None}
    return {'rateLimits':bucket,'rateLimitsByLimitId':{'codex':bucket},
            'rateLimitResetCredits':None,'ordinaryUsageAllowed':True,
            'accountId':None,'rateLimitUpsell':None}

for line in sys.stdin:
    message = json.loads(line)
    with open(os.environ['AUTORESUME_TEST_BACKEND_LOG'], 'a') as log:
        log.write(json.dumps(message) + '\n')
    method = message.get('method')
    if method == 'initialize':
        reply(message, {'userAgent':'synthetic-desktop'})
    elif method == 'initialized':
        pass
    elif method == 'autoresume/capabilities':
        reply(message, {'version':1, 'conditionalContinuation':True})
    elif method == 'thread/resume':
        reply(message, {'thread':{'id':thread_id,'name':'Synthetic existing task','cwd':'/synthetic-project','turns':[]}})
    elif method == 'turn/start':
        text = message['params']['input'][0]['text']
        if text == 'NEW USER WORK':
            reply(message, {'turn':turn('new-user-turn','inProgress')})
            event('new-user-turn','inProgress')
            event('new-user-turn','completed')
            if held_quota is not None:
                reply(held_quota, quota())
                held_quota = None
        else:
            reply(message, {'turn':turn('quota-failed-turn','inProgress')})
            event('quota-failed-turn','inProgress')
            emit({'id':'native-tool-1','method':'item/tool/call','params':{
                'threadId':thread_id,'turnId':'quota-failed-turn','callId':'call-1',
                'tool':'original_desktop_tool','arguments':{'sample':'unchanged'}}})
    elif method is None and message.get('id') == 'native-tool-1':
        event('quota-failed-turn','failed','usageLimitExceeded')
    elif method == 'account/read':
        reply(message, {'account':{'type':'chatgpt'}})
    elif method == 'account/rateLimits/read':
        if os.environ.get('AUTORESUME_TEST_HOLD_QUOTA') == '1':
            held_quota = message
        else:
            reply(message, quota())
    elif method == 'turn/continueIfFailed':
        assert message['params'] == {'threadId':thread_id,'expectedPreviousTurnId':'quota-failed-turn'}
        reply(message, {'status':'started','turnId':'automatic-continuation'})
        event('automatic-continuation','inProgress')
        emit({'method':'item/agentMessage/delta','params':{'threadId':thread_id,'turnId':'automatic-continuation','delta':'synthetic result'}})
        event('automatic-continuation','completed')
    else:
        raise RuntimeError('unexpected synthetic request: ' + str(method))
"#;

struct Fixture {
    _directory: TempDir,
    data: PathBuf,
    log: PathBuf,
    child: Child,
    input: Option<ChildStdin>,
    output: Receiver<Value>,
    registration: Value,
}

impl Fixture {
    fn start(hold_quota: bool) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let data = directory.path().join("data");
        fs::create_dir(&data).unwrap();
        fs::set_permissions(&data, fs::Permissions::from_mode(0o700)).unwrap();
        let official = directory.path().join("synthetic-official");
        let backend = directory.path().join("synthetic-backend");
        // Two distinct executables are required; the official fallback must
        // never run in this test's recognized app-server invocation.
        fs::write(&official, "#!/bin/sh\nexit 91\n").unwrap();
        fs::write(&backend, BACKEND).unwrap();
        for path in [&official, &backend] {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let config = directory.path().join("config.json");
        fs::write(
            &config,
            serde_json::to_vec(&json!({
                "officialCli":official,"guardedAppServer":backend,"dataDir":data
            }))
            .unwrap(),
        )
        .unwrap();
        fs::set_permissions(&config, fs::Permissions::from_mode(0o600)).unwrap();
        let log = directory.path().join("backend.jsonl");
        let mut child = Command::new(env!("CARGO_BIN_EXE_codex-autoresume-bridge"))
            .arg("app-server")
            .arg("--listen")
            .arg("stdio://")
            .env("AUTORESUME_BRIDGE_CONFIG", &config)
            .env("AUTORESUME_TEST_BACKEND_LOG", &log)
            .env(
                "AUTORESUME_TEST_HOLD_QUOTA",
                if hold_quota { "1" } else { "0" },
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, output) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let value = serde_json::from_str(&line).expect("wrapper stdout must be JSON only");
                if tx.send(value).is_err() {
                    break;
                }
            }
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        let registration = loop {
            let registration = fs::read_dir(data.join("desktop-bridge"))
                .ok()
                .and_then(|entries| {
                    entries
                        .filter_map(Result::ok)
                        .find(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
                })
                .and_then(|entry| fs::read(entry.path()).ok())
                .and_then(|bytes| serde_json::from_slice(&bytes).ok());
            if let Some(value) = registration {
                break value;
            }
            assert!(
                Instant::now() < deadline,
                "wrapper did not register its local control endpoint"
            );
            assert!(
                child.try_wait().unwrap().is_none(),
                "wrapper exited before registration"
            );
            thread::sleep(Duration::from_millis(20));
        };
        Self {
            _directory: directory,
            data,
            log,
            child,
            input: Some(input),
            output,
            registration,
        }
    }

    fn send(&mut self, value: Value) {
        let input = self.input.as_mut().unwrap();
        serde_json::to_writer(&mut *input, &value).unwrap();
        input.write_all(b"\n").unwrap();
        input.flush().unwrap();
    }

    fn receive_until(&self, predicate: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let value = self
                .output
                .recv_timeout(remaining)
                .expect("expected native Desktop output");
            // Internal capability/account/quota/continuation responses must not
            // escape into the original client's JSON-RPC response namespace.
            assert!(
                !value
                    .get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| id.starts_with("autoresume:") || id.starts_with("desktop:")),
                "internal response leaked: {value}"
            );
            if predicate(&value) {
                return value;
            }
        }
    }

    fn control(&self, command: Value) -> Value {
        let port = self.registration["port"].as_u64().unwrap() as u16;
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        serde_json::to_writer(&mut stream, &json!({
            "secret":self.registration["secret"],"generation":self.registration["generation"],"command":command
        })).unwrap();
        stream.write_all(b"\n").unwrap();
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }

    fn connect_existing_and_fail(&mut self) {
        self.send(json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"original_desktop","version":"fixture"},"capabilities":{"experimentalApi":true}}}));
        self.receive_until(|value| value["id"] == 1);
        self.send(json!({"method":"initialized","params":{}}));
        self.send(json!({"id":"resume-id","method":"thread/resume","params":{"threadId":"original-desktop-chat"}}));
        let loaded = self.receive_until(|value| value["id"] == "resume-id");
        assert_eq!(loaded["result"]["thread"]["id"], "original-desktop-chat");
        self.send(json!({"id":3,"method":"turn/start","params":{"threadId":"original-desktop-chat","input":[{"type":"text","text":"SYNTHETIC TASK"}]}}));
        let tool = self.receive_until(|value| value["method"] == "item/tool/call");
        assert_eq!(
            tool,
            json!({"id":"native-tool-1","method":"item/tool/call","params":{
                "threadId":"original-desktop-chat","turnId":"quota-failed-turn","callId":"call-1",
                "tool":"original_desktop_tool","arguments":{"sample":"unchanged"}
            }})
        );
        self.send(json!({"id":"native-tool-1","result":{"success":true,"contentItems":[{"type":"inputText","text":"native tool answer"}]}}));
        self.receive_until(|value| {
            value["method"] == "turn/completed" && value["params"]["turn"]["status"] == "failed"
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let snapshot = self.control(json!({"command":"snapshot"}));
            if snapshot["tasks"][0]["canEnable"] == true {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "conditional capability was not confirmed: {snapshot}"
            );
            thread::sleep(Duration::from_millis(50));
        }
        let result = self.control(
            json!({"command":"set_enabled","thread_id":"original-desktop-chat","enabled":true}),
        );
        assert!(result["error"].is_null(), "{result}");
        assert_eq!(result["tasks"][0]["enabled"], true);
    }

    fn requests(&self) -> Vec<Value> {
        fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn finish(&mut self) {
        self.input.take();
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(
                    status.success(),
                    "wrapper or fake backend exited unsuccessfully: {status}"
                );
                break;
            }
            assert!(
                Instant::now() < deadline,
                "wrapper did not exit after Desktop EOF"
            );
            thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            fs::read_dir(self.data.join("desktop-bridge"))
                .unwrap()
                .count(),
            0,
            "stale endpoint registration survived shutdown"
        );
        for value in self.output.try_iter() {
            assert!(!value
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| id.starts_with("autoresume:")));
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.input.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn original_chat_continues_once_and_native_tools_and_response_ids_survive() {
    let mut fixture = Fixture::start(false);
    fixture.connect_existing_and_fail();
    fixture.receive_until(|value| {
        value["method"] == "turn/completed"
            && value["params"]["turn"]["id"] == "automatic-continuation"
    });
    let requests = fixture.requests();
    assert_eq!(
        requests
            .iter()
            .filter(|value| value["method"] == "turn/continueIfFailed")
            .count(),
        1
    );
    assert_eq!(
        requests
            .iter()
            .filter(|value| value["method"] == "turn/start")
            .count(),
        1,
        "guard must not emit ordinary turn/start"
    );
    assert_eq!(
        requests
            .iter()
            .find(|value| value["method"] == "initialize")
            .unwrap()["params"]["clientInfo"]["name"],
        "original_desktop"
    );
    assert_eq!(
        requests
            .iter()
            .find(|value| value["id"] == "native-tool-1")
            .unwrap()["result"],
        json!({"success":true,"contentItems":[{"type":"inputText","text":"native tool answer"}]})
    );
    fixture.finish();
}

#[test]
fn new_native_input_cancels_a_held_quota_preflight_before_any_continuation() {
    let mut fixture = Fixture::start(true);
    fixture.connect_existing_and_fail();
    let deadline = Instant::now() + Duration::from_secs(8);
    while !fixture
        .requests()
        .iter()
        .any(|value| value["method"] == "account/rateLimits/read")
    {
        assert!(Instant::now() < deadline, "quota preflight was not sent");
        thread::sleep(Duration::from_millis(20));
    }
    fixture.send(
        json!({"id":"new-user-input","method":"turn/start","params":{
            "threadId":"original-desktop-chat","input":[{"type":"text","text":"NEW USER WORK"}]
        }}),
    );
    fixture.receive_until(|value| {
        value["method"] == "turn/completed" && value["params"]["turn"]["id"] == "new-user-turn"
    });
    // Allow two periodic dispatch opportunities after the stale quota reply.
    thread::sleep(Duration::from_millis(2200));
    let snapshot = fixture.control(json!({"command":"snapshot"}));
    assert_eq!(snapshot["tasks"][0]["enabled"], false);
    assert!(!fixture
        .requests()
        .iter()
        .any(|value| value["method"] == "turn/continueIfFailed"));
    fixture.finish();
}
