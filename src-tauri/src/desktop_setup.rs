use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

const DESKTOP_BUILD: &str = "20770";
const RECEIPT_VERSION: u32 = 1;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupStatus {
    pub available: bool,
    pub installed: bool,
    pub message: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Receipt {
    version: u32,
    profile: PathBuf,
    runtime: PathBuf,
    desktop_app: PathBuf,
    build: String,
}

pub struct Setup {
    directory: PathBuf,
    resources: PathBuf,
}

impl Setup {
    pub fn new(data: &Path, resources: &Path) -> Self {
        Self {
            directory: data.join("desktop-integration"),
            resources: resources.into(),
        }
    }

    pub fn status(&self) -> SetupStatus {
        let result = self.read_receipt().and_then(|receipt| {
            if let Some(receipt) = receipt {
                let installed = self.enabled().exists();
                let profile_installed =
                    crate::desktop_profile::installed(&receipt.profile, &self.hook())?;
                return Ok(SetupStatus {
                    available: cfg!(target_os = "macos"),
                    installed,
                    message: if installed && !profile_installed {
                        "启动配置已变化，请关闭接入后重新设置。".into()
                    } else if installed {
                        "已设置正常启动接入；首次设置后，请退出 Codex，再从平时的图标打开。".into()
                    } else {
                        "桌面接入已关闭。".into()
                    },
                });
            }
            self.sources()?;
            Ok(SetupStatus {
                available: true,
                installed: false,
                message: "桌面接入尚未启用。".into(),
            })
        });
        result.unwrap_or_else(|message| SetupStatus {
            available: false,
            // Even a damaged receipt must leave a way to turn off the gate.
            installed: self.enabled().exists(),
            message,
        })
    }

    fn enabled(&self) -> PathBuf {
        self.directory.join("enabled")
    }
    fn hook(&self) -> PathBuf {
        self.directory.join("environment.sh")
    }
    fn bootstrap(&self) -> PathBuf {
        self.directory.join("codex")
    }
    fn receipt_path(&self) -> PathBuf {
        self.directory.join("receipt.json")
    }

    fn read_receipt(&self) -> Result<Option<Receipt>, String> {
        let path = self.receipt_path();
        if !path.exists() {
            return Ok(None);
        }
        require_regular(&path)?;
        let data = fs::read(path).map_err(|_| "无法读取桌面接入记录")?;
        if data.len() > 16 * 1024 {
            return Err("桌面接入记录无效".into());
        }
        let receipt: Receipt = serde_json::from_slice(&data).map_err(|_| "桌面接入记录无效")?;
        if receipt.version != RECEIPT_VERSION
            || receipt.runtime.parent() != Some(self.directory.as_path())
            || !receipt
                .runtime
                .file_name()
                .and_then(|v| v.to_str())
                .is_some_and(|v| v.starts_with("runtime-"))
            || !receipt.profile.is_absolute()
        {
            return Err("桌面接入记录不兼容，请检查详情".into());
        }
        Ok(Some(receipt))
    }

    fn sources(&self) -> Result<(PathBuf, PathBuf), String> {
        if !cfg!(target_os = "macos") {
            return Err("正常启动接入目前仅支持 macOS；此平台暂不可启用。".into());
        }
        let exe = std::env::current_exe().map_err(|_| "无法定位应用组件")?;
        let bridge = exe
            .parent()
            .ok_or("无法定位应用组件")?
            .join("codex-autoresume-bridge");
        let backend = self.resources.join("desktop/codex-app-server");
        #[cfg(debug_assertions)]
        let backend = if backend.is_file() {
            backend
        } else {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../work/guarded-codex-build/bin/codex-app-server")
        };
        if !bridge.is_file() || !backend.is_file() {
            return Err("当前安装包未附带完整桌面续跑组件，已有会话仍可监测。".into());
        }
        Ok((bridge, backend))
    }

    pub async fn install(&self) -> Result<SetupStatus, String> {
        if !cfg!(target_os = "macos") {
            return Err("此平台尚不支持正常启动接入".into());
        }
        private_directory(self.directory.parent().ok_or("应用目录无效")?)?;
        private_directory(&self.directory)?;
        let _lock = SetupLock::acquire(&self.directory)?;
        let mut desktop_app = None;
        for candidate in ["/Applications/ChatGPT.app", "/Applications/Codex.app"] {
            let path = PathBuf::from(candidate);
            let info = path.join("Contents/Info.plist");
            if info.is_file()
                && plist_value(&info, "CFBundleIdentifier").await.as_deref()
                    == Ok("com.openai.codex")
                && plist_value(&info, "CFBundleVersion").await.as_deref() == Ok(DESKTOP_BUILD)
            {
                desktop_app = Some(path);
                break;
            }
        }
        let desktop_app = desktop_app.ok_or("未找到已核实版本的 Codex，保留原启动方式。")?;
        let plist = desktop_app.join("Contents/Info.plist");
        let official =
            desktop_app.join("Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex");
        require_regular(&official)?;
        let profile = account_profile().await?;
        if let Some(receipt) = self.read_receipt()? {
            if self.enabled().is_file() {
                return Err("桌面接入已经设置；如需更新组件，请先关闭接入。".into());
            }
            if receipt.profile != profile {
                crate::desktop_profile::remove(&receipt.profile, &self.hook())?;
            }
        }
        // Existing custom CLI overrides are never overwritten, including an
        // explicitly empty value. An inherited pointer to our disabled stub is OK.
        let ordinary_cli = shell_cli(false).await?;
        let scoped_cli = shell_cli(true).await?;
        if scoped_cli
            .as_deref()
            .is_some_and(|value| Some(value) != self.bootstrap().to_str())
        {
            return Err("已配置其他 Codex 执行入口，未覆盖原设置。".into());
        }
        // Validate profile ownership before copying the large runtime.
        crate::desktop_profile::installed(&profile, &self.hook())?;
        let (bridge_source, backend_source) = self.sources()?;
        let runtime = self
            .directory
            .join(format!("runtime-{}", uuid::Uuid::new_v4()));
        private_directory(&runtime)?;
        let bridge = runtime.join("codex-autoresume-bridge");
        let backend = runtime.join("codex-app-server");
        let copy_bridge = bridge.clone();
        let copy_backend = backend.clone();
        tokio::task::spawn_blocking(move || {
            copy_executable(&bridge_source, &copy_bridge)?;
            copy_executable(&backend_source, &copy_backend)
        })
        .await
        .map_err(|_| "安装运行组件已中断")??;
        write_private(
            &runtime.join("LICENSE"),
            include_bytes!("../../integrations/codex/LICENSE"),
            false,
        )?;
        write_private(
            &runtime.join("NOTICE"),
            include_bytes!("../../integrations/codex/NOTICE"),
            false,
        )?;
        let config = crate::desktop_launch::BridgeConfig {
            official_cli: official.clone(),
            guarded_app_server: backend,
            data_dir: self.directory.parent().ok_or("应用目录无效")?.into(),
        };
        let config_path = runtime.join("bridge.json");
        write_private(
            &config_path,
            &serde_json::to_vec_pretty(&config).map_err(|_| "无法保存接入配置")?,
            false,
        )?;
        crate::desktop_launch::BridgeConfig::load(&config_path, &bridge)?;
        let bootstrap = crate::desktop_bootstrap::bootstrap(
            &official,
            &bridge,
            &config_path,
            &self.enabled(),
            &plist,
            DESKTOP_BUILD,
        )?;
        let hook = crate::desktop_bootstrap::environment_hook(&self.bootstrap(), &self.enabled())?;
        write_private(&self.bootstrap(), bootstrap.as_bytes(), true)?;
        write_private(&self.hook(), hook.as_bytes(), false)?;
        let receipt = Receipt {
            version: RECEIPT_VERSION,
            profile: profile.clone(),
            runtime,
            desktop_app,
            build: DESKTOP_BUILD.into(),
        };
        // Save the removable ownership record before touching the user's shell.
        // No execution consent is created by preparing or enabling this entry.
        write_private(
            &self.receipt_path(),
            &serde_json::to_vec_pretty(&receipt).map_err(|_| "无法保存接入记录")?,
            false,
        )?;
        let activate = async {
            crate::desktop_profile::install(&profile, &self.hook())?;
            let token = receipt
                .runtime
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or("缺少桌面运行组件代际")?;
            write_private(&self.enabled(), token.as_bytes(), false)?;
            if shell_cli(true).await?.as_deref() != self.bootstrap().to_str()
                || shell_cli(false).await? != ordinary_cli
            {
                return Err("启动环境核对未通过，已撤销接入；原配置保持不变。".to_owned());
            }
            Ok(())
        }
        .await;
        if let Err(error) = activate {
            // Always disable first, even if a concurrent profile edit prevents
            // removing our exact block. That remaining block becomes inert.
            let _ = fs::remove_file(self.enabled());
            let _ = crate::desktop_profile::remove(&profile, &self.hook());
            return Err(error);
        }
        Ok(self.status())
    }

    pub fn disable(&self) -> Result<SetupStatus, String> {
        if !self.directory.exists() {
            return Ok(self.status());
        }
        let _lock = SetupLock::acquire(&self.directory)?;
        match fs::remove_file(self.enabled()) {
            Ok(()) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => return Err("无法关闭桌面接入开关".into()),
        }
        if let Some(receipt) = self.read_receipt()? {
            crate::desktop_profile::remove(&receipt.profile, &self.hook())?;
        }
        // Retain the disabled bootstrap for already-running clients with an old
        // environment. Future invocations pass through to the official CLI.
        Ok(self.status())
    }
}

fn require_regular(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|_| "桌面接入文件缺失")?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("桌面接入文件类型无效".into());
    }
    Ok(())
}

