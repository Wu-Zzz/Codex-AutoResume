use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

pub const CONFIG_ENV: &str = "AUTORESUME_BRIDGE_CONFIG";

/// Paths prepared by AutoResume, never inferred from the CLI override or PATH.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BridgeConfig {
    pub official_cli: PathBuf,
    pub guarded_app_server: PathBuf,
    pub data_dir: PathBuf,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Invocation {
    Official,
    /// Standalone app-server arguments, with only the subcommand removed.
    Guarded(Vec<OsString>),
}

/// Recognize only the Desktop stdio invocation shape that we have verified.
/// Unknown flags/subcommands keep working through the unmodified official CLI.
pub fn classify(args: &[OsString]) -> Invocation {
    let mut index = 0;
    while index < args.len() {
        match args[index].to_str() {
            Some("-c" | "--config") if index + 1 < args.len() => index += 2,
            Some(value) if config_assignment(value) => index += 1,
            Some("app-server") => return classify_server(args, index),
            _ => return Invocation::Official,
        }
    }
    Invocation::Official
}

fn classify_for_installation(args: &[OsString], installed: bool) -> Invocation {
    let invocation = classify(args);
    if installed {
        if let Invocation::Guarded(server_args) = &invocation {
            if !desktop_main_signature(server_args) {
                return Invocation::Official;
            }
        }
    }
    invocation
}

/// The verified Desktop launcher supplies both code-mode hosting and its
/// analytics default. Its Node MCP helper also inherits CODEX_CLI_PATH but starts
/// a generic App Server without these flags; that helper must stay official.
/// Parse config values as values, so a quoted flag cannot imitate a launcher.
fn desktop_main_signature(args: &[OsString]) -> bool {
    let mut code_mode_host = false;
    let mut analytics_default = false;
    let mut index = 0;
    while index < args.len() {
        let value = args[index].to_str();
        let config = match value {
            Some("-c" | "--config") => {
                index += 1;
                args.get(index).and_then(|value| value.to_str())
            }
            Some("--analytics-default-enabled") => {
                analytics_default = true;
                None
            }
            Some(value) => value
                .strip_prefix("--config=")
                .or_else(|| value.strip_prefix("-c")),
            None => None,
        };
        if let Some((key, value)) = config.and_then(|config| config.split_once('=')) {
            if key.trim() == "features.code_mode_host" {
                // A later override must not leave the earlier true marker active.
                code_mode_host = value.trim() == "true";
            }
        }
        index += 1;
    }
    code_mode_host && analytics_default
}

fn config_assignment(value: &str) -> bool {
    value.starts_with("--config=") || (value.starts_with("-c") && value.len() > 2)
}

fn classify_server(args: &[OsString], command_index: usize) -> Invocation {
    let mut index = command_index + 1;
    let mut transport_seen = false;
    while index < args.len() {
        match args[index].to_str() {
            Some("-c" | "--config") if index + 1 < args.len() => index += 2,
            Some(value) if config_assignment(value) => index += 1,
            Some("--strict-config" | "--analytics-default-enabled") => index += 1,
            Some("--stdio") if !transport_seen => {
                transport_seen = true;
                index += 1;
            }
            Some("--listen") if !transport_seen && index + 1 < args.len() => {
                if args[index + 1] != "stdio://" {
                    return Invocation::Official;
                }
                transport_seen = true;
                index += 2;
            }
            Some("--listen=stdio://") if !transport_seen => {
                transport_seen = true;
                index += 1;
            }
            _ => return Invocation::Official,
        }
    }
    let mut guarded_args = args.to_vec();
    guarded_args.remove(command_index);
    Invocation::Guarded(guarded_args)
}

