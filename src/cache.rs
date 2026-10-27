// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Shared, revision-addressed, locked executable cache.
use std::collections::BTreeMap;
use std::fs;
use std::fs::OpenOptions;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;
use serde_json::json;

use crate::archive::unpack;
use crate::catalog::Catalog;
use crate::download::verified;
use crate::locked_tool::LockedTool;
use crate::required::Required;
use crate::runtime::command;
use crate::tools_error::Result;
use crate::tools_error::config;
use crate::tools_error::infra;
use crate::util::atomic_write;
use crate::util::executable;
use crate::util::hash;
use crate::util::json_bytes;
use crate::util::parse_json;
use crate::util::read_json;
use crate::util::unique;
use crate::util::verify_hash;
pub(crate) fn host() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}
fn configured_root() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("CPP_INFRA_CACHE_DIR") {
        return Ok(PathBuf::from(path));
    }
    if cfg!(windows) {
        return std::env::var_os("LOCALAPPDATA")
            .map(|p| PathBuf::from(p).join("qubit/cpp-infra"))
            .required("LOCALAPPDATA is required");
    }
    if let Some(path) = std::env::var_os("XDG_CACHE_HOME") {
        return Ok(PathBuf::from(path).join("qubit/cpp-infra"));
    }
    std::env::var_os("HOME")
        .map(|p| PathBuf::from(p).join(".cache/qubit/cpp-infra"))
        .required("HOME or XDG_CACHE_HOME is required")
}
pub(crate) fn root() -> Result<PathBuf> {
    let directory = configured_root()?;
    fs::create_dir_all(&directory)?;
    Ok(directory.canonicalize()?)
}
pub(crate) fn prepare(
    tool: &LockedTool,
    offline: bool,
    from_source: bool,
    records: &mut Vec<Value>,
) -> Result<PathBuf> {
    let asset = tool.assets.get(&host());
    if asset.is_none() && !from_source {
        return Err(config(format!(
            "no platform asset for {} on {}; use --from-source explicitly",
            tool.name,
            host()
        )));
    }
    if asset.is_none() {
        return prepare_source(tool, offline, records);
    }
    let asset = asset.required("missing asset")?;
    let directory = root()?
        .join(&tool.name)
        .join(&tool.revision)
        .join(host())
        .join(&asset.sha256);
    fs::create_dir_all(&directory)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join(".lock"))?;
    crate::util::acquire_lock(&lock, true)?;
    let binary = directory.join(&asset.binary);
    if binary.exists() {
        verify_hash(&fs::read(&binary)?, &asset.executable_sha256)?;
        verify_version(tool, &binary, &directory, records)?;
        return Ok(binary);
    }
    if offline {
        return Err(infra(format!("offline: cache missing for {}", tool.name)));
    }
    let bytes = verified(&asset.url, &asset.sha256, false)?;
    let executable_bytes = if asset.format == "raw" {
        bytes
    } else {
        let files = unpack(&bytes, &asset.format)?;
        let matches: Vec<_> = files
            .iter()
            .filter(|(path, _)| Path::new(path).file_name().is_some_and(|n| n == asset.binary.as_str()))
            .collect();
        if matches.len() != 1 {
            return Err(config("archive must contain exactly one matching executable"));
        }
        let (_, bytes) = matches[0];
        bytes.clone()
    };
    verify_hash(&executable_bytes, &asset.executable_sha256)?;
    let staged = directory.join(format!(".candidate-{}", unique()));
    atomic_write(&staged, &executable_bytes)?;
    executable(&staged, true)?;
    if let Err(error) = verify_version(tool, &staged, &directory, records) {
        let _ = fs::remove_file(&staged);
        return Err(error);
    }
    fs::rename(&staged, &binary)?;
    atomic_write(
        &directory.join("verified.json"),
        &json_bytes(&json!({"schema":1,"revision":tool.revision,"sha256":asset.executable_sha256,"host":host()}))?,
    )?;
    Ok(binary)
}
fn verify_version(tool: &LockedTool, binary: &Path, logs: &Path, records: &mut Vec<Value>) -> Result<()> {
    let (status, stdout, stderr) = command(
        binary,
        &["--version".to_owned(), "--json".to_owned()],
        logs,
        &BTreeMap::new(),
        logs,
        Duration::from_secs(15),
        records,
    )?;
    if status != 0 {
        return Err(config(format!(
            "version query failed for {}: {}",
            tool.name,
            String::from_utf8_lossy(&stderr)
        )));
    }
    let version: Value = parse_json(&stdout, "tool version")?;
    if version["name"] != tool.name
        || version["version"] != tool.version
        || version["revision"] != tool.revision
        || version["protocol"] != tool.protocol
    {
        return Err(config(format!(
            "tool version/revision/protocol mismatch: {}",
            tool.name
        )));
    }
    Ok(())
}
fn prepare_source(tool: &LockedTool, offline: bool, records: &mut Vec<Value>) -> Result<PathBuf> {
    let source = tool
        .source
        .as_ref()
        .required("catalog does not provide a pinned source archive")?;
    let directory = root()?
        .join(&tool.name)
        .join(&tool.revision)
        .join(host())
        .join(format!("source-{}", source.sha256));
    fs::create_dir_all(&directory)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join(".lock"))?;
    crate::util::acquire_lock(&lock, true)?;
    let binary = directory.join(if cfg!(windows) {
        format!("{}.exe", tool.name)
    } else {
        tool.name.clone()
    });
    if binary.exists() {
        let marker: Value = read_json(&directory.join("source-build.json"))?;
        let expected = source
            .executable_sha256
            .as_deref()
            .or_else(|| marker["executable_sha256"].as_str())
            .required("source cache is missing executable digest")?;
        verify_hash(&fs::read(&binary)?, expected)?;
        if marker["revision"] != tool.revision || marker["cargo_lock_sha256"] != source.cargo_lock_sha256 {
            return Err(config("source build cache identity mismatch"));
        }
        verify_version(tool, &binary, &directory, records)?;
        return Ok(binary);
    }
    if offline {
        return Err(infra("offline: pinned source build cache missing"));
    }
    let files = unpack(&verified(&source.url, &source.sha256, false)?, "tar.gz")?;
    let checkout = directory.join("source");
    fs::create_dir_all(&checkout)?;
    for (name, bytes) in files {
        let target = crate::util::within(&checkout, &name)?;
        atomic_write(&target, &bytes)?;
    }
    verify_hash(&fs::read(checkout.join("Cargo.lock"))?, &source.cargo_lock_sha256)?;
    let mut environment = BTreeMap::new();
    environment.insert("CPP_INFRA_BUILD_REVISION".to_owned(), tool.revision.clone());
    environment.insert(
        "CARGO_TARGET_DIR".to_owned(),
        directory.join("target").to_string_lossy().into_owned(),
    );
    let (code, _, stderr) = crate::runtime::command_network(
        Path::new("cargo"),
        &[
            "build".to_owned(),
            "--release".to_owned(),
            "--locked".to_owned(),
            "--bin".to_owned(),
            tool.name.clone(),
        ],
        &checkout,
        &environment,
        &directory,
        Duration::from_secs(1800),
        records,
    )?;
    if code != 0 {
        return Err(crate::ToolsError::Gate(format!(
            "pinned source build failed: {}",
            String::from_utf8_lossy(&stderr)
        )));
    }
    let built = directory
        .join("target/release")
        .join(binary.file_name().required("invalid binary name")?);
    let bytes = fs::read(built)?;
    if let Some(expected) = &source.executable_sha256 {
        verify_hash(&bytes, expected)?;
    }
    atomic_write(&binary, &bytes)?;
    executable(&binary, true)?;
    verify_version(tool, &binary, &directory, records)?;
    let (_, rustc, _) = command(
        Path::new("rustc"),
        &["--version".to_owned(), "--verbose".to_owned()],
        &checkout,
        &BTreeMap::new(),
        &directory,
        Duration::from_secs(15),
        records,
    )?;
    atomic_write(
        &directory.join("source-build.json"),
        &json_bytes(
            &json!({"schema":1,"revision":tool.revision,"host":host(),"cargo_lock_sha256":source.cargo_lock_sha256,"rustc":String::from_utf8_lossy(&rustc),"executable_sha256":hash(&bytes)}),
        )?,
    )?;
    Ok(binary)
}
pub(crate) fn bundle_bin(catalog: &Catalog, binaries: &[(String, PathBuf)]) -> Result<PathBuf> {
    let identity = hash(&json_bytes(catalog)?);
    let directory = root()?.join("bundles").join(identity).join("bin");
    fs::create_dir_all(&directory)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join(".lock"))?;
    crate::util::acquire_lock(&lock, true)?;
    for (name, path) in binaries {
        let target = directory.join(if cfg!(windows) {
            format!("{name}.exe")
        } else {
            name.clone()
        });
        let bytes = fs::read(path)?;
        if !target.exists() || fs::read(&target)? != bytes {
            atomic_write(&target, &bytes)?;
            executable(&target, true)?;
        }
    }
    Ok(directory)
}
