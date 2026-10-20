//! Open the official CLI without putting the local channel capability in argv.
//! Account authentication remains entirely inside official Codex.
use crate::managed::LaunchSpec;
use std::path::Path;

pub async fn open(spec: &LaunchSpec, data_dir: &Path) -> Result<(), String> {
    validate(spec)?;
    #[cfg(target_os = "macos")]
    {
        open_macos(
            spec,
            data_dir,
            tokio::process::Command::new("/usr/bin/open"),
            std::time::Duration::from_secs(15),
        )
        .await
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

#[cfg(any(target_os = "macos", test))]
const STARTED_MARKER: &str = "AUTORESUME_TERMINAL_STARTED\n";

#[cfg(any(target_os = "macos", all(test, unix)))]
struct MacLauncher {
    directory: std::path::PathBuf,
}

#[cfg(any(target_os = "macos", all(test, unix)))]
impl MacLauncher {
    fn prepare(spec: &LaunchSpec, data_dir: &Path) -> Result<Self, String> {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        validate(spec)?;
        let parent = data_dir.join("terminal-launchers");
        std::fs::create_dir_all(&parent).map_err(|_| "无法准备终端启动器")?;
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| "无法保护终端启动器")?;
        let directory = parent.join(format!("{}-{}", spec.id, uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).map_err(|_| "无法准备终端启动目录")?;
        let launcher = Self { directory };
        std::fs::set_permissions(&launcher.directory, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| "无法保护终端启动器")?;
        let token_file = launcher.directory.join("channel");
        let script_file = launcher.directory.join("Codex.command");
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
        Ok(launcher)
    }

    fn acknowledged(&self) -> Result<bool, String> {
        use std::io::{ErrorKind, Read};

        match std::fs::File::open(self.directory.join("started")) {
            Ok(file) => {
                let mut bytes = Vec::new();
                file.take((STARTED_MARKER.len() + 1) as u64)
                    .read_to_end(&mut bytes)
                    .map_err(|_| "无法读取终端启动确认")?;
                if bytes == STARTED_MARKER.as_bytes() {
                    Ok(true)
                } else {
                    Err("终端启动确认无效，未建立托管连接".into())
                }
            }
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
            Err(_) => Err("无法读取终端启动确认".into()),
        }
    }
}

#[cfg(any(target_os = "macos", all(test, unix)))]
impl Drop for MacLauncher {
    fn drop(&mut self) {
        // Per-attempt files cannot interfere with a later launch. Cleanup also
        // runs when a launch future is cancelled or its opener times out.
        for name in ["channel", "Codex.command", "started", "started.tmp"] {
            let _ = std::fs::remove_file(self.directory.join(name));
        }
        let _ = std::fs::remove_dir(&self.directory);
    }
}

#[cfg(any(target_os = "macos", all(test, unix)))]
async fn open_macos(
    spec: &LaunchSpec,
    data_dir: &Path,
    mut opener: tokio::process::Command,
    launch_timeout: std::time::Duration,
) -> Result<(), String> {
    let launcher = MacLauncher::prepare(spec, data_dir)?;
    opener
        .args(["-a", "Terminal"])
        .arg(launcher.directory.join("Codex.command"))
        .kill_on_drop(true);
    tokio::time::timeout(launch_timeout, async {
        let status = opener.status().await.map_err(|_| "无法打开系统终端")?;
        if !status.success() {
            return Err("无法打开系统终端".into());
        }
        loop {
            if launcher.acknowledged()? {
                // Only confirm script startup here. Official login can take as
                // long as the user needs and is never limited by this deadline.
                return Ok(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await
    .map_err(|_| "终端未确认启动，请打开系统终端后重试".to_owned())?
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
umask 077
unset channel {env}
channel=$(cat {token})
test -n "$channel"
rm -f -- {token} {script}
export CODEX_HOME={home}
export PATH={parent}:"${{PATH:-/usr/bin:/bin}}"
cd -- {cwd}
printf '%s' {started_marker} > {started_tmp}
mv -- {started_tmp} {started}
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
        started_marker = quote(STARTED_MARKER),
        started = path(&script_file.with_file_name("started"))?,
        started_tmp = path(&script_file.with_file_name("started.tmp"))?,
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
    slow)
      remaining=200
      while test ! -f "$CODEX_HOME/release-login"; do
        test "$remaining" -gt 0 || exit 94
        remaining=$((remaining - 1))
        sleep 0.025
      done
      touch "$CODEX_HOME/ready"
      ;;
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

        fn opener(&self, body: &str) -> tokio::process::Command {
            use std::os::unix::fs::PermissionsExt;

            let executable = self._directory.path().join("fake-open");
            std::fs::write(&executable, format!("#!/bin/sh\nset -eu\n{body}\n")).unwrap();
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
            let mut command = tokio::process::Command::new(executable);
            command.env("TEST_DIRECTORY", self._directory.path());
            command
        }

        fn app_data(&self) -> std::path::PathBuf {
            self._directory.path().join("app-data")
        }

        fn assert_launch_files_removed(&self) {
            assert_eq!(
                std::fs::read_dir(self.app_data().join("terminal-launchers"))
                    .unwrap()
                    .count(),
                0
            );
        }

        async fn wait_for_event(&self, event: &str) {
            tokio::time::timeout(std::time::Duration::from_secs(3), async {
                loop {
                    if std::fs::read_to_string(self.spec.codex_home.join("events"))
                        .unwrap_or_default()
                        .lines()
                        .any(|line| line == event)
                    {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn mac_open_requires_exact_script_acknowledgement_and_cleans_failed_attempts() {
        for opener in [
            "exit 0",
            "exit 7",
            "printf '%s' incorrect > \"$(dirname \"$3\")/started\"\nexit 0",
        ] {
            let fixture = Fixture::new("success");
            let result = open_macos(
                &fixture.spec,
                &fixture.app_data(),
                fixture.opener(opener),
                std::time::Duration::from_secs(1),
            )
            .await;
            assert!(result.is_err());
            fixture.assert_launch_files_removed();
            assert!(!fixture.spec.codex_home.join("events").exists());
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn mac_open_times_out_and_kills_an_unresponsive_opener() {
        let fixture = Fixture::new("success");
        let opener = fixture
            .opener("printf '%s' \"$$\" > \"$TEST_DIRECTORY/opener-pid\"\nexec /bin/sleep 30");
        assert!(open_macos(
            &fixture.spec,
            &fixture.app_data(),
            opener,
            std::time::Duration::from_secs(2),
        )
        .await
        .is_err());
        fixture.assert_launch_files_removed();
        let pid = std::fs::read_to_string(fixture._directory.path().join("opener-pid")).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let status = tokio::process::Command::new("/bin/kill")
                    .args(["-0", pid.trim()])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .await
                    .unwrap();
                if !status.success() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(!fixture.spec.codex_home.join("events").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn mac_open_confirms_startup_without_timing_out_slow_official_login() {
        let fixture = Fixture::new("slow");
        let opener =
            fixture.opener("/bin/sh \"$3\" > \"$TEST_DIRECTORY/launcher-output\" 2>&1 &\nexit 0");
        open_macos(
            &fixture.spec,
            &fixture.app_data(),
            opener,
            std::time::Duration::from_secs(2),
        )
        .await
        .unwrap();
        fixture.assert_launch_files_removed();
        fixture.wait_for_event("login").await;
        assert_eq!(fixture.events(), "status\nlogin\n");
        // Startup is complete, although the fake user has not finished login.
        std::fs::write(fixture.spec.codex_home.join("release-login"), "").unwrap();
        fixture.wait_for_event("remote").await;
        assert_eq!(fixture.events(), "status\nlogin\nstatus\nremote\n");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn mac_late_invocation_cannot_use_a_timed_out_launch() {
        let fixture = Fixture::new("success");
        let opener = fixture.opener("cp \"$3\" \"$TEST_DIRECTORY/late.command\"\nexit 0");
        assert!(open_macos(
            &fixture.spec,
            &fixture.app_data(),
            opener,
            std::time::Duration::from_secs(1),
        )
        .await
        .is_err());
        fixture.assert_launch_files_removed();
        let output = tokio::process::Command::new("/bin/sh")
            .arg(fixture._directory.path().join("late.command"))
            .output()
            .await
            .unwrap();
        assert!(!output.status.success());
        assert!(!fixture.spec.codex_home.join("events").exists());
    }

    #[cfg(unix)]
    #[test]
    fn mac_launch_files_are_private_and_marker_contains_no_capability() {
        use std::os::unix::fs::PermissionsExt;

        let fixture = Fixture::new("fail");
        let launcher = MacLauncher::prepare(&fixture.spec, &fixture.app_data()).unwrap();
        for (name, mode) in [("", 0o700), ("channel", 0o600), ("Codex.command", 0o700)] {
            assert_eq!(
                std::fs::metadata(launcher.directory.join(name))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                mode
            );
        }
        let output = std::process::Command::new("/bin/sh")
            .arg(launcher.directory.join("Codex.command"))
            .output()
            .unwrap();
        assert!(!output.status.success()); // The fixture intentionally refuses login.
        assert!(launcher.acknowledged().unwrap());
        let marker = launcher.directory.join("started");
        assert_eq!(std::fs::read(&marker).unwrap(), STARTED_MARKER.as_bytes());
        assert_eq!(
            std::fs::metadata(marker).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(!STARTED_MARKER.contains(&fixture.spec.token));
        drop(launcher);
        fixture.assert_launch_files_removed();
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
