//! Open the official CLI without putting the local channel capability in argv.
//! Account authentication remains entirely inside official Codex.
use crate::managed::LaunchSpec;
use std::path::Path;

pub async fn open(spec: &LaunchSpec, data_dir: &Path) -> Result<(), String> {
    validate(spec)?;
    #[cfg(target_os = "macos")]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let parent = data_dir.join("terminal-launchers");
        std::fs::create_dir_all(&parent).map_err(|_| "无法准备终端启动器")?;
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| "无法保护终端启动器")?;
        // A later launch must not share files with an earlier cleanup timer.
        let directory = parent.join(format!("{}-{}", spec.id, uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).map_err(|_| "终端已在打开，请勿重复操作")?;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| "无法保护终端启动器")?;
        let token_file = directory.join("channel");
        let script_file = directory.join("Codex.command");
        let result = async {
            // This is a freshly generated loopback IPC nonce, not an account token.
            let mut file = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(&token_file)
                .map_err(|_| "无法创建本地连接凭据")?;
            file.write_all(spec.token.as_bytes())
                .map_err(|_| "无法保存本地连接凭据")?;
            let script = mac_script(spec, &token_file, &script_file)?;
            let mut file = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o700)
                .open(&script_file)
                .map_err(|_| "无法创建终端启动器")?;
            file.write_all(script.as_bytes())
                .map_err(|_| "无法保存终端启动器")?;
            let status = tokio::process::Command::new("/usr/bin/open")
                .args(["-a", "Terminal"])
                .arg(&script_file)
                .status()
                .await
                .map_err(|_| "无法打开系统终端")?;
            if !status.success() {
                return Err("无法打开系统终端".to_owned());
            }
            Ok(())
        }
        .await;
        if result.is_err() {
            let _ = std::fs::remove_file(&token_file);
            let _ = std::fs::remove_file(&script_file);
            let _ = std::fs::remove_dir(&directory);
        } else {
            // A cancelled Terminal open must not leave a usable capability on disk.
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(120)).await;
                let _ = std::fs::remove_file(token_file);
                let _ = std::fs::remove_file(script_file);
                let _ = std::fs::remove_dir(directory);
            });
        }
        result
    }
    #[cfg(target_os = "windows")]
    {
        let _ = data_dir;
        ensure_login(spec).await?;
        remote_command(spec)?
            .spawn()
            .map_err(|_| "无法打开官方 Codex 终端")?;
        Ok(())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (spec, data_dir);
        Err("当前仅支持 macOS 和 Windows 终端".into())
    }
}

fn validate(spec: &LaunchSpec) -> Result<(), String> {
    if spec.token_env != crate::managed::TOKEN_ENV
        || spec.id.is_empty()
        || !spec
            .id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        || spec.token.is_empty()
        || spec.token.contains(['\0', '\n', '\r'])
    {
        return Err("连接配置不一致".into());
    }
    let endpoint = url::Url::parse(&spec.endpoint).map_err(|_| "本地连接地址无效")?;
    if endpoint.scheme() != "ws"
        || endpoint.host_str() != Some("127.0.0.1")
        || !endpoint.port().is_some_and(|port| port > 0)
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
        || endpoint.path() != "/"
    {
        return Err("本地连接地址无效".into());
    }
    Ok(())
}

#[cfg(any(target_os = "windows", test))]
fn login_command(spec: &LaunchSpec) -> Result<tokio::process::Command, String> {
    let mut command = crate::protocol::command(&spec.executable)?;
    command
        .current_dir(&spec.cwd)
        .env("CODEX_HOME", &spec.codex_home)
        .env_remove(&spec.token_env);
    Ok(command)
}