impl BridgeConfig {
    pub fn load(path: &Path, wrapper: &Path) -> Result<Self, String> {
        require_private_path(path, false)?;
        if fs::metadata(path)
            .map_err(|error| format!("无法检查桌面桥接配置：{error}"))?
            .len()
            > 16 * 1024
        {
            return Err("桌面桥接配置过大".into());
        }
        let bytes = fs::read(path).map_err(|error| format!("无法读取桌面桥接配置：{error}"))?;
        if bytes.len() > 16 * 1024 {
            return Err("桌面桥接配置过大".into());
        }
        let mut config: Self = serde_json::from_slice(&bytes)
            .map_err(|error| format!("桌面桥接配置格式无效：{error}"))?;
        let wrapper = wrapper
            .canonicalize()
            .map_err(|error| format!("无法确认桥接程序路径：{error}"))?;
        config.official_cli = canonical_executable(&config.official_cli)?;
        config.guarded_app_server = canonical_executable(&config.guarded_app_server)?;
        if same_executable(&config.official_cli, &wrapper)?
            || same_executable(&config.guarded_app_server, &wrapper)?
            || same_executable(&config.official_cli, &config.guarded_app_server)?
        {
            return Err("桌面桥接路径存在递归或运行组件混用，已拒绝启动".into());
        }
        require_private_path(&config.data_dir, true)?;
        config.data_dir = config
            .data_dir
            .canonicalize()
            .map_err(|error| format!("无法确认桌面桥接数据目录：{error}"))?;
        Ok(config)
    }

    pub(crate) fn check_installed_helper(&self) -> Result<(), String> {
        // Installation currently supports this verified macOS Desktop package
        // only. Never discover a replacement helper through PATH or another app.
        let package = self
            .official_cli
            .ancestors()
            .nth(4)
            .filter(|package| {
                package.join("CodexCLI.app/Contents/MacOS/codex") == self.official_cli
            })
            .ok_or("官方 Codex 运行组件布局尚未验证")?;
        let source = package.join("bin/codex-code-mode-host");
        let source_metadata = fs::symlink_metadata(&source)
            .map_err(|_| "官方代码执行组件缺失，已拒绝使用实验桌面组件")?;
        if !source_metadata.is_file() || source_metadata.file_type().is_symlink() {
            return Err("官方代码执行组件类型无效".into());
        }
        let expected = canonical_executable(&source)?;
        let linked = self
            .guarded_app_server
            .parent()
            .ok_or("实验桌面运行组件目录无效")?
            .join("codex-code-mode-host");
        if !fs::symlink_metadata(&linked)
            .map_err(|_| "实验桌面代码执行组件链接缺失")?
            .file_type()
            .is_symlink()
            || canonical_executable(&linked)? != expected
        {
            return Err("实验桌面代码执行组件未连接到已验证的官方组件".into());
        }
        Ok(())
    }
}

fn same_executable(left: &Path, right: &Path) -> Result<bool, String> {
    if left == right {
        return Ok(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let left = fs::metadata(left).map_err(|error| format!("无法检查运行组件：{error}"))?;
        let right = fs::metadata(right).map_err(|error| format!("无法检查运行组件：{error}"))?;
        Ok(left.dev() == right.dev() && left.ino() == right.ino())
    }
    #[cfg(not(unix))]
    {
        Ok(false)
    }
}

fn canonical_executable(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("桌面桥接运行组件必须使用绝对路径".into());
    }
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("无法找到桌面桥接运行组件：{error}"))?;
    let metadata =
        fs::metadata(&canonical).map_err(|error| format!("无法检查运行组件：{error}"))?;
    if !metadata.is_file() {
        return Err("桌面桥接运行组件不是普通文件".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err("桌面桥接运行组件不可执行".into());
        }
    }
    Ok(canonical)
}

fn require_private_path(path: &Path, directory: bool) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("桌面桥接配置与数据目录必须使用绝对路径".into());
    }
    let metadata =
        fs::symlink_metadata(path).map_err(|error| format!("无法检查桌面桥接私有文件：{error}"))?;
    if metadata.file_type().is_symlink()
        || (directory && !metadata.is_dir())
        || (!directory && !metadata.is_file())
    {
        return Err("桌面桥接配置或数据目录类型无效".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err("桌面桥接配置与数据目录必须仅当前用户可访问".into());
        }
    }
    Ok(())
}

/// Inherit the original working directory, environment and stdio handles.
/// Point nested CLI calls at the original binary instead of entering the bridge.
pub fn official_command(config: &BridgeConfig, args: &[OsString]) -> Command {
    let mut command = Command::new(&config.official_cli);
    command
        .args(args)
        .env("CODEX_CLI_PATH", &config.official_cli)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    command
}

