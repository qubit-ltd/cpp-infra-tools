// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Receipt-authorized update journals; untrusted project state never grants
//! writes.
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;
use serde_json::json;
use walkdir::WalkDir;

use crate::error_context::ErrorContext;
use crate::manifest::Manifest;
use crate::required::Required;
use crate::tools_error::Result;
use crate::tools_error::config;
use crate::util::atomic_write;
use crate::util::atomic_write_mode;
use crate::util::hash;
use crate::util::is_executable;
use crate::util::json_bytes;
use crate::util::read_json;
use crate::util::safe_relative;
use crate::util::unique;
use crate::util::verify_hash;
use crate::util::within;
const DIRECTORY: &str = ".infra/transactions/tools-update";
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    schema: u32,
    transaction_id: String,
    project: PathBuf,
    old_manifest: Option<Manifest>,
    new_manifest: Manifest,
    initial_files: Vec<String>,
    entries: Vec<Entry>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    target: String,
    existed: bool,
    executable: bool,
    new_executable: bool,
    backup: String,
    old_sha256: Option<String>,
    new_sha256: Option<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    schema: u32,
    project: PathBuf,
    transaction_id: String,
    journal_sha256: String,
    journal: Journal,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Owner {
    schema: u32,
    owner: String,
    transaction_id: String,
    receipt: PathBuf,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SnapshotReceipt {
    schema: u32,
    project: PathBuf,
    manifest_sha256: String,
    manifest: Manifest,
}
fn snapshot_path(project: &Path) -> Result<PathBuf> {
    Ok(receipt_directory(project)?.join(format!(
        "project-{}.json",
        hash(project.to_str().required("project path is not UTF-8")?.as_bytes())
    )))
}
fn verify_private_file(path: &Path) -> Result<()> {
    if !fs::symlink_metadata(path)
        .configuration("missing trusted receipt")?
        .file_type()
        .is_file()
    {
        return Err(config("trusted receipt must be a regular file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if fs::metadata(path)?.permissions().mode() & 0o077 != 0 {
            return Err(config("trusted receipt must have 0600 permissions"));
        }
    }
    Ok(())
}
fn verify_snapshot(project: &Path, manifest: &Manifest) -> Result<()> {
    let path = snapshot_path(project)?;
    verify_private_file(&path)?;
    let receipt: SnapshotReceipt = read_json(&path)?;
    if receipt.schema != 1
        || receipt.project != project
        || receipt.manifest_sha256 != hash(&json_bytes(&receipt.manifest)?)
        || receipt.manifest_sha256 != hash(&json_bytes(manifest)?)
    {
        return Err(config(
            "old managed manifest differs from trusted committed snapshot; refusing ownership claims",
        ));
    }
    Ok(())
}
fn store_snapshot(project: &Path, manifest: &Manifest) -> Result<()> {
    let path = snapshot_path(project)?;
    if path.exists() {
        verify_private_file(&path)?;
        let old: SnapshotReceipt = read_json(&path)?;
        if old.schema != 1 || old.project != project {
            return Err(config("unknown data at snapshot receipt path"));
        }
    }
    let receipt = SnapshotReceipt {
        schema: 1,
        project: project.to_path_buf(),
        manifest_sha256: hash(&json_bytes(manifest)?),
        manifest: manifest.clone(),
    };
    let temporary = path.with_file_name(format!("snapshot-{}.tmp", unique()));
    create_receipt(&temporary, &receipt)?;
    fs::rename(&temporary, &path)?;
    #[cfg(unix)]
    fs::File::open(path.parent().required("missing snapshot receipt parent")?)?.sync_all()?;
    Ok(())
}

pub(crate) fn snapshot_known(project: &Path, manifest: &Manifest) -> Result<bool> {
    if !snapshot_path(project)?.exists() {
        return Ok(false);
    }
    verify_snapshot(project, manifest)?;
    Ok(true)
}
/// Accept a cold-cache snapshot only after caller reconstruction from trusted
/// sources.
pub(crate) fn establish_snapshot(project: &Path, old: &Manifest, authority: &Manifest) -> Result<()> {
    authorized_manifest(authority)?;
    if hash(&json_bytes(old)?) != hash(&json_bytes(authority)?) {
        return Err(config("old managed records do not match immutable upstream authority"));
    }
    store_snapshot(project, authority)
}

pub(crate) fn pending(project: &Path) -> Result<bool> {
    Ok(within(project, DIRECTORY)?.exists())
}
fn receipt_directory(project: &Path) -> Result<PathBuf> {
    let configured = crate::cache::root()?;
    let root = if configured.starts_with(project) {
        let base = if cfg!(windows) {
            std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
        } else {
            std::env::var_os("XDG_CACHE_HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".cache")))
        }
        .required("no external user directory for transaction receipts")?;
        let fallback = base.join("qubit/cpp-infra");
        fs::create_dir_all(&fallback)?;
        fallback.canonicalize()?
    } else {
        configured
    };
    if root.starts_with(project) {
        return Err(config("transaction trust receipts must be outside the project"));
    }
    let directory = within(&root, "transaction-receipts")?;
    fs::create_dir_all(&directory)?;
    if directory.canonicalize()?.starts_with(project) {
        return Err(config("receipt directory resolves inside project"));
    }
    Ok(directory)
}
fn create_receipt(path: &Path, receipt: &impl Serialize) -> Result<()> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(&json_bytes(receipt)?)?;
    file.sync_all()?;
    #[cfg(unix)]
    fs::File::open(path.parent().required("missing receipt parent")?)?.sync_all()?;
    Ok(())
}
fn authorized_manifest(manifest: &Manifest) -> Result<()> {
    if manifest.schema != 1 {
        return Err(config("unsupported transaction manifest schema"));
    }
    let mut seen = BTreeSet::new();
    for file in &manifest.files {
        crate::snapshot::validate_target(&file.target)?;
        crate::util::valid_digest(&file.sha256)?;
        if !seen.insert(&file.target) {
            return Err(config("duplicate transaction managed target"));
        }
        let owner = file
            .source
            .split(':')
            .next()
            .unwrap_or("")
            .split('@')
            .next()
            .unwrap_or("");
        let role = owner
            .strip_prefix("cpp-infra-")
            .required("missing managed source owner")?;
        let known = [
            "tools",
            "ci",
            "verify",
            "style",
            "analyze",
            "dependency",
            "coverage",
            "pages",
        ]
        .contains(&role);
        let policy = file.target.starts_with(&format!(".infra/{role}/policy/"));
        let bootstrap = role == "tools"
            && (crate::bootstrap::local_files().contains(&file.target.as_str())
                || file.target.starts_with(".infra/tools/schemas/"));
        if !known || (!policy && !bootstrap) {
            return Err(config(format!(
                "unauthorized managed transaction target: {}",
                file.target
            )));
        }
    }
    Ok(())
}
fn initial_target(path: &str) -> bool {
    [".infra/project.toml", "CMakePresets.json"].contains(&path)
        || ["ci", "verify", "style", "analyze", "dependency", "coverage", "pages"]
            .iter()
            .any(|role| path == format!(".infra/{role}/config.toml"))
        || path.starts_with(".infra/dependency/profiles/")
}
fn metadata_target(path: &str) -> bool {
    [
        crate::snapshot::MANIFEST,
        crate::snapshot::LOCK,
        ".infra/tools/main-snapshot.json",
    ]
    .contains(&path)
}
fn validate_journal(journal: &Journal, project: &Path) -> Result<()> {
    if journal.schema != 2
        || journal.project != project
        || safe_relative(&journal.transaction_id)?.components().count() != 1
    {
        return Err(config("invalid transaction journal identity"));
    }
    if let Some(old) = &journal.old_manifest {
        authorized_manifest(old)?;
    }
    authorized_manifest(&journal.new_manifest)?;
    let mut seen = BTreeSet::new();
    for (index, entry) in journal.entries.iter().enumerate() {
        safe_relative(&entry.target)?;
        if !seen.insert(&entry.target)
            || entry.backup != format!("backup/{index}")
            || entry.existed != entry.old_sha256.is_some()
        {
            return Err(config("invalid transaction entry identity"));
        }
        for digest in entry.old_sha256.iter().chain(&entry.new_sha256) {
            crate::util::valid_digest(digest)?;
        }
        let old = journal
            .old_manifest
            .as_ref()
            .and_then(|m| m.files.iter().find(|f| f.target == entry.target));
        let new = journal.new_manifest.files.iter().find(|f| f.target == entry.target);
        if let Some(old) = old
            && entry.old_sha256.as_deref() != Some(old.sha256.as_str())
        {
            return Err(config("old managed digest does not match journal"));
        }
        if let Some(new) = new
            && entry.new_sha256.as_deref() != Some(new.sha256.as_str())
        {
            return Err(config("new managed digest does not match journal"));
        }
        if old.is_none()
            && new.is_none()
            && !metadata_target(&entry.target)
            && (!journal.initial_files.contains(&entry.target)
                || !initial_target(&entry.target)
                || entry.existed
                || entry.new_sha256.is_none())
        {
            return Err(config(format!(
                "journal cannot authorize project-owned target: {}",
                entry.target
            )));
        }
    }
    Ok(())
}
fn current(path: &Path) -> Result<Option<(String, bool)>> {
    if !path.exists() {
        return Ok(None);
    }
    if !fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(config("transaction target is not a regular file"));
    }
    Ok(Some((hash(&fs::read(path)?), is_executable(path)?)))
}
fn matches(state: &Option<(String, bool)>, digest: &Option<String>, mode: bool) -> bool {
    match (state, digest) {
        (None, None) => true,
        (Some((actual, actual_mode)), Some(expected)) => actual == expected && (!cfg!(unix) || *actual_mode == mode),
        _ => false,
    }
}

/// Restore only an independently authorized transaction after validating every
/// file.
pub(crate) fn recover(project: &Path) -> Result<()> {
    let directory = within(project, DIRECTORY)?;
    if !directory.exists() {
        return Ok(());
    }
    let owner: Owner = read_json(&within(&directory, "owner.json")?)
        .configuration("untrusted transaction owner; preserve files and inspect the journal")?;
    if owner.schema != 2
        || owner.owner != "cpp-infra-tools"
        || safe_relative(&owner.transaction_id)?.components().count() != 1
    {
        return Err(config("untrusted transaction owner"));
    }
    let expected_receipt = receipt_directory(project)?.join(format!("{}.json", owner.transaction_id));
    if owner.receipt != expected_receipt {
        return Err(config("transaction receipt is not in the external trust directory"));
    }
    let receipt_path = within(
        expected_receipt.parent().required("missing receipt parent")?,
        expected_receipt
            .file_name()
            .and_then(|v| v.to_str())
            .required("invalid receipt name")?,
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if fs::metadata(&receipt_path)
            .configuration("missing trusted transaction receipt")?
            .permissions()
            .mode()
            & 0o077
            != 0
        {
            return Err(config("transaction receipt permissions must be 0600"));
        }
    }
    let receipt: Receipt = read_json(&receipt_path).configuration("missing or invalid trusted transaction receipt")?;
    if receipt.schema != 1 || receipt.project != project || receipt.transaction_id != owner.transaction_id {
        return Err(config("transaction receipt identity mismatch"));
    }
    let bytes = json_bytes(&receipt.journal)?;
    verify_hash(&bytes, &receipt.journal_sha256)?;
    let journal_path = within(&directory, "journal.json")?;
    if journal_path.exists() {
        verify_hash(&fs::read(&journal_path)?, &receipt.journal_sha256)
            .configuration("project journal differs from trusted receipt")?;
    }
    let journal = &receipt.journal;
    validate_journal(journal, project)?;
    let commit = within(&directory, "commit.json")?;
    if commit.exists() {
        let value: serde_json::Value = read_json(&commit)?;
        if value != json!({"schema":1,"journal_sha256":receipt.journal_sha256}) {
            return Err(config("invalid transaction commit marker"));
        }
    }
    let trusted_commit = receipt_directory(project)?.join(format!("{}.commit.json", owner.transaction_id));
    let committed = if trusted_commit.exists() {
        verify_private_file(&trusted_commit)?;
        let value: serde_json::Value = read_json(&trusted_commit)?;
        if value
            != json!({"schema":1,"project":project,"transaction_id":owner.transaction_id,"journal_sha256":receipt.journal_sha256})
        {
            return Err(config("invalid trusted commit receipt"));
        }
        true
    } else {
        false
    };
    let allowed: BTreeSet<String> = [
        "owner.json".to_owned(),
        "journal.json".to_owned(),
        "commit.json".to_owned(),
    ]
    .into_iter()
    .chain(journal.entries.iter().map(|e| e.backup.clone()))
    .collect();
    for item in WalkDir::new(&directory).follow_links(false) {
        let item = item.configuration_display()?;
        let relative = item
            .path()
            .strip_prefix(&directory)
            .configuration_display()?
            .to_str()
            .required("non-UTF8 transaction path")?;
        if item.file_type().is_dir() {
            if relative.is_empty() || relative == "backup" {
                continue;
            }
            return Err(config("unknown transaction staging directory"));
        }
        if !item.file_type().is_file() || !allowed.contains(relative) {
            return Err(config("unknown transaction staging contents; preserving conflict"));
        }
    }
    let mut states = Vec::new();
    for entry in &journal.entries {
        let target = within(project, &entry.target)?;
        let state = current(&target)?;
        let old = matches(&state, &entry.old_sha256, entry.executable);
        let new = matches(&state, &entry.new_sha256, entry.new_executable);
        if (committed || !old) && !new {
            return Err(config(format!(
                "transaction target has local changes: {}",
                entry.target
            )));
        }
        let backup = within(&directory, &entry.backup)?;
        if backup.exists() {
            let digest = entry.old_sha256.as_deref().required("unexpected backup for new file")?;
            verify_hash(&fs::read(&backup)?, digest).configuration("transaction backup SHA conflict")?;
        } else if !old && entry.existed {
            return Err(config("required transaction backup is missing"));
        }
        states.push((entry, target, old));
    }
    if !committed {
        for (entry, target, old) in states.into_iter().rev() {
            if old {
                continue;
            }
            if entry.existed {
                atomic_write_mode(
                    &target,
                    &fs::read(within(&directory, &entry.backup)?)?,
                    entry.executable,
                )?;
            } else {
                fs::remove_file(target)?;
            }
        }
    }
    if committed {
        store_snapshot(project, &journal.new_manifest)?;
    }
    fs::remove_dir_all(&directory)?;
    fs::remove_file(receipt_path)?;
    if trusted_commit.exists() {
        fs::remove_file(trusted_commit)?;
    }
    Ok(())
}

pub(crate) fn apply(project: &Path, files: &BTreeMap<String, (Vec<u8>, bool)>, retired: &[String]) -> Result<()> {
    recover(project)?;
    let new_manifest: Manifest = crate::util::parse_json(
        &files
            .get(crate::snapshot::MANIFEST)
            .required("update has no new managed manifest")?
            .0,
        "new manifest",
    )?;
    let old_manifest = if project.join(crate::snapshot::MANIFEST).is_file() {
        let manifest = crate::snapshot::manifest(project)?;
        verify_snapshot(project, &manifest)?;
        Some(manifest)
    } else {
        None
    };
    let mut targets: Vec<_> = files.keys().cloned().chain(retired.iter().cloned()).collect();
    targets.sort_by_key(|name| {
        if name == crate::snapshot::MANIFEST {
            2
        } else if metadata_target(name) {
            1
        } else {
            0
        }
    });
    targets.dedup();
    let mut entries = Vec::new();
    let mut backups = Vec::new();
    let mut initial_files = Vec::new();
    for (index, name) in targets.iter().enumerate() {
        let path = within(project, name)?;
        let old = if path.exists() { Some(fs::read(&path)?) } else { None };
        let mode = if old.is_some() { is_executable(&path)? } else { false };
        if !new_manifest.files.iter().any(|f| &f.target == name) && !metadata_target(name) && !retired.contains(name) {
            initial_files.push(name.clone());
        }
        entries.push(Entry {
            target: name.clone(),
            existed: old.is_some(),
            executable: mode,
            new_executable: files.get(name).is_some_and(|(_, mode)| *mode),
            backup: format!("backup/{index}"),
            old_sha256: old.as_ref().map(|b| hash(b)),
            new_sha256: files.get(name).map(|(bytes, _)| hash(bytes)),
        });
        backups.push(old);
    }
    let transaction_id = unique();
    let journal = Journal {
        schema: 2,
        transaction_id: transaction_id.clone(),
        project: project.to_path_buf(),
        old_manifest,
        new_manifest,
        initial_files,
        entries,
    };
    validate_journal(&journal, project)?;
    let bytes = json_bytes(&journal)?;
    let digest = hash(&bytes);
    let receipt_path = receipt_directory(project)?.join(format!("{transaction_id}.json"));
    let receipt = Receipt {
        schema: 1,
        project: project.to_path_buf(),
        transaction_id: transaction_id.clone(),
        journal_sha256: digest.clone(),
        journal: journal.clone(),
    };
    create_receipt(&receipt_path, &receipt)?;
    let directory = within(project, DIRECTORY)?;
    fs::create_dir_all(&directory)?;
    atomic_write(
        &directory.join("owner.json"),
        &json_bytes(&Owner {
            schema: 2,
            owner: "cpp-infra-tools".to_owned(),
            transaction_id,
            receipt: receipt_path,
        })?,
    )?;
    atomic_write(&directory.join("journal.json"), &bytes)?;
    fs::create_dir_all(directory.join("backup"))?;
    for (entry, backup) in journal.entries.iter().zip(backups) {
        if let Some(bytes) = backup {
            atomic_write(&directory.join(&entry.backup), &bytes)?;
        }
    }
    let outcome = (|| -> Result<()> {
        for entry in &journal.entries {
            crate::util::cancelled()?;
            let path = within(project, &entry.target)?;
            if let Some((bytes, mode)) = files.get(&entry.target) {
                atomic_write_mode(&path, bytes, *mode)?;
            } else if path.exists() {
                fs::remove_file(path)?;
            }
        }
        crate::snapshot::check_managed(project)?;
        create_receipt(
            &receipt_directory(project)?.join(format!("{}.commit.json", journal.transaction_id)),
            &json!({"schema":1,"project":project,"transaction_id":journal.transaction_id,"journal_sha256":digest}),
        )?;
        atomic_write(
            &directory.join("commit.json"),
            &json_bytes(&json!({"schema":1,"journal_sha256":digest}))?,
        )?;
        Ok(())
    })();
    recover(project)?;
    outcome
}
