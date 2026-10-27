// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Canonical path boundaries, digests, and atomic local writes.
use std::fs;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use serde::de::DeserializeOwned;
use sha2::Digest;
use sha2::Sha256;
use walkdir::WalkDir;

use crate::error_context::ErrorContext;
use crate::required::Required;
use crate::tools_error::Result;
use crate::tools_error::config;
pub(crate) static CANCELLED: AtomicBool = AtomicBool::new(false);
static COUNTER: AtomicU64 = AtomicU64::new(0);
pub(crate) fn cancelled() -> Result<()> {
    if CANCELLED.load(Ordering::SeqCst) {
        Err(crate::ToolsError::Cancelled)
    } else {
        Ok(())
    }
}
pub(crate) fn unique() -> String {
    format!(
        "{}-{}-{}",
        chrono::Utc::now().format("%Y%m%dT%H%M%S%3fZ"),
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}
pub(crate) fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
pub(crate) fn valid_digest(value: &str) -> Result<()> {
    if value.len() == 64 && value.bytes().all(|c| c.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(config("invalid SHA-256 digest"))
    }
}
pub(crate) fn verify_hash(bytes: &[u8], expected: &str) -> Result<()> {
    valid_digest(expected)?;
    if hash(bytes).eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(config("SHA-256 mismatch"))
    }
}
pub(crate) fn parse_json<T: DeserializeOwned>(bytes: &[u8], label: &str) -> Result<T> {
    serde_json::from_slice(bytes).configuration_context(format_args!("{label}"))
}
pub(crate) fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    parse_json(&fs::read(path)?, &path.display().to_string())
}
pub(crate) fn json_bytes(value: &impl serde::Serialize) -> Result<Vec<u8>> {
    serde_json::to_vec_pretty(value).configuration_display()
}
pub(crate) fn safe_relative(value: &str) -> Result<PathBuf> {
    let path = Path::new(value);
    if value.is_empty()
        || value.contains('\\')
        || value.contains(':')
        || path.components().any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(config(format!("unsafe relative path: {value}")));
    }
    let canonical = path
        .components()
        .map(|part| match part {
            Component::Normal(value) => value.to_str().required("path is not UTF-8"),
            _ => Err(config("unsafe path component")),
        })
        .collect::<Result<Vec<_>>>()?
        .join("/");
    if canonical != value {
        return Err(config(format!("non-canonical relative path: {value}")));
    }
    Ok(path.to_path_buf())
}
pub(crate) fn within(root: &Path, relative: &str) -> Result<PathBuf> {
    let path = root.join(safe_relative(relative)?);
    let mut ancestor = path.as_path();
    while !ancestor.exists() {
        ancestor = ancestor.parent().required("missing path ancestor")?;
    }
    let canonical = ancestor.canonicalize()?;
    if !canonical.starts_with(root) {
        return Err(config(format!("path escapes project: {relative}")));
    }
    // Managed outputs never traverse symlinks, even when their destination remains
    // inside root.
    let mut cursor = root.to_path_buf();
    for part in Path::new(relative).components() {
        cursor.push(part);
        if let Ok(metadata) = fs::symlink_metadata(&cursor)
            && metadata.file_type().is_symlink()
        {
            return Err(config(format!("symbolic link in managed path: {relative}")));
        }
    }
    Ok(path)
}
pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    atomic_write_inner(path, bytes, None)
}
pub(crate) fn atomic_write_mode(path: &Path, bytes: &[u8], mode: bool) -> Result<()> {
    atomic_write_inner(path, bytes, Some(mode))
}
fn atomic_write_inner(path: &Path, bytes: &[u8], mode: Option<bool>) -> Result<()> {
    let parent = path.parent().required("output path has no parent")?;
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".cpp-infra-write-{}", unique()));
    fs::write(&temporary, bytes)?;
    if let Some(enabled) = mode {
        executable(&temporary, enabled)?;
    }
    fs::File::open(&temporary)?.sync_all()?;
    fs::rename(&temporary, path)?;
    #[cfg(unix)]
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}
pub(crate) fn executable(path: &Path, enabled: bool) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)?.permissions();
        let mode = permissions.mode();
        permissions.set_mode(if enabled { mode | 0o111 } else { mode & !0o111 });
        fs::set_permissions(path, permissions)?;
    }
    #[cfg(not(unix))]
    {
        let _ = (path, enabled);
    }
    Ok(())
}
pub(crate) fn is_executable(path: &Path) -> Result<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Ok(fs::metadata(path)?.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(true)
    }
}
pub(crate) fn digests(project: &Path, excluded: &[String]) -> Result<(String, String)> {
    let mut sources = Vec::new();
    let mut configs = Vec::new();
    for entry in WalkDir::new(project)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            if entry.path() == project {
                return true;
            }
            let name = entry.file_name().to_str().unwrap_or("");
            if entry.depth() == 1 && entry.file_type().is_dir() && [".git", "build", "target"].contains(&name) {
                return false;
            }
            let relative = entry
                .path()
                .strip_prefix(project)
                .ok()
                .and_then(Path::to_str)
                .unwrap_or("");
            !(entry.file_type().is_dir()
                && [".infra/.tmp", ".infra/transactions", ".infra/.transactions"].contains(&relative))
        })
    {
        let entry = entry.infrastructure_context(format_args!("project traversal"))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry.path().strip_prefix(project).configuration_display()?;
        let name = relative
            .components()
            .map(|part| match part {
                Component::Normal(value) => value.to_str().required("project path is not UTF-8"),
                _ => Err(config("project path alias")),
            })
            .collect::<Result<Vec<_>>>()?
            .join("/");
        safe_relative(&name)?;
        if !name.starts_with(".infra/") && !excluded.contains(&name) {
            sources.push((name.clone(), entry.path().to_path_buf()));
        }
        if name.starts_with(".infra/")
            || ["CMakeLists.txt", "CMakePresets.json", "conanfile.py", "conanfile.txt"].contains(&name.as_str())
            || name.starts_with("conan/")
        {
            configs.push((name, entry.path().to_path_buf()));
        }
    }
    fn digest(mut files: Vec<(String, PathBuf)>) -> Result<String> {
        files.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        let mut digest = Sha256::new();
        for (name, path) in files {
            let bytes = fs::read(path)?;
            digest.update((name.len() as u64).to_le_bytes());
            digest.update(name.as_bytes());
            digest.update((bytes.len() as u64).to_le_bytes());
            digest.update(bytes);
        }
        Ok(hex::encode(digest.finalize()))
    }
    Ok((digest(sources)?, digest(configs)?))
}