pub fn run_wrapper() -> Result<i32, String> {
    let config_path = std::env::var_os(CONFIG_ENV)
        .filter(|value| !value.is_empty())
        .ok_or("缺少桌面桥接配置；请先在 AutoResume 准备桌面连接")?;
    let wrapper =
        std::env::current_exe().map_err(|error| format!("无法确认桥接程序路径：{error}"))?;
    let config = BridgeConfig::load(Path::new(&config_path), &wrapper)?;
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    // Presence of either installation marker selects the narrower route even
    // when the pair is malformed. The bridge separately validates their token;
    // a partial pair must never silently become the legacy manual mode.
    let installed = std::env::var_os("AUTORESUME_DESKTOP_ENABLE_FILE").is_some()
        || std::env::var_os("AUTORESUME_DESKTOP_ENABLE_TOKEN").is_some();
    // The persistent entry point checks configuration before selecting a backend.
    // This branch never starts an App Server or touches a user's chat.
    if args == [OsString::from("--autoresume-check-config")] {
        if installed {
            config.check_installed_helper()?;
        }
        return Ok(0);
    }
    match classify_for_installation(&args, installed) {
        Invocation::Official => pass_through(&config, &args),
        Invocation::Guarded(server_args) => {
            // Recheck after bootstrap preflight. Losing a helper must not break
            // normal Desktop use or register an unusable guarded connection.
            if installed && config.check_installed_helper().is_err() {
                eprintln!("Codex AutoResume：代码执行组件未通过检查，已使用官方 Codex。");
                return pass_through(&config, &args);
            }
            run_guarded(config, server_args)
        }
    }
}

#[cfg(target_os = "macos")]
fn run_guarded(config: BridgeConfig, args: Vec<OsString>) -> Result<i32, String> {
    crate::desktop_bridge::run(config, args)
}

#[cfg(not(target_os = "macos"))]
fn run_guarded(_config: BridgeConfig, _args: Vec<OsString>) -> Result<i32, String> {
    Err("此平台尚未验证桌面原运行环境与沙箱保持，已拒绝启用桌面自动续跑".into())
}