fn private_directory(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|_| "无法创建桌面接入目录")?;
    let meta = fs::symlink_metadata(path).map_err(|_| "无法检查桌面接入目录")?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err("桌面接入目录类型无效".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|_| "无法保护桌面接入目录")?;
    }
    Ok(())
}

fn write_private(path: &Path, bytes: &[u8], _executable: bool) -> Result<(), String> {
    use std::io::Write;
    let temporary = path.with_file_name(format!(".setup-{}", uuid::Uuid::new_v4()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(if _executable { 0o700 } else { 0o600 });
    }
    let result: Result<(), String> = (|| {
        let mut file = options.open(&temporary).map_err(|_| "无法写入接入配置")?;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|_| "无法保存接入配置")?;
        fs::rename(&temporary, path).map_err(|_| "无法替换接入配置".into())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn copy_executable(source: &Path, target: &Path) -> Result<(), String> {
    require_regular(source)?;
    fs::copy(source, target).map_err(|_| "无法安装桌面运行组件，请检查可用空间")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(target, fs::Permissions::from_mode(0o700))
            .map_err(|_| "无法保护运行组件")?;
    }
    Ok(())
}

// SQLite's OS-backed transaction lock is released even after a crash.
struct SetupLock {
    _connection: rusqlite::Connection,
}
impl SetupLock {
    fn acquire(directory: &Path) -> Result<Self, String> {
        let connection = rusqlite::Connection::open(directory.join("setup-lock.sqlite"))
            .map_err(|_| "无法打开接入设置锁")?;
        connection
            .busy_timeout(std::time::Duration::ZERO)
            .map_err(|_| "无法设置接入锁")?;
        connection
            .execute_batch("BEGIN EXCLUSIVE")
            .map_err(|_| "另一项接入设置尚未结束；请稍后再试。")?;
        Ok(Self {
            _connection: connection,
        })
    }
}

async fn bounded_output(command: &mut tokio::process::Command) -> Result<String, String> {
    command.kill_on_drop(true);
    let output = tokio::time::timeout(std::time::Duration::from_secs(5), command.output())
        .await
        .map_err(|_| "终端启动配置读取超时，尚未启用桌面接入。")?
        .map_err(|_| "无法检查本机启动环境")?;
    if !output.status.success() {
        return Err("本机启动环境检查未通过".into());
    }
    String::from_utf8(output.stdout).map_err(|_| "本机启动环境编码无效".into())
}

async fn plist_value(path: &Path, key: &str) -> Result<String, String> {
    bounded_output(
        tokio::process::Command::new("/usr/libexec/PlistBuddy")
            .arg("-c")
            .arg(format!("Print {key}"))
            .arg(path),
    )
    .await
    .map(|s| s.trim().to_owned())
}

async fn account_profile() -> Result<PathBuf, String> {
    let name = bounded_output(tokio::process::Command::new("/usr/bin/id").arg("-un")).await?;
    let shell = bounded_output(tokio::process::Command::new("/usr/bin/dscl").args([
        ".",
        "-read",
        &format!("/Users/{}", name.trim()),
        "UserShell",
    ]))
    .await?;
    if shell.trim() != "UserShell: /bin/zsh" {
        return Err("当前仅支持 macOS 默认 zsh 启动环境，未修改配置。".into());
    }
    let value = bounded_output(tokio::process::Command::new("/bin/zsh").args([
        "-ilc",
        "printf '\\nAUTORESUME_PROFILE=%s\\n' \"${ZDOTDIR:-$HOME}/.zshrc\"",
    ]))
    .await?;
    let path = value
        .lines()
        .find_map(|line| line.strip_prefix("AUTORESUME_PROFILE="))
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .ok_or("无法确定终端启动配置的位置")?;
    Ok(path)
}

async fn shell_cli(scoped: bool) -> Result<Option<String>, String> {
    // Only inspect the selected CLI, never return or persist the whole env.
    let mut command = tokio::process::Command::new("/bin/zsh");
    if scoped {
        command.env("CODEX_SHELL", "1");
    } else {
        command.env_remove("CODEX_SHELL");
    }
    let output = bounded_output(command.args(["-ilc",
        "if [ \"${CODEX_CLI_PATH+x}\" = x ]; then printf '\\nAUTORESUME_CLI_SET=%s\\n' \"$CODEX_CLI_PATH\"; else printf '\\nAUTORESUME_CLI_UNSET\\n'; fi"
    ])).await?;
    for line in output.lines().rev() {
        if line == "AUTORESUME_CLI_UNSET" {
            return Ok(None);
        }
        if let Some(value) = line.strip_prefix("AUTORESUME_CLI_SET=") {
            return Ok(Some(value.into()));
        }
    }
    Err("无法核对终端环境；尚未启用桌面接入。".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_lock_excludes_other_connections_and_can_be_reacquired() {
        let directory = tempfile::tempdir().unwrap();
        let first = SetupLock::acquire(directory.path()).unwrap();
        assert!(SetupLock::acquire(directory.path()).is_err());
        drop(first);
        // The persistent lock file may remain after an exit; it never means busy.
        assert!(directory.path().join("setup-lock.sqlite").exists());
        assert!(SetupLock::acquire(directory.path()).is_ok());
    }

    #[test]
    fn disable_removes_only_our_entry_and_retains_the_fallback_for_old_clients() {
        let directory = tempfile::tempdir().unwrap();
        let setup = Setup::new(directory.path(), directory.path());
        private_directory(&setup.directory).unwrap();
        let profile = directory.path().join(".zshrc");
        fs::write(&profile, b"# existing user settings\n").unwrap();
        let receipt = Receipt {
            version: RECEIPT_VERSION,
            profile: profile.clone(),
            runtime: setup.directory.join("runtime-test"),
            desktop_app: PathBuf::from("/Applications/Codex.app"),
            build: DESKTOP_BUILD.into(),
        };
        write_private(
            &setup.receipt_path(),
            &serde_json::to_vec(&receipt).unwrap(),
            false,
        )
        .unwrap();
        write_private(&setup.bootstrap(), b"old client fallback", true).unwrap();
        write_private(&setup.enabled(), b"runtime-test", false).unwrap();
        crate::desktop_profile::install(&profile, &setup.hook()).unwrap();
        assert!(setup.status().installed);
        assert!(!setup.disable().unwrap().installed);
        assert!(!setup.enabled().exists());
        assert!(setup.bootstrap().exists());
        assert_eq!(fs::read(&profile).unwrap(), b"# existing user settings\n");
        assert!(!setup.disable().unwrap().installed);
    }

    #[test]
    fn receipt_rejects_unknown_fields_and_runtime_outside_owned_directory() {
        let directory = tempfile::tempdir().unwrap();
        let setup = Setup::new(directory.path(), directory.path());
        private_directory(&setup.directory).unwrap();
        let mut receipt = serde_json::json!({"version":1,"profile":directory.path().join(".zshrc"),
            "runtime":setup.directory.join("runtime-test"),"desktopApp":"/Applications/Codex.app","build":DESKTOP_BUILD});
        write_private(
            &setup.receipt_path(),
            &serde_json::to_vec(&receipt).unwrap(),
            false,
        )
        .unwrap();
        assert!(setup.read_receipt().unwrap().is_some());
        receipt["unexpected"] = true.into();
        write_private(
            &setup.receipt_path(),
            &serde_json::to_vec(&receipt).unwrap(),
            false,
        )
        .unwrap();
        assert!(setup.read_receipt().is_err());
        receipt.as_object_mut().unwrap().remove("unexpected");
        receipt["runtime"] = "/unrelated/runtime-test".into();
        write_private(
            &setup.receipt_path(),
            &serde_json::to_vec(&receipt).unwrap(),
            false,
        )
        .unwrap();
        assert!(setup.read_receipt().is_err());
    }
}
