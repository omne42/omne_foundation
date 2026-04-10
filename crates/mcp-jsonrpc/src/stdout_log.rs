use std::path::{Path, PathBuf};

#[cfg(unix)]
use cap_fs_ext::OpenOptionsExt;
use cap_fs_ext::OpenOptionsFollowExt;
use cap_std::fs::OpenOptions;
use omne_fs_primitives::{MissingRootPolicy, open_root};
use tokio::io::AsyncWriteExt;

use crate::StdoutLog;

const APPENDABLE_OPEN_NOT_FOUND_RETRIES: usize = 4;

async fn ensure_stdout_log_path_has_no_symlink(path: &Path) -> Result<(), std::io::Error> {
    use std::path::Component;

    let skip_last_component_symlink_check = cfg!(unix);
    let mut components = path.components().peekable();
    let mut current = PathBuf::new();
    while let Some(component) = components.next() {
        let is_last = components.peek().is_none();
        match component {
            Component::Prefix(prefix) => current.push(prefix.as_os_str()),
            Component::RootDir => current.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                current.pop();
            }
            Component::Normal(part) => {
                current.push(part);
                if is_last && skip_last_component_symlink_check {
                    // Do not pre-check the final path component. On unix we open with `O_NOFOLLOW`
                    // to prevent TOCTOU symlink replacement.
                    continue;
                }
                match tokio::fs::symlink_metadata(&current).await {
                    Ok(metadata) => {
                        if metadata.file_type().is_symlink() {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::PermissionDenied,
                                format!(
                                    "stdout_log path contains symlink component: {}",
                                    current.display()
                                ),
                            ));
                        }
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => break,
                    Err(err) => return Err(err),
                }
            }
        }
    }
    Ok(())
}

async fn open_stdout_log_append(path: &Path) -> Result<tokio::fs::File, std::io::Error> {
    ensure_stdout_log_path_has_no_symlink(path).await?;

    let path = path.to_path_buf();
    let file = tokio::task::spawn_blocking(move || open_stdout_log_append_std(&path))
        .await
        .map_err(|err| {
            std::io::Error::other(format!("open stdout_log path task failed: {err}"))
        })??;
    Ok(tokio::fs::File::from_std(file))
}

fn open_stdout_log_append_std(path: &Path) -> Result<std::fs::File, std::io::Error> {
    let mut last_not_found = None;

    for attempt in 0..APPENDABLE_OPEN_NOT_FOUND_RETRIES {
        match open_stdout_log_append_std_once(path) {
            Ok(file) => return Ok(file),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                last_not_found = Some(err);
                if attempt + 1 < APPENDABLE_OPEN_NOT_FOUND_RETRIES {
                    std::thread::yield_now();
                    continue;
                }
            }
            Err(err) => return Err(err),
        }
    }

    Err(last_not_found
        .unwrap_or_else(|| std::io::Error::other("stdout_log open failed without an error")))
}

fn open_stdout_log_append_std_once(path: &Path) -> Result<std::fs::File, std::io::Error> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "stdout_log path must include a parent directory: {}",
                path.display()
            ),
        )
    })?;
    let leaf = path.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "stdout_log path must include a file name: {}",
                path.display()
            ),
        )
    })?;

    let root = open_root(
        parent,
        "stdout_log path",
        MissingRootPolicy::Create,
        |_, _, _, error| error,
    )?
    .ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!(
                "stdout_log parent directory not found: {}",
                parent.display()
            ),
        )
    })?;

    let mut options = OpenOptions::new();
    options.create(true).append(true);
    options.follow(cap_fs_ext::FollowSymlinks::No);
    #[cfg(unix)]
    {
        options.mode(0o600);
    }

    let file = root.into_dir().open_with(Path::new(leaf), &options)?;
    ensure_regular_stdout_log_file(file.into_std(), path)
}

fn ensure_regular_stdout_log_file(
    file: std::fs::File,
    path: &Path,
) -> Result<std::fs::File, std::io::Error> {
    let metadata = file.metadata()?;
    if metadata.is_file() {
        return Ok(file);
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        format!("stdout_log path must be a regular file: {}", path.display()),
    ))
}

pub(crate) struct LogState {
    base_path: PathBuf,
    max_bytes_per_part: u64,
    max_parts: Option<u32>,
    file: tokio::fs::File,
    current_len: u64,
    next_part: u32,
}

