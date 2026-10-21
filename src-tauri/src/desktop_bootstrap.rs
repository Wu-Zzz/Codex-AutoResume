use std::path::Path;

fn quoted(value: &str) -> Result<String, String> {
    if value.contains('\0') {
        return Err("桌面连接配置不能包含空字符".into());
    }
    Ok(format!("'{}'", value.replace('\'', "'\\''")))
}

fn quoted_path(path: &Path) -> Result<String, String> {
    if !path.is_absolute() {
        return Err("桌面连接配置必须使用绝对路径".into());
    }
    quoted(path.to_str().ok_or("桌面连接路径必须是有效的 UTF-8 文本")?)
}

/// A silent hook for the shell that Desktop uses to read its environment.
/// Existing overrides, including an explicitly empty override, remain untouched.
pub fn environment_hook(bootstrap: &Path, enabled: &Path) -> Result<String, String> {
    let bootstrap = quoted_path(bootstrap)?;
    let enabled = quoted_path(enabled)?;
    Ok(format!(
        r#"if [ "${{CODEX_SHELL-}}" = '1' ] && \
    [ -f {enabled} ] && [ -f {bootstrap} ] && [ -x {bootstrap} ] && \
    {{ [ "${{CODEX_CLI_PATH+x}}" != 'x' ] || [ "${{CODEX_CLI_PATH-}}" = {bootstrap} ]; }}; then
    export CODEX_CLI_PATH={bootstrap}
fi
"#
    ))
}