fn pass_through(config: &BridgeConfig, args: &[OsString]) -> Result<i32, String> {
    let mut command = official_command(config, args);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        Err(format!("无法启动原 Codex 运行组件：{}", command.exec()))
    }
    #[cfg(not(unix))]
    {
        command
            .status()
            .map(|status| status.code().unwrap_or(1))
            .map_err(|error| format!("无法启动原 Codex 运行组件：{error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn desktop_stdio_preserves_config_order_and_values() {
        let original = args(&[
            "-c",
            "features.code_mode_host=true",
            "app-server",
            "--analytics-default-enabled",
            "-c",
            "sandbox_mode=\"workspace-write\"",
        ]);
        assert_eq!(
            classify(&original),
            Invocation::Guarded(args(&[
                "-c",
                "features.code_mode_host=true",
                "--analytics-default-enabled",
                "-c",
                "sandbox_mode=\"workspace-write\"",
            ]))
        );
    }

    #[test]
    fn installed_route_accepts_both_verified_desktop_config_orders() {
        for values in [
            vec![
                "-c",
                "features.code_mode_host=true",
                "app-server",
                "--analytics-default-enabled",
                "-c",
                "sandbox_mode=\"workspace-write\"",
            ],
            vec![
                "app-server",
                "-c",
                "features.code_mode_host=true",
                "-c",
                "chatgpt_base_url=\"https://example.invalid\"",
                "--analytics-default-enabled",
            ],
        ] {
            let original = args(&values);
            assert!(matches!(
                classify_for_installation(&original, true),
                Invocation::Guarded(_)
            ));
            assert_eq!(
                classify_for_installation(&original, true),
                classify(&original)
            );
        }
    }

    #[test]
    fn installed_route_keeps_auxiliary_servers_official_and_manual_mode_unchanged() {
        for values in [
            vec!["app-server", "--listen", "stdio://"],
            vec!["app-server"],
            vec!["-c", "features.code_mode_host=true", "app-server"],
            vec!["app-server", "--analytics-default-enabled"],
            vec![
                "-c",
                "--analytics-default-enabled",
                "app-server",
                "-c",
                "features.code_mode_host=true",
            ],
            vec![
                "-c",
                "features.code_mode_host=true",
                "app-server",
                "--analytics-default-enabled",
                "-c",
                "features.code_mode_host=false",
            ],
        ] {
            let original = args(&values);
            assert_eq!(
                classify_for_installation(&original, true),
                Invocation::Official,
                "{values:?}"
            );
            assert_eq!(
                classify_for_installation(&original, false),
                classify(&original)
            );
            assert!(matches!(classify(&original), Invocation::Guarded(_)));
        }
    }

    #[test]
    fn configuration_values_cannot_become_subcommands() {
        assert_eq!(
            classify(&args(&["--config", "app-server", "exec", "hello"])),
            Invocation::Official
        );
        assert_eq!(
            classify(&args(&["--config", "app-server", "app-server"])),
            Invocation::Guarded(args(&["--config", "app-server"]))
        );
    }

    #[test]
    fn all_non_server_and_uncertain_invocations_pass_through() {
        for values in [
            vec!["--version"],
            vec!["exec", "app-server"],
            vec!["--", "app-server"],
            vec!["--unknown", "app-server"],
            vec!["app-server", "proxy"],
            vec!["app-server", "daemon", "version"],
            vec!["app-server", "generate-ts", "--out", "example"],
            vec!["app-server", "generate-json-schema"],
            vec!["app-server", "--help"],
            vec!["app-server", "--remote-control"],
            vec!["app-server", "--managed-daemon"],
            vec!["app-server", "--listen"],
            vec!["app-server", "--listen", "ws://127.0.0.1:4500"],
            vec!["app-server", "--listen=unix://"],
            vec!["app-server", "--listen", "off"],
            vec!["app-server", "--stdio", "--listen", "stdio://"],
            vec!["app-server", "-c"],
        ] {
            assert_eq!(classify(&args(&values)), Invocation::Official, "{values:?}");
        }
    }

    #[test]
    fn explicit_stdio_and_inline_configuration_are_preserved() {
        for values in [
            vec!["app-server", "--stdio"],
            vec!["app-server", "--listen", "stdio://"],
            vec!["app-server", "--listen=stdio://"],
            vec!["-cfoo=\"bar\"", "app-server", "--config=baz=true"],
        ] {
            let original = args(&values);
            let mut expected = original.clone();
            expected.remove(
                values
                    .iter()
                    .position(|value| *value == "app-server")
                    .unwrap(),
            );
            assert_eq!(classify(&original), Invocation::Guarded(expected));
        }
    }

    #[test]
    fn child_environment_only_overrides_nested_cli_path() {
        let config = BridgeConfig {
            official_cli: PathBuf::from("/original/codex"),
            guarded_app_server: PathBuf::from("/bridge/guarded"),
            data_dir: PathBuf::from("/private/data"),
        };
        let command = official_command(&config, &args(&["--version"]));
        assert_eq!(command.get_program(), "/original/codex");
        assert_eq!(command.get_current_dir(), None);
        let changes: Vec<_> = command.get_envs().collect();
        assert_eq!(
            changes,
            vec![(
                OsStr::new("CODEX_CLI_PATH"),
                Some(OsStr::new("/original/codex"))
            )]
        );
        assert_eq!(command.get_args().collect::<Vec<_>>(), vec!["--version"]);
    }

    struct Fixture {
        _directory: tempfile::TempDir,
        wrapper: PathBuf,
        config_file: PathBuf,
        config: BridgeConfig,
    }

    impl Fixture {
        fn new() -> Self {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path().canonicalize().unwrap();
            let wrapper = root.join("bridge");
            let official = root.join("official");
            let guarded = root.join("guarded");
            for file in [&wrapper, &official, &guarded] {
                fs::write(file, b"fake executable").unwrap();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(file, fs::Permissions::from_mode(0o700)).unwrap();
                }
            }
            let data_dir = root.join("data");
            fs::create_dir(&data_dir).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&data_dir, fs::Permissions::from_mode(0o700)).unwrap();
            }
            let config = BridgeConfig {
                official_cli: official,
                guarded_app_server: guarded,
                data_dir,
            };
            let fixture = Self {
                _directory: directory,
                wrapper,
                config_file: root.join("config.json"),
                config,
            };
            fixture.write_config();
            fixture
        }

        fn write_config(&self) {
            fs::write(&self.config_file, serde_json::to_vec(&self.config).unwrap()).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&self.config_file, fs::Permissions::from_mode(0o600)).unwrap();
            }
        }
    }

    #[test]
    fn private_configuration_loads_canonical_distinct_paths() {
        let fixture = Fixture::new();
        let config = BridgeConfig::load(&fixture.config_file, &fixture.wrapper).unwrap();
        assert_eq!(config.official_cli, fixture.config.official_cli);
        assert_eq!(config.data_dir, fixture.config.data_dir);
    }

    #[test]
    fn configuration_rejects_recursive_and_mixed_executables() {
        let mut fixture = Fixture::new();
        fixture.config.official_cli = fixture.wrapper.clone();
        fixture.write_config();
        assert!(BridgeConfig::load(&fixture.config_file, &fixture.wrapper).is_err());
        fixture.config.official_cli = fixture.config.guarded_app_server.clone();
        fixture.write_config();
        assert!(BridgeConfig::load(&fixture.config_file, &fixture.wrapper).is_err());
    }

    #[test]
    fn configuration_rejects_relative_paths_and_unknown_fields() {
        let mut fixture = Fixture::new();
        fixture.config.official_cli = PathBuf::from("codex");
        fixture.write_config();
        assert!(BridgeConfig::load(&fixture.config_file, &fixture.wrapper).is_err());
        let fixture = Fixture::new();
        let mut json = serde_json::to_value(&fixture.config).unwrap();
        json["permissionOverride"] = serde_json::json!("unsafe");
        fs::write(&fixture.config_file, serde_json::to_vec(&json).unwrap()).unwrap();
        assert!(BridgeConfig::load(&fixture.config_file, &fixture.wrapper).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_to_self_cannot_bypass_recursion_protection() {
        use std::os::unix::fs::symlink;
        let mut fixture = Fixture::new();
        let link = fixture.wrapper.with_file_name("alias");
        symlink(&fixture.wrapper, &link).unwrap();
        fixture.config.official_cli = link;
        fixture.write_config();
        assert!(BridgeConfig::load(&fixture.config_file, &fixture.wrapper).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn hard_link_to_self_cannot_bypass_recursion_protection() {
        let mut fixture = Fixture::new();
        let link = fixture.wrapper.with_file_name("hard-alias");
        fs::hard_link(&fixture.wrapper, &link).unwrap();
        fixture.config.official_cli = link;
        fixture.write_config();
        assert!(BridgeConfig::load(&fixture.config_file, &fixture.wrapper).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn original_command_keeps_input_arguments_environment_and_exit_status() {
        use std::io::Write;
        let fixture = Fixture::new();
        fs::write(
            &fixture.config.official_cli,
            b"#!/bin/sh\nIFS= read -r input\n[ \"$input\" = 'stream data' ] || exit 4\n[ \"$1\" = 'two words' ] || exit 5\n[ \"$HOME\" = \"$2\" ] || exit 6\n[ \"$CODEX_CLI_PATH\" = \"$0\" ] || exit 7\nprintf 'preserved'\nprintf 'diagnostic' >&2\nexit 23\n",
        )
        .unwrap();
        let original_home = std::env::var_os("HOME").unwrap_or_default();
        let mut command = official_command(
            &fixture.config,
            &[OsString::from("two words"), original_home],
        );
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"stream data\n")
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(23));
        assert_eq!(output.stdout, b"preserved");
        assert_eq!(output.stderr, b"diagnostic");
    }

    #[cfg(unix)]
    #[test]
    fn public_configuration_or_data_directory_is_rejected() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = Fixture::new();
        fs::set_permissions(&fixture.config_file, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(BridgeConfig::load(&fixture.config_file, &fixture.wrapper).is_err());
        fixture.write_config();
        fs::set_permissions(&fixture.config.data_dir, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(BridgeConfig::load(&fixture.config_file, &fixture.wrapper).is_err());
    }
}