#[cfg(any(target_os = "windows", test))]
async fn login_status(spec: &LaunchSpec) -> Result<bool, String> {
    use std::{process::Stdio, time::Duration};
    let mut command = login_command(spec)?;
    command
        .args(["login", "status"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Only consume the exit status. Official status output can identify the
    // account or credential kind and is never needed by this launcher.
    let output = tokio::time::timeout(Duration::from_secs(20), command.output())
        .await
        .map_err(|_| "检查 Codex 登录状态超时")?
        .map_err(|_| "无法检查 Codex 登录状态")?;
    Ok(output.status.success())
}

#[cfg(any(target_os = "windows", test))]
async fn ensure_login(spec: &LaunchSpec) -> Result<(), String> {
    if login_status(spec).await? {
        return Ok(());
    }
    let mut command = login_command(spec)?;
    command.arg("login");
    #[cfg(target_os = "windows")]
    command.creation_flags(0x00000010); // CREATE_NEW_CONSOLE
    let status = command
        .status()
        .await
        .map_err(|_| "无法打开官方 Codex 登录")?;
    if !status.success() {
        return Err("登录未完成，未启动任务".into());
    }
    if !login_status(spec).await? {
        return Err("登录状态尚未就绪，未启动任务".into());
    }
    Ok(())
}

#[cfg(any(target_os = "windows", test))]
fn remote_command(spec: &LaunchSpec) -> Result<tokio::process::Command, String> {
    validate(spec)?;
    let mut command = crate::protocol::command(&spec.executable)?;
    command
        .kill_on_drop(false)
        .current_dir(&spec.cwd)
        .env("CODEX_HOME", &spec.codex_home)
        .env(&spec.token_env, &spec.token);
    #[cfg(target_os = "windows")]
    command.creation_flags(0x00000010); // CREATE_NEW_CONSOLE
    if let Some(thread_id) = &spec.thread_id {
        command.args(["resume", thread_id]);
    }
    command.args([
        "--remote",
        &spec.endpoint,
        "--remote-auth-token-env",
        &spec.token_env,
    ]);
    Ok(command)
}

#[cfg(any(target_os = "macos", test))]
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(any(target_os = "macos", test))]
fn mac_script(spec: &LaunchSpec, token_file: &Path, script_file: &Path) -> Result<String, String> {
    validate(spec)?;
    let path = |value: &Path| {
        value
            .to_str()
            .map(quote)
            .ok_or("路径编码不受支持".to_owned())
    };
    let executable = path(&spec.executable)?;
    let parent = path(spec.executable.parent().ok_or("Codex 路径无效")?)?;
    let resume = spec
        .thread_id
        .as_ref()
        .map(|id| format!("resume {} ", quote(id)))
        .unwrap_or_default();
    Ok(format!(
        r#"#!/bin/sh
set -eu
unset channel {env}
channel=$(cat {token})
rm -f -- {token} {script}
rmdir -- {directory} 2>/dev/null || true
export CODEX_HOME={home}
export PATH={parent}:"${{PATH:-/usr/bin:/bin}}"
cd -- {cwd}
if ! {executable} login status >/dev/null 2>&1; then
  if ! {executable} login; then
    printf '%s\n' '登录未完成，未启动任务' >&2
    exit 1
  fi
  if ! {executable} login status >/dev/null 2>&1; then
    printf '%s\n' '登录状态尚未就绪，未启动任务' >&2
    exit 1
  fi
fi
export {env}="$channel"
unset channel
exec {executable} {resume}--remote {endpoint} --remote-auth-token-env {env}
"#,
        token = path(token_file)?,
        script = path(script_file)?,
        env = spec.token_env,
        home = path(&spec.codex_home)?,
        cwd = path(&spec.cwd)?,
        endpoint = quote(&spec.endpoint),
        directory = path(script_file.parent().ok_or("终端启动目录无效")?)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example() -> LaunchSpec {
        LaunchSpec {
            id: "test".into(),
            executable: "/tmp/space ' $() `cmd`/codex".into(),
            cwd: "/tmp/project ' $(touch SHOULD_NOT_EXIST)\nline".into(),
            endpoint: "ws://127.0.0.1:12345".into(),
            token_env: crate::managed::TOKEN_ENV.into(),
            token: "private-channel-nonce".into(),
            codex_home: "/tmp/private codex".into(),
            thread_id: None,
        }
    }

    #[test]
    fn shell_arguments_remain_literal_and_capability_is_not_in_script() {
        let directory = tempfile::tempdir().unwrap();
        let spec = example();
        let script = mac_script(
            &spec,
            &directory.path().join("token"),
            &directory.path().join("launch"),
        )
        .unwrap();
        assert!(!script.contains(&spec.token));
        #[cfg(unix)]
        {
            let output = std::process::Command::new("/bin/sh")
                .args([
                    "-c",
                    &format!("printf '%s' {}", quote(spec.cwd.to_str().unwrap())),
                ])
                .output()
                .unwrap();
            assert!(output.status.success());
            assert_eq!(output.stdout, spec.cwd.to_str().unwrap().as_bytes());
        }
    }

    #[test]
    fn unsafe_environment_keys_nonlocal_endpoints_and_directory_ids_are_rejected() {
        let mut spec = example();
        spec.token_env = "TOKEN; unwanted-command".into();
        assert!(validate(&spec).is_err());
        spec = example();
        spec.endpoint = "ws://example.com:12345".into();
        assert!(validate(&spec).is_err());
        spec = example();
        spec.id = "../outside".into();
        assert!(validate(&spec).is_err());
    }

    #[test]
    fn direct_commands_keep_the_capability_only_in_the_remote_environment() {
        let mut spec = example();
        spec.thread_id = Some("literal ' $(unwanted) thread".into());
        let command = remote_command(&spec).unwrap();
        let command = command.as_std();
        assert_eq!(command.get_program(), spec.executable.as_os_str());
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(args[0], "resume");
        assert_eq!(args[1], spec.thread_id.as_deref().unwrap());
        assert!(!args.iter().any(|value| *value == spec.token.as_str()));
        assert!(command.get_envs().any(|(key, value)| {
            key == spec.token_env.as_str() && value == Some(spec.token.as_ref())
        }));
        assert!(login_command(&spec)
            .unwrap()
            .as_std()
            .get_envs()
            .any(|(key, value)| key == spec.token_env.as_str() && value.is_none()));
    }

    #[cfg(unix)]
    struct Fixture {
        _directory: tempfile::TempDir,
        spec: LaunchSpec,
        token_file: std::path::PathBuf,
        script_file: std::path::PathBuf,
    }

    #[cfg(unix)]
    impl Fixture {
        fn new(mode: &str) -> Self {
            use std::os::unix::fs::PermissionsExt;

            let directory = tempfile::tempdir().unwrap();
            let mut spec = example();
            let bin = directory.path().join("bin ' $(touch SHOULD_NOT_EXIST)");
            spec.executable = bin.join("codex");
            spec.cwd = directory
                .path()
                .join("project ' $(touch SHOULD_NOT_EXIST)\nline");
            spec.codex_home = directory.path().join("managed home '\nline");
            for path in [&bin, &spec.cwd, &spec.codex_home] {
                std::fs::create_dir(path).unwrap();
            }
            std::fs::write(spec.codex_home.join("mode"), mode).unwrap();
            let fake_cli = format!(
                r#"#!/bin/sh
set -eu
case "${{1-}}:${{2-}}" in
login:status)
  printf '%s\n' status >> "$CODEX_HOME/events"
  test -z "${{{env}-}}" || exit 91
  test -z "${{channel-}}" || exit 93
  test -f "$CODEX_HOME/ready"
  ;;
login:)
  printf '%s\n' login >> "$CODEX_HOME/events"
  test -z "${{{env}-}}" || exit 91
  test -z "${{channel-}}" || exit 93
  case "$(cat "$CODEX_HOME/mode")" in
    fail) exit 2 ;;
    no-state) exit 0 ;;
    *) touch "$CODEX_HOME/ready" ;;
  esac
  ;;
*)
  printf '%s\n' remote >> "$CODEX_HOME/events"
  test "${{{env}-}}" = 'private-channel-nonce' || exit 92
  printf '%s\000' "$@" > "$CODEX_HOME/argv"
  printf '%s' "$PWD" > "$CODEX_HOME/cwd"
  ;;
esac
"#,
                env = crate::managed::TOKEN_ENV,
            );
            std::fs::write(&spec.executable, fake_cli).unwrap();
            std::fs::set_permissions(&spec.executable, std::fs::Permissions::from_mode(0o700))
                .unwrap();
            let launch = directory.path().join("launcher");
            std::fs::create_dir(&launch).unwrap();
            Self {
                token_file: launch.join("channel"),
                script_file: launch.join("Codex.command"),
                spec,
                _directory: directory,
            }
        }

        fn script(&self) -> std::process::Output {
            use std::os::unix::fs::PermissionsExt;

            std::fs::write(&self.token_file, &self.spec.token).unwrap();
            std::fs::set_permissions(&self.token_file, std::fs::Permissions::from_mode(0o600))
                .unwrap();
            let script = mac_script(&self.spec, &self.token_file, &self.script_file).unwrap();
            assert!(!script.contains(&self.spec.token));
            std::fs::write(&self.script_file, script).unwrap();
            std::process::Command::new("/bin/sh")
                .arg(&self.script_file)
                .env(
                    &self.spec.token_env,
                    "ambient-capability-must-not-reach-login",
                )
                .env("channel", "ambient-export-attribute-must-be-cleared")
                .output()
                .unwrap()
        }

        fn events(&self) -> String {
            std::fs::read_to_string(self.spec.codex_home.join("events")).unwrap()
        }
    }

    #[cfg(unix)]
    #[test]
    fn mac_launcher_logs_in_before_remote_and_consumes_its_nonce_file() {
        let mut fixture = Fixture::new("success");
        fixture.spec.thread_id = Some("literal ' $(touch SHOULD_NOT_EXIST)".into());
        let output = fixture.script();
        assert!(output.status.success());
        assert_eq!(fixture.events(), "status\nlogin\nstatus\nremote\n");
        assert!(!fixture.token_file.exists());
        assert!(!fixture.script_file.exists());
        let args = std::fs::read(fixture.spec.codex_home.join("argv")).unwrap();
        assert!(!args
            .windows(fixture.spec.token.len())
            .any(|bytes| bytes == fixture.spec.token.as_bytes()));
        assert!(args
            .windows(fixture.spec.thread_id.as_ref().unwrap().len())
            .any(|bytes| { bytes == fixture.spec.thread_id.as_ref().unwrap().as_bytes() }));
        assert!(!fixture.spec.cwd.join("SHOULD_NOT_EXIST").exists());
        assert_eq!(
            std::fs::read_to_string(fixture.spec.codex_home.join("cwd")).unwrap(),
            fixture.spec.cwd.to_str().unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn mac_launcher_never_continues_after_failed_or_incomplete_login() {
        for (mode, expected) in [
            ("fail", "status\nlogin\n"),
            ("no-state", "status\nlogin\nstatus\n"),
        ] {
            let fixture = Fixture::new(mode);
            assert!(!fixture.script().status.success());
            assert_eq!(fixture.events(), expected);
            assert!(!fixture.token_file.exists());
            assert!(!fixture.spec.codex_home.join("argv").exists());
        }
        let fixture = Fixture::new("fail");
        std::fs::write(fixture.spec.codex_home.join("ready"), "").unwrap();
        assert!(fixture.script().status.success());
        assert_eq!(fixture.events(), "status\nremote\n");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn direct_login_gate_stops_on_failure_and_rechecks_success() {
        for mode in ["fail", "no-state"] {
            let fixture = Fixture::new(mode);
            assert!(ensure_login(&fixture.spec).await.is_err());
            assert!(!fixture.events().contains("remote"));
        }
        let fixture = Fixture::new("success");
        ensure_login(&fixture.spec).await.unwrap();
        assert_eq!(fixture.events(), "status\nlogin\nstatus\n");
        assert!(remote_command(&fixture.spec)
            .unwrap()
            .output()
            .await
            .unwrap()
            .status
            .success());
        assert_eq!(fixture.events(), "status\nlogin\nstatus\nremote\n");
    }
}