/// Fail over only before any guarded backend is started. Once exec succeeds,
/// bridge failures belong to that live generation and must never be replayed.
pub fn bootstrap(
    official_cli: &Path,
    bridge: &Path,
    config: &Path,
    enabled: &Path,
    plist: &Path,
    expected_build: &str,
) -> Result<String, String> {
    let token = quoted(
        config
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .ok_or("缺少桌面运行组件代际")?,
    )?;
    let official = quoted_path(official_cli)?;
    let bridge = quoted_path(bridge)?;
    let config = quoted_path(config)?;
    let enabled = quoted_path(enabled)?;
    let plist = quoted_path(plist)?;
    if expected_build.is_empty() {
        return Err("必须指定已验证的 Codex Desktop 构建版本".into());
    }
    let build = quoted(expected_build)?;
    Ok(format!(
        r#"#!/bin/sh
autoresume_official() {{
    unset AUTORESUME_BRIDGE_CONFIG
    unset AUTORESUME_DESKTOP_ENABLE_FILE AUTORESUME_DESKTOP_ENABLE_TOKEN
    exec /usr/bin/env CODEX_CLI_PATH={official} {official} "$@"
}}
autoresume_matching_build() (
    detected_build=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' {plist} 2>/dev/null) || exit 1
    [ "$detected_build" = {build} ]
)
if [ ! -f {enabled} ]; then
    autoresume_official "$@"
fi
if [ ! -f {bridge} ] || [ ! -x {bridge} ] || [ ! -f {config} ] || [ ! -r {config} ]; then
    printf '%s\n' 'Codex AutoResume：桌面连接组件不可用，已使用官方 Codex。' >&2
    autoresume_official "$@"
fi
if ! autoresume_matching_build; then
    printf '%s\n' 'Codex AutoResume：当前 Codex 版本尚未验证，已使用官方 Codex。' >&2
    autoresume_official "$@"
fi
if ! /usr/bin/env AUTORESUME_BRIDGE_CONFIG={config} {bridge} --autoresume-check-config >/dev/null 2>&1; then
    printf '%s\n' 'Codex AutoResume：桌面连接配置未通过检查，已使用官方 Codex。' >&2
    autoresume_official "$@"
fi
exec /usr/bin/env AUTORESUME_BRIDGE_CONFIG={config} AUTORESUME_DESKTOP_ENABLE_FILE={enabled} AUTORESUME_DESKTOP_ENABLE_TOKEN={token} {bridge} "$@"
"#
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_absolute_utf8_paths_and_build() {
        let absolute = Path::new("/tmp/desktop");
        assert!(environment_hook(Path::new("relative"), absolute).is_err());
        assert!(environment_hook(absolute, Path::new("relative")).is_err());
        assert!(bootstrap(absolute, absolute, absolute, absolute, absolute, "").is_err());
        assert!(bootstrap(absolute, absolute, absolute, absolute, absolute, "a\0b").is_err());
        assert!(environment_hook(Path::new("/a\0b"), absolute).is_err());
        #[cfg(unix)]
        {
            use std::{ffi::OsStr, os::unix::ffi::OsStrExt};
            assert!(environment_hook(Path::new(OsStr::from_bytes(b"/a\xff")), absolute).is_err());
        }
    }

    #[cfg(unix)]
    mod unix {
        use super::*;
        use std::{
            fs,
            os::unix::fs::PermissionsExt,
            process::{Command, Output},
        };

        fn executable(path: &Path, body: &str) {
            fs::write(path, body).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }

        fn fields(output: &Output) -> Vec<String> {
            assert!(output.stdout.ends_with(&[0]), "{output:?}");
            output.stdout[..output.stdout.len() - 1]
                .split(|byte| *byte == 0)
                .map(|part| String::from_utf8(part.to_vec()).unwrap())
                .collect()
        }

        #[test]
        fn hook_is_silent_conditional_and_preserves_other_environment() {
            verify_hook("/bin/sh");
        }

        #[cfg(target_os = "macos")]
        #[test]
        fn hook_works_in_zsh_without_reading_user_configuration() {
            verify_hook("/bin/zsh");
        }

        fn verify_hook(shell_program: &str) {
            let dir = tempfile::tempdir().unwrap();
            let launcher = dir.path().join("boot 'quoted'\n$(touch nope)");
            let enabled = dir.path().join("enabled '\n");
            executable(&launcher, "#!/bin/sh\nexit 0\n");
            fs::write(&enabled, "").unwrap();
            let hook = environment_hook(&launcher, &enabled).unwrap();
            let probe = format!(
                "{hook}\nprintf '%s\\0' \"${{CODEX_CLI_PATH-unset}}\" \"$CODEX_HOME\" \"$KEEP_ME\"\n"
            );
            let run = |shell: Option<&str>, current: Option<&str>| {
                let mut command = Command::new(shell_program);
                command.env_clear().args(["-f", "-c", &probe]);
                command
                    .env("CODEX_HOME", "/untouched home")
                    .env("KEEP_ME", "yes");
                if let Some(value) = shell {
                    command.env("CODEX_SHELL", value);
                }
                if let Some(value) = current {
                    command.env("CODEX_CLI_PATH", value);
                }
                let output = command.output().unwrap();
                assert!(output.status.success());
                assert!(output.stderr.is_empty());
                let values = fields(&output);
                assert_eq!(&values[1..], ["/untouched home", "yes"]);
                values[0].clone()
            };
            let launcher = launcher.to_str().unwrap();
            assert_eq!(run(Some("1"), None), launcher);
            assert_eq!(run(Some("1"), Some(launcher)), launcher);
            assert_eq!(run(Some("1"), Some("/other/cli")), "/other/cli");
            assert_eq!(run(Some("1"), Some("")), "");
            assert_eq!(run(None, None), "unset");
            assert_eq!(run(Some("0"), None), "unset");
            fs::set_permissions(launcher, fs::Permissions::from_mode(0o600)).unwrap();
            assert_eq!(run(Some("1"), None), "unset");
            fs::set_permissions(launcher, fs::Permissions::from_mode(0o700)).unwrap();
            fs::remove_file(enabled).unwrap();
            assert_eq!(run(Some("1"), None), "unset");
        }

        // PlistBuddy is a real macOS dependency. Other platforms run hook/quote
        // validation above; they do not claim to validate this macOS bootstrap.
        #[cfg(target_os = "macos")]
        mod macos {
            use super::*;
            use std::path::PathBuf;

            struct Fixture {
                _dir: tempfile::TempDir,
                official: PathBuf,
                bridge: PathBuf,
                config: PathBuf,
                enabled: PathBuf,
                plist: PathBuf,
                launcher: PathBuf,
            }

            impl Fixture {
                fn new() -> Self {
                    let dir = tempfile::tempdir().unwrap();
                    let path = dir.path().join("paths 'with'\nspaces");
                    fs::create_dir(&path).unwrap();
                    let fixture = Self {
                        _dir: dir,
                        official: path.join("official"),
                        bridge: path.join("bridge"),
                        config: path.join("config.json"),
                        enabled: path.join("enabled"),
                        plist: path.join("Info.plist"),
                        launcher: path.join("bootstrap"),
                    };
                    let report = "printf '%s\\0' \"${CODEX_CLI_PATH-unset}\" \"${AUTORESUME_BRIDGE_CONFIG-unset}\" \"$CODEX_HOME\" \"$KEEP_ME\" \"$@\"\n";
                    executable(
                        &fixture.official,
                        &format!("#!/bin/sh\nprintf 'official\\0'\n{report}"),
                    );
                    executable(
                        &fixture.bridge,
                        &format!(
                            "#!/bin/sh\nif [ \"${{1-}}\" = '--autoresume-check-config' ]; then\n  printf 'preflight must not leak\\n'\n  printf 'preflight stderr must not leak\\n' >&2\n  [ \"$(/bin/cat \"$AUTORESUME_BRIDGE_CONFIG\")\" = '{{}}' ] || exit 42\n  exit \"${{FAKE_PREFLIGHT_EXIT-0}}\"\nfi\nprintf 'guarded\\0'\n{report}exit \"${{FAKE_BRIDGE_EXIT-0}}\"\n"
                        ),
                    );
                    fs::write(&fixture.config, "{}").unwrap();
                    fs::write(&fixture.enabled, "").unwrap();
                    fixture.write_build("20770");
                    executable(
                        &fixture.launcher,
                        &bootstrap(
                            &fixture.official,
                            &fixture.bridge,
                            &fixture.config,
                            &fixture.enabled,
                            &fixture.plist,
                            "20770",
                        )
                        .unwrap(),
                    );
                    fixture
                }

                fn write_build(&self, build: &str) {
                    fs::write(&self.plist, format!("<?xml version=\"1.0\"?><plist version=\"1.0\"><dict><key>CFBundleVersion</key><string>{build}</string></dict></plist>")).unwrap();
                }

                fn run(&self, env: &[(&str, &str)], args: &[&str]) -> Output {
                    let mut command = Command::new(&self.launcher);
                    command.env_clear().args(args);
                    command.env("CODEX_CLI_PATH", &self.launcher);
                    command.env("AUTORESUME_BRIDGE_CONFIG", "stale config");
                    command.env("CODEX_HOME", "/unchanged private home");
                    command.env("KEEP_ME", "untouched");
                    command.envs(env.iter().copied());
                    command.output().unwrap()
                }

                fn assert_official(&self, output: &Output) {
                    assert!(output.status.success(), "{output:?}");
                    assert_eq!(
                        fields(output),
                        [
                            "official",
                            self.official.to_str().unwrap(),
                            "unset",
                            "/unchanged private home",
                            "untouched",
                            "app-server"
                        ]
                    );
                    assert!(!String::from_utf8_lossy(&output.stderr).contains("preflight"));
                }
            }

            #[test]
            fn disabled_is_silent_and_missing_components_fall_back() {
                let fixture = Fixture::new();
                fs::remove_file(&fixture.enabled).unwrap();
                let output = fixture.run(&[], &["app-server"]);
                fixture.assert_official(&output);
                assert!(output.stderr.is_empty());
                fs::write(&fixture.enabled, "").unwrap();
                fs::remove_file(&fixture.config).unwrap();
                fixture.assert_official(&fixture.run(&[], &["app-server"]));
                fs::write(&fixture.config, "{}").unwrap();
                fs::set_permissions(&fixture.bridge, fs::Permissions::from_mode(0o600)).unwrap();
                fixture.assert_official(&fixture.run(&[], &["app-server"]));
                fs::remove_file(&fixture.bridge).unwrap();
                fixture.assert_official(&fixture.run(&[], &["app-server"]));
            }

            #[test]
            fn changed_missing_or_malformed_plist_falls_back() {
                let fixture = Fixture::new();
                fixture.write_build("20771");
                fixture.assert_official(&fixture.run(&[], &["app-server"]));
                fs::write(&fixture.plist, "invalid plist").unwrap();
                fixture.assert_official(&fixture.run(&[], &["app-server"]));
                fs::remove_file(&fixture.plist).unwrap();
                fixture.assert_official(&fixture.run(&[], &["app-server"]));
            }

            #[test]
            fn preflight_failure_and_unknown_config_fall_back() {
                let fixture = Fixture::new();
                fixture.assert_official(
                    &fixture.run(&[("FAKE_PREFLIGHT_EXIT", "1")], &["app-server"]),
                );
                fs::write(&fixture.config, "{\"unknownField\":true}").unwrap();
                fixture.assert_official(&fixture.run(&[], &["app-server"]));
            }

            #[test]
            fn guarded_failure_never_falls_back_and_arguments_are_literal() {
                let fixture = Fixture::new();
                let args = [
                    "app-server",
                    "",
                    "a 'quote'\nline",
                    "$(touch never)",
                    "--config",
                    "x=\"空 格\"",
                    "*",
                ];
                let output = fixture.run(&[("FAKE_BRIDGE_EXIT", "23")], &args);
                assert_eq!(output.status.code(), Some(23));
                assert!(output.stderr.is_empty());
                let values = fields(&output);
                assert_eq!(
                    &values[..5],
                    [
                        "guarded",
                        fixture.launcher.to_str().unwrap(),
                        fixture.config.to_str().unwrap(),
                        "/unchanged private home",
                        "untouched"
                    ]
                );
                assert_eq!(&values[5..], args);
                fs::remove_file(&fixture.enabled).unwrap();
                let output = fixture.run(&[], &args);
                assert_eq!(&fields(&output)[5..], args);
            }
        }
    }
}
