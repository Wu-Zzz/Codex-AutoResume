//! Owns one exact shell hook block; surrounding profile bytes remain user-owned.
use std::{
    fs::{self, File, Metadata, OpenOptions},
    io::{Read, Write},
    ops::Range,
    path::{Path, PathBuf},
};

const MARKER: &[u8] = b"CODEX_AUTORESUME_DESKTOP_HOOK";

pub fn install(profile: &Path, hook: &Path) -> Result<(), String> {
    let block = hook_block(hook)?;
    let before = read_profile(profile)?;
    let mut bytes = before.as_ref().map_or_else(Vec::new, |s| s.bytes.clone());
    if block_range(&bytes, &block)?.is_some() {
        return Ok(());
    }
    bytes.extend_from_slice(&block);
    replace(profile, before.as_ref(), &bytes)
}

pub fn remove(profile: &Path, hook: &Path) -> Result<(), String> {
    let block = hook_block(hook)?;
    let Some(before) = read_profile(profile)? else {
        return Ok(());
    };
    let Some(range) = block_range(&before.bytes, &block)? else {
        return Ok(());
    };
    let mut bytes = before.bytes.clone();
    bytes.drain(range);
    replace(profile, Some(&before), &bytes)
}

pub fn installed(profile: &Path, hook: &Path) -> Result<bool, String> {
    let block = hook_block(hook)?;
    let current = read_profile(profile)?;
    block_range(current.as_ref().map_or(&[], |s| &s.bytes), &block).map(|r| r.is_some())
}

fn hook_block(hook: &Path) -> Result<Vec<u8>, String> {
    let path = hook.to_str().ok_or("启动脚本路径不是有效的 UTF-8")?;
    if !hook.is_absolute()
        || path.chars().any(char::is_control)
        || path.as_bytes().windows(MARKER.len()).any(|s| s == MARKER)
    {
        return Err("启动脚本路径不符合要求".into());
    }
    let quoted = format!("'{}'", path.replace('\'', "'\\''"));
    // The leading newline is ours too: removing the block restores even a
    // profile whose original last line had no newline, including an empty file.
    Ok(format!(
        "\n# BEGIN CODEX_AUTORESUME_DESKTOP_HOOK\nif [ \"${{CODEX_SHELL:-}}\" = '1' ] && [ -r {quoted} ]; then\n  . {quoted}\nfi\n# END CODEX_AUTORESUME_DESKTOP_HOOK\n"
    )
    .into_bytes())
}

fn block_range(bytes: &[u8], block: &[u8]) -> Result<Option<Range<usize>>, String> {
    let markers = bytes.windows(MARKER.len()).filter(|s| *s == MARKER).count();
    if markers == 0 {
        return Ok(None);
    }
    if markers == 2 {
        if let Some(start) = bytes.windows(block.len()).position(|s| s == block) {
            return Ok(Some(start..start + block.len()));
        }
    }
    Err("启动配置中存在冲突或已修改的 AutoResume 标记，请先检查该标记块".into())
}

struct Profile {
    bytes: Vec<u8>,
    metadata: Metadata,
}

fn same_metadata(a: &Metadata, b: &Metadata) -> bool {
    let same = a.is_file()
        && b.is_file()
        && a.len() == b.len()
        && a.modified().ok() == b.modified().ok()
        && a.permissions().readonly() == b.permissions().readonly();
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        same && a.dev() == b.dev() && a.ino() == b.ino() && a.mode() == b.mode()
    }
    #[cfg(not(unix))]
    same
}

fn read_profile(path: &Path) -> Result<Option<Profile>, String> {
    let initial = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("无法检查 shell 启动配置".into()),
    };
    if !initial.is_file() || initial.file_type().is_symlink() {
        return Err("shell 启动配置必须是普通文件，不能是符号链接".into());
    }
    let mut file = File::open(path).map_err(|_| "无法读取 shell 启动配置")?;
    let opened = file.metadata().map_err(|_| "无法检查已打开的启动配置")?;
    // Inspect the opened inode before reading: a path swap must not cause us to
    // read an unrelated file through a newly introduced symlink.
    if !same_metadata(&initial, &opened) {
        return Err("shell 启动配置在读取时发生变化，请重试".into());
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|_| "无法读取 shell 启动配置")?;
    let after = fs::symlink_metadata(path).map_err(|_| "shell 启动配置在读取时发生变化")?;
    if !same_metadata(&opened, &after) {
        return Err("shell 启动配置在读取时发生变化，请重试".into());
    }
    Ok(Some(Profile {
        bytes,
        metadata: after,
    }))
}

struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn replace(profile: &Path, before: Option<&Profile>, bytes: &[u8]) -> Result<(), String> {
    let parent = profile.parent().ok_or("启动配置路径缺少父目录")?;
    let temporary_path = parent.join(format!(
        ".codex-autoresume-profile-{}.tmp",
        uuid::Uuid::new_v4()
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary_path)
        .map_err(|_| "无法创建启动配置的临时文件")?;
    let temporary = Temporary(temporary_path);
    file.write_all(bytes)
        .map_err(|_| "无法写入启动配置的临时文件")?;
    if let Some(before) = before {
        file.set_permissions(before.metadata.permissions())
            .map_err(|_| "无法保留启动配置权限")?;
    }
    file.sync_all().map_err(|_| "无法保存启动配置的临时文件")?;
    drop(file);
    // An external editor is not coordinated with our writes; re-read immediately
    // before the atomic rename and refuse any observed content/identity change.
    let current = read_profile(profile)?;
    let unchanged = match (before, current.as_ref()) {
        (None, None) => true,
        (Some(a), Some(b)) => a.bytes == b.bytes && same_metadata(&a.metadata, &b.metadata),
        _ => false,
    };
    if !unchanged {
        return Err("shell 启动配置已被其他程序修改，本次操作未保存".into());
    }
    if before.is_none() {
        // Unlike rename, exclusive creation cannot replace a concurrently
        // created profile. Removing our temporary hard link leaves the file.
        fs::hard_link(&temporary.0, profile).map_err(|_| "无法独占创建 shell 启动配置")?;
    } else {
        fs::rename(&temporary.0, profile).map_err(|_| "无法原子更新 shell 启动配置")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn preserves_original_bytes_and_repeated_install_is_noop() {
        let dir = tempdir().unwrap();
        let profile = dir.path().join(".zshrc");
        let hook = dir.path().join("hook.zsh");
        for original in [b"".as_slice(), b"export X=1", b"# user\r\n\xff\n"] {
            fs::write(&profile, original).unwrap();
            install(&profile, &hook).unwrap();
            let installed_bytes = fs::read(&profile).unwrap();
            assert!(installed_bytes.starts_with(original));
            assert!(installed(&profile, &hook).unwrap());
            install(&profile, &hook).unwrap();
            assert_eq!(fs::read(&profile).unwrap(), installed_bytes);
            remove(&profile, &hook).unwrap();
            remove(&profile, &hook).unwrap();
            assert_eq!(fs::read(&profile).unwrap(), original);
        }
    }

    #[test]
    fn creates_missing_profile_and_removes_only_the_owned_block() {
        let dir = tempdir().unwrap();
        let profile = dir.path().join(".zshrc");
        let hook = dir.path().join("hook.zsh");
        assert!(!installed(&profile, &hook).unwrap());
        remove(&profile, &hook).unwrap();
        assert!(!profile.exists());
        install(&profile, &hook).unwrap();
        let mut edited = b"# added before\n".to_vec();
        edited.extend(fs::read(&profile).unwrap());
        edited.extend(b"export LATER=1\n");
        fs::write(&profile, edited).unwrap();
        remove(&profile, &hook).unwrap();
        assert_eq!(
            fs::read(&profile).unwrap(),
            b"# added before\nexport LATER=1\n"
        );
    }

    #[test]
    fn conflicts_or_modified_markers_never_change_user_content() {
        let dir = tempdir().unwrap();
        let profile = dir.path().join(".zshrc");
        let hook = dir.path().join("hook.zsh");
        let block = hook_block(&hook).unwrap();
        let mut duplicate = block.clone();
        duplicate.extend(&block);
        for bytes in [
            hook_block(&dir.path().join("other.zsh")).unwrap(),
            b"# BEGIN CODEX_AUTORESUME_DESKTOP_HOOK\n".to_vec(),
            String::from_utf8(block.clone())
                .unwrap()
                .replace("then", "then # modified")
                .into_bytes(),
            duplicate,
        ] {
            fs::write(&profile, &bytes).unwrap();
            assert!(install(&profile, &hook).is_err());
            assert!(remove(&profile, &hook).is_err());
            assert!(installed(&profile, &hook).is_err());
            assert_eq!(fs::read(&profile).unwrap(), bytes);
        }
    }

    #[test]
    fn refuses_content_changes_since_snapshot_and_cleans_temporary_files() {
        let dir = tempdir().unwrap();
        let profile = dir.path().join(".zshrc");
        fs::write(&profile, b"original").unwrap();
        let before = read_profile(&profile).unwrap().unwrap();
        fs::write(&profile, b"user edit").unwrap();
        assert!(replace(&profile, Some(&before), b"replacement").is_err());
        assert_eq!(fs::read(&profile).unwrap(), b"user edit");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn quotes_shell_metacharacters_without_executing_them() {
        let dir = tempdir().unwrap();
        let hook = dir
            .path()
            .join("quo'te $(touch injected) `touch injected` ; hook");
        fs::write(&hook, "printf 'HOOK_OK'").unwrap();
        let profile = dir.path().join(".zshrc");
        install(&profile, &hook).unwrap();
        for value in ["", "0", "1"] {
            let output = std::process::Command::new("/bin/sh")
                .arg(&profile)
                .env("CODEX_SHELL", value)
                .current_dir(dir.path())
                .output()
                .unwrap();
            assert!(output.status.success());
            assert_eq!(
                output.stdout,
                if value == "1" {
                    b"HOOK_OK".as_slice()
                } else {
                    b""
                }
            );
            assert!(!dir.path().join("injected").exists());
        }
        assert!(hook_block(&dir.path().join("bad\npath")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlinks_and_keeps_permissions() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let dir = tempdir().unwrap();
        let profile = dir.path().join(".zshrc");
        let target = dir.path().join("real-profile");
        let hook = dir.path().join("hook.zsh");
        fs::write(&target, "user settings").unwrap();
        symlink(&target, &profile).unwrap();
        assert!(install(&profile, &hook).is_err());
        assert!(remove(&profile, &hook).is_err());
        assert!(installed(&profile, &hook).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"user settings");
        fs::remove_file(&profile).unwrap();
        fs::write(&profile, "private").unwrap();
        fs::set_permissions(&profile, fs::Permissions::from_mode(0o640)).unwrap();
        install(&profile, &hook).unwrap();
        assert_eq!(
            fs::metadata(&profile).unwrap().permissions().mode() & 0o777,
            0o640
        );
        remove(&profile, &hook).unwrap();
        assert_eq!(
            fs::metadata(&profile).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }
}