impl LogState {
    pub(crate) async fn new(opts: StdoutLog) -> Result<Self, std::io::Error> {
        let base_path = opts.path;
        let max_bytes_per_part = opts.max_bytes_per_part.max(1);
        let max_parts = opts.max_parts.filter(|v| *v > 0);

        ensure_stdout_log_path_has_no_symlink(&base_path).await?;
        let file = open_stdout_log_append(&base_path).await?;
        let current_len = file.metadata().await.map(|m| m.len()).unwrap_or(0);
        let next_part = next_rotating_log_part(&base_path).await.unwrap_or(1);
        if let Some(max_parts) = max_parts {
            prune_rotating_log_parts(&base_path, max_parts).await?;
        }

        Ok(Self {
            base_path,
            max_bytes_per_part,
            max_parts,
            file,
            current_len,
            next_part,
        })
    }

    pub(crate) async fn write_line_bytes(&mut self, line: &[u8]) -> Result<(), std::io::Error> {
        self.write_bytes_with_rotation(line).await?;
        if !line.ends_with(b"\n") {
            self.write_bytes_with_rotation(b"\n").await?;
        }
        Ok(())
    }

    async fn write_bytes_with_rotation(&mut self, mut bytes: &[u8]) -> Result<(), std::io::Error> {
        while !bytes.is_empty() {
            let remaining = self.max_bytes_per_part.saturating_sub(self.current_len);
            if remaining == 0 {
                self.file.flush().await?;
                self.next_part = rotate_log_file(&self.base_path, self.next_part).await?;
                if let Some(max_parts) = self.max_parts {
                    prune_rotating_log_parts(&self.base_path, max_parts).await?;
                }
                self.file = open_stdout_log_append(&self.base_path).await?;
                self.current_len = 0;
                continue;
            }

            let take = usize::try_from(remaining.min(bytes.len() as u64)).unwrap_or(bytes.len());
            self.file.write_all(&bytes[..take]).await?;
            self.current_len = self.current_len.saturating_add(take as u64);
            bytes = &bytes[take..];
        }
        Ok(())
    }
}

async fn next_rotating_log_part(base_path: &Path) -> Result<u32, std::io::Error> {
    let Some(parent) = base_path.parent() else {
        return Ok(1);
    };
    let Some(stem) = base_path.file_stem().and_then(|s| s.to_str()) else {
        return Ok(1);
    };

    let mut read_dir = match tokio::fs::read_dir(parent).await {
        Ok(read_dir) => read_dir,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(1),
        Err(err) => return Err(err),
    };

    let prefix = format!("{stem}.segment-");
    let mut max_part = 0u32;
    while let Some(entry) = read_dir.next_entry().await? {
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        let Some(rest) = name.strip_prefix(&prefix) else {
            continue;
        };
        let Some(part_str) = rest.strip_suffix(".log") else {
            continue;
        };
        let Ok(part) = part_str.parse::<u32>() else {
            continue;
        };
        let ty = entry.file_type().await?;
        if !ty.is_file() {
            continue;
        }
        max_part = max_part.max(part);
    }

    Ok(max_part.saturating_add(1).max(1))
}

async fn rotate_log_file(base_path: &Path, mut part: u32) -> Result<u32, std::io::Error> {
    let Some(parent) = base_path.parent() else {
        return Ok(part);
    };
    let Some(stem) = base_path.file_stem().and_then(|s| s.to_str()) else {
        return Ok(part);
    };

    loop {
        let rotated = parent.join(format!("{stem}.segment-{part:04}.log"));
        match tokio::fs::symlink_metadata(&rotated).await {
            Ok(_) => {
                let Some(next_part) = part.checked_add(1) else {
                    return Err(std::io::Error::other("stdout_log rotation index exhausted"));
                };
                part = next_part;
                continue;
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
        match tokio::fs::rename(base_path, &rotated).await {
            Ok(()) => return Ok(part.checked_add(1).unwrap_or(part)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(part),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                let Some(next_part) = part.checked_add(1) else {
                    return Err(std::io::Error::other("stdout_log rotation index exhausted"));
                };
                part = next_part;
            }
            Err(err) => {
                return Err(err);
            }
        }
    }
}

pub(crate) async fn list_rotating_log_parts(
    base_path: &Path,
) -> Result<Vec<(u32, PathBuf)>, std::io::Error> {
    let Some(parent) = base_path.parent() else {
        return Ok(Vec::new());
    };
    let Some(stem) = base_path.file_stem().and_then(|s| s.to_str()) else {
        return Ok(Vec::new());
    };

    let mut read_dir = match tokio::fs::read_dir(parent).await {
        Ok(read_dir) => read_dir,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };

    let prefix = format!("{stem}.segment-");
    let mut parts = Vec::new();
    while let Some(entry) = read_dir.next_entry().await? {
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        let Some(rest) = name.strip_prefix(&prefix) else {
            continue;
        };
        let Some(part_str) = rest.strip_suffix(".log") else {
            continue;
        };
        let Ok(part) = part_str.parse::<u32>() else {
            continue;
        };
        let ty = entry.file_type().await?;
        if !ty.is_file() {
            continue;
        }

        parts.push((part, entry.path()));
    }

    Ok(parts)
}

pub(crate) async fn prune_rotating_log_parts(
    base_path: &Path,
    max_parts: u32,
) -> Result<(), std::io::Error> {
    #[cfg(test)]
    if let Some(err) = take_injected_prune_error() {
        return Err(err);
    }

    if max_parts == 0 {
        return Ok(());
    }
    let mut parts = list_rotating_log_parts(base_path).await?;
    parts.sort_by_key(|(part, _)| *part);

    let keep = max_parts as usize;
    if parts.len() <= keep {
        return Ok(());
    }

    let remove = parts.len().saturating_sub(keep);
    for (_part, path) in parts.into_iter().take(remove) {
        remove_rotating_log_part(&path).await?;
    }

    Ok(())
}

#[cfg(not(test))]
async fn remove_rotating_log_part(path: &Path) -> Result<(), std::io::Error> {
    tokio::fs::remove_file(path).await
}

#[cfg(test)]
async fn remove_rotating_log_part(path: &Path) -> Result<(), std::io::Error> {
    if let Some(err) = take_injected_remove_file_error() {
        return Err(err);
    }
    tokio::fs::remove_file(path).await
}

#[cfg(test)]
fn injected_prune_error_slot() -> &'static std::sync::Mutex<Option<std::io::ErrorKind>> {
    static SLOT: std::sync::OnceLock<std::sync::Mutex<Option<std::io::ErrorKind>>> =
        std::sync::OnceLock::new();
    SLOT.get_or_init(|| std::sync::Mutex::new(None))
}

#[cfg(test)]
fn injected_remove_file_error_slot() -> &'static std::sync::Mutex<Option<std::io::ErrorKind>> {
    static SLOT: std::sync::OnceLock<std::sync::Mutex<Option<std::io::ErrorKind>>> =
        std::sync::OnceLock::new();
    SLOT.get_or_init(|| std::sync::Mutex::new(None))
}