/// Wait for a cache/project lock while preserving cancellation responsiveness.
pub(crate) fn acquire_lock(file: &fs::File, exclusive: bool) -> Result<()> {
    loop {
        cancelled()?;
        let result = if exclusive {
            fs2::FileExt::try_lock_exclusive(file)
        } else {
            fs2::FileExt::try_lock_shared(file)
        };
        match result {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(20))
            }
            Err(error) => return Err(error.into()),
        }
    }
}

/// Hash a declared artifact without any project-source filtering.
pub(crate) fn artifact_digest(path: &Path) -> Result<String> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(config("artifact symlinks are forbidden"));
    }
    if metadata.is_file() {
        return Ok(hash(&fs::read(path)?));
    }
    if !metadata.is_dir() {
        return Err(config("artifact is not a regular file or directory"));
    }
    let mut files = Vec::new();
    for entry in WalkDir::new(path).follow_links(false) {
        let entry = entry.configuration_display()?;
        if entry.file_type().is_symlink() {
            return Err(config("artifact tree contains a symlink"));
        }
        if entry.file_type().is_dir() {
            continue;
        }
        if !entry.file_type().is_file() {
            return Err(config("artifact tree contains a special file"));
        }
        let relative = entry.path().strip_prefix(path).configuration_display()?;
        let name = relative
            .components()
            .map(|part| match part {
                Component::Normal(value) => value.to_str().required("artifact path is not UTF-8"),
                _ => Err(config("artifact path alias")),
            })
            .collect::<Result<Vec<_>>>()?
            .join("/");
        safe_relative(&name)?;
        files.push((name, entry.path().to_path_buf()));
    }
    files.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let mut digest = Sha256::new();
    for (name, file) in files {
        let bytes = fs::read(file)?;
        digest.update((name.len() as u64).to_le_bytes());
        digest.update(name.as_bytes());
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(bytes);
    }
    Ok(hex::encode(digest.finalize()))
}

/// Validate consumed project configuration against the shared digest blind
/// spots.
pub(crate) fn config_location(project: &Path, path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        project.join(path)
    };
    let relative = absolute
        .strip_prefix(project)
        .configuration("configuration input is outside project")?;
    let text = relative.to_str().required("configuration path is not UTF-8")?;
    safe_relative(text)?;
    let mut parts = relative.components();
    let first = parts.next();
    if matches!(first,Some(Component::Normal(name))if [".git","build","target"].contains(&name.to_str().unwrap_or("")))
        || [".infra/.tmp", ".infra/transactions", ".infra/.transactions"]
            .iter()
            .any(|root| relative.starts_with(root))
    {
        return Err(config("configuration input is in a digest-excluded directory"));
    }
    within(project, text)
}
pub(crate) fn tracked_config(project: &Path, path: &Path) -> Result<PathBuf> {
    let validated = config_location(project, path)?;
    if !fs::symlink_metadata(&validated)?.file_type().is_file() {
        return Err(config("configuration input must be a non-symlink regular file"));
    }
    Ok(validated)
}

/// Read configuration text, preserving filesystem failures as infrastructure
/// errors. Invalid UTF-8 is a configuration error annotated with the input
/// path.
pub(crate) fn read_configuration_text(path: &Path) -> Result<String> {
    String::from_utf8(fs::read(path)?).configuration_context(format_args!("configuration {}", path.display()))
}