#[cfg(test)]
fn prune_test_mutex() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

#[cfg(test)]
fn inject_prune_error(kind: std::io::ErrorKind) {
    let mut slot = injected_prune_error_slot()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *slot = Some(kind);
}

#[cfg(test)]
fn take_injected_prune_error() -> Option<std::io::Error> {
    let mut slot = injected_prune_error_slot()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    slot.take()
        .map(|kind| std::io::Error::new(kind, "injected stdout_log prune failure"))
}

#[cfg(test)]
fn inject_remove_file_error(kind: std::io::ErrorKind) {
    let mut slot = injected_remove_file_error_slot()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *slot = Some(kind);
}

#[cfg(test)]
fn take_injected_remove_file_error() -> Option<std::io::Error> {
    let mut slot = injected_remove_file_error_slot()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    slot.take()
        .map(|kind| std::io::Error::new(kind, "injected stdout_log remove_file failure"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn prune_rotating_log_parts_keeps_latest_n() {
        let _guard = prune_test_mutex()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("server.stdout.log");

        for part in 1..=5u32 {
            let path = dir
                .path()
                .join(format!("server.stdout.segment-{part:04}.log"));
            tokio::fs::write(&path, format!("part-{part}\n"))
                .await
                .unwrap();
        }

        prune_rotating_log_parts(&base, 2).await.unwrap();
        let mut parts = list_rotating_log_parts(&base).await.unwrap();
        parts.sort_by_key(|(part, _)| *part);
        assert_eq!(
            parts.iter().map(|(p, _)| *p).collect::<Vec<_>>(),
            vec![4, 5]
        );
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn log_state_new_propagates_prune_failure() {
        let _guard = prune_test_mutex()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("server.stdout.log");
        inject_prune_error(std::io::ErrorKind::PermissionDenied);

        let err = match LogState::new(StdoutLog {
            path: base,
            max_bytes_per_part: 1024,
            max_parts: Some(1),
        })
        .await
        {
            Ok(_) => panic!("init should fail when prune cannot enforce max_parts"),
            Err(err) => err,
        };
        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn write_line_bytes_propagates_rotation_prune_failure() {
        let _guard = prune_test_mutex()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("server.stdout.log");
        let mut state = LogState::new(StdoutLog {
            path: base,
            max_bytes_per_part: 1,
            max_parts: Some(1),
        })
        .await
        .unwrap();

        state.write_line_bytes(b"a").await.unwrap();
        inject_prune_error(std::io::ErrorKind::PermissionDenied);
        let err = state
            .write_line_bytes(b"b")
            .await
            .expect_err("rotation should fail when prune cannot enforce max_parts");
        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn prune_rotating_log_parts_propagates_remove_file_failure() {
        let _guard = prune_test_mutex()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("server.stdout.log");
        for part in 1..=2u32 {
            let path = dir
                .path()
                .join(format!("server.stdout.segment-{part:04}.log"));
            tokio::fs::write(path, format!("part-{part}\n"))
                .await
                .unwrap();
        }

        inject_remove_file_error(std::io::ErrorKind::PermissionDenied);
        let err = prune_rotating_log_parts(&base, 1)
            .await
            .expect_err("prune should fail when an old part cannot be removed");
        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
    }

    #[tokio::test]
    async fn write_line_bytes_preserves_order_across_rotation() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("server.stdout.log");
        let mut state = LogState::new(StdoutLog {
            path: base.clone(),
            max_bytes_per_part: 3,
            max_parts: None,
        })
        .await
        .unwrap();

        state.write_line_bytes(b"abc").await.unwrap();
        drop(state);

        let mut parts = list_rotating_log_parts(&base).await.unwrap();
        parts.sort_by_key(|(part, _)| *part);

        let mut out = Vec::new();
        for (_part, path) in parts {
            out.extend(tokio::fs::read(path).await.unwrap());
        }
        out.extend(tokio::fs::read(&base).await.unwrap());
        assert_eq!(out, b"abc\n");
    }

    #[tokio::test]
    async fn rotate_log_file_fails_when_segment_index_exhausted() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("server.stdout.log");
        tokio::fs::write(&base, b"x").await.unwrap();
        let max_segment = dir.path().join("server.stdout.segment-4294967295.log");
        tokio::fs::write(&max_segment, b"taken").await.unwrap();

        let err = rotate_log_file(&base, u32::MAX)
            .await
            .expect_err("rotation should fail after reaching u32::MAX segment");
        assert_eq!(err.kind(), std::io::ErrorKind::Other);
        assert!(err.to_string().contains("index exhausted"));
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn rotate_log_file_at_max_keeps_part_index() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("server.stdout.log");
        tokio::fs::write(&base, b"x").await.unwrap();

        let next_part = rotate_log_file(&base, u32::MAX)
            .await
            .expect("rotation at max part should not hang");
        assert_eq!(next_part, u32::MAX);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stdout_log_rejects_symlink_target() {
        let base = std::fs::canonicalize(std::env::current_dir().unwrap()).unwrap();
        let dir = tempfile::tempdir_in(base).unwrap();
        let target = dir.path().join("target.log");
        tokio::fs::write(&target, b"ok\n").await.unwrap();

        let link = dir.path().join("link.log");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let err = LogState::new(StdoutLog {
            path: link,
            max_bytes_per_part: 1024,
            max_parts: None,
        })
        .await
        .err()
        .expect("should reject symlink stdout_log path");

        assert_eq!(err.raw_os_error(), Some(libc::ELOOP));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stdout_log_rejects_symlink_parent_dir() {
        let dir = tempfile::tempdir().unwrap();
        let real_dir = dir.path().join("real");
        tokio::fs::create_dir_all(&real_dir).await.unwrap();

        let link_dir = dir.path().join("link");
        std::os::unix::fs::symlink(&real_dir, &link_dir).unwrap();

        let err = LogState::new(StdoutLog {
            path: link_dir.join("server.stdout.log"),
            max_bytes_per_part: 1024,
            max_parts: None,
        })
        .await
        .err()
        .expect("should reject symlink stdout_log parent dir");

        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(
            err.to_string()
                .contains("stdout_log path contains symlink component")
        );
    }

    #[tokio::test]
    async fn stdout_log_creates_missing_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir
            .path()
            .join("nested")
            .join("logs")
            .join("server.stdout.log");

        let mut state = LogState::new(StdoutLog {
            path: base.clone(),
            max_bytes_per_part: 1024,
            max_parts: None,
        })
        .await
        .expect("missing parents should be created securely");
        state.write_line_bytes(b"ok").await.unwrap();
        state.file.flush().await.unwrap();
        drop(state);

        assert!(base.parent().unwrap().is_dir());
        assert_eq!(tokio::fs::read(&base).await.unwrap(), b"ok\n");
    }

    #[tokio::test]
    async fn write_line_bytes_rotates_large_line_without_extra_newline()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let base = dir.path().join("server.stdout.log");

        let mut state = LogState::new(StdoutLog {
            path: base.clone(),
            max_bytes_per_part: 4,
            max_parts: None,
        })
        .await?;
        state.write_line_bytes(b"abcdef").await?;
        state.file.flush().await?;
        drop(state);

        let mut parts = list_rotating_log_parts(&base).await?;
        parts.sort_by_key(|(part, _)| *part);

        let mut combined = Vec::new();
        for (_part, path) in parts {
            combined.extend_from_slice(&tokio::fs::read(path).await?);
        }
        combined.extend_from_slice(&tokio::fs::read(&base).await?);

        assert_eq!(combined, b"abcdef\n");
        Ok(())
    }
}
