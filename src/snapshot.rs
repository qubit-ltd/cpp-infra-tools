// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Local snapshot integrity and strict project configuration.
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::Deserialize;

use crate::catalog::Catalog;
use crate::error_context::ErrorContext;
use crate::manifest::Manifest;
use crate::tools_error::Result;
use crate::tools_error::config;
use crate::util::is_executable;
use crate::util::read_json;
use crate::util::verify_hash;
use crate::util::within;
pub(crate) const LOCK: &str = ".infra/tools/tools.lock.json";
pub(crate) const MANIFEST: &str = ".infra/managed-files.json";
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectConfig {
    schema: u32,
    kind: String,
    #[serde(default)]
    source_roots: Vec<String>,
    #[serde(default)]
    test_roots: Vec<String>,
    #[serde(default)]
    generated_roots: Vec<String>,
    #[serde(default)]
    third_party_roots: Vec<String>,
    #[serde(default)]
    profiles: BTreeMap<String, Profile>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    configure: String,
    build: String,
    #[serde(default)]
    test: Option<String>,
    #[serde(default)]
    conan_context: Option<String>,
}
pub(crate) fn load(project: &Path) -> Result<Catalog> {
    let lock_path = within(project, LOCK)?;
    if !lock_path.is_file() {
        return Err(config("missing .infra/tools/tools.lock.json; run init first"));
    }
    let catalog: Catalog = read_json(&lock_path)?;
    catalog.validate()?;
    if catalog.catalog_url.is_none() || catalog.catalog_sha256.is_none() {
        return Err(config("snapshot is missing catalog provenance"));
    }
    if let Some(hash) = &catalog.catalog_sha256 {
        crate::util::valid_digest(hash)?;
    }
    Ok(catalog)
}
pub(crate) fn manifest(project: &Path) -> Result<Manifest> {
    let path = within(project, MANIFEST)?;
    if !path.is_file() {
        return Err(config("missing managed snapshot; run init first"));
    }
    let manifest: Manifest = read_json(&path)?;
    if manifest.schema != 1 {
        return Err(config("unsupported managed-files schema"));
    }
    let mut targets = std::collections::BTreeSet::new();
    for entry in &manifest.files {
        validate_target(&entry.target)?;
        if !targets.insert(&entry.target) {
            return Err(config("duplicate managed target"));
        }
    }
    Ok(manifest)
}
pub(crate) fn validate_target(target: &str) -> Result<()> {
    crate::util::safe_relative(target)?;
    if target == "update-infra.sh" {
        return Ok(());
    }
    if !target.starts_with(".infra/")
        || [LOCK, MANIFEST, ".infra/project.toml", ".infra/tools/main-snapshot.json"].contains(&target)
        || target.ends_with("/config.toml")
        || target.starts_with(".infra/transactions/")
        || target.starts_with(".infra/.transactions/")
        || target.starts_with(".infra/.tmp/")
    {
        return Err(config(format!(
            "managed ownership cannot claim project-owned/reserved path: {target}"
        )));
    }
    Ok(())
}
pub(crate) fn check(project: &Path) -> Result<Catalog> {
    let catalog = load(project)?;
    check_managed(project)?;
    Ok(catalog)
}
pub(crate) fn check_managed(project: &Path) -> Result<()> {
    let manifest = manifest(project)?;
    if manifest.files.is_empty() {
        return Err(config("managed snapshot is empty"));
    }
    for entry in manifest.files {
        let path = within(project, &entry.target)?;
        let bytes = fs::read(&path).configuration_context(format_args!("managed file {}", entry.target))?;
        verify_hash(&bytes, &entry.sha256).configuration(format_args!("managed file conflict: {}", entry.target))?;
        #[cfg(unix)]
        if is_executable(&path)? != entry.executable {
            return Err(config(format!("managed executable mode conflict: {}", entry.target)));
        }
    }
    validate_project(project)?;
    Ok(())
}
pub(crate) fn validate_project(project: &Path) -> Result<()> {
    let path = within(project, ".infra/project.toml")?;
    if !path.is_file() {
        return Err(config("missing required .infra/project.toml"));
    }
    validate_project_text(
        project,
        &crate::util::read_configuration_text(&crate::util::tracked_config(project, &path)?)?,
    )?;
    // Every enabled tool owns its full schema; the coordinator validates the common
    // discriminator.
    let infra = within(project, ".infra")?;
    for item in fs::read_dir(infra)? {
        let item = item?;
        if !item.file_type()?.is_dir() {
            continue;
        }
        if [".tmp", "transactions", ".transactions"].contains(&item.file_name().to_str().unwrap_or("")) {
            continue;
        }
        let config_path = item.path().join("config.toml");
        if config_path.is_file() {
            let text = crate::util::read_configuration_text(&crate::util::tracked_config(project, &config_path)?)?;
            let value: toml::Value =
                toml::from_str(&text).configuration_context(format_args!("{}", config_path.display()))?;
            if value.get("schema").and_then(toml::Value::as_integer) != Some(1) {
                return Err(config("tool configuration must use schema = 1"));
            }
        }
    }
    Ok(())
}
pub(crate) fn validate_project_text(project: &Path, text: &str) -> Result<()> {
    let parsed: ProjectConfig = toml::from_str(text).configuration_context(format_args!("project.toml"))?;
    if parsed.schema != 1 || !["library", "application", "header-only"].contains(&parsed.kind.as_str()) {
        return Err(config("unsupported project schema or kind"));
    }
    let mut roots = Vec::new();
    for root in parsed
        .source_roots
        .iter()
        .chain(&parsed.test_roots)
        .chain(&parsed.generated_roots)
        .chain(&parsed.third_party_roots)
    {
        let path = within(project, root)?;
        if !path.is_dir() {
            return Err(config(format!("configured root does not exist: {root}")));
        }
        let canonical = path.canonicalize()?;
        if roots
            .iter()
            .any(|previous: &std::path::PathBuf| previous.starts_with(&canonical) || canonical.starts_with(previous))
        {
            return Err(config(format!("overlapping project root: {root}")));
        }
        roots.push(canonical);
    }
    for (name, profile) in parsed.profiles {
        if name.is_empty()
            || profile.configure.is_empty()
            || profile.build.is_empty()
            || profile.test.as_ref().is_some_and(String::is_empty)
            || profile.conan_context.as_ref().is_some_and(String::is_empty)
        {
            return Err(config("empty profile mapping"));
        }
    }
    Ok(())
}
pub(crate) fn validate_profile(project: &Path, name: &str) -> Result<()> {
    let text = crate::util::read_configuration_text(&crate::util::tracked_config(
        project,
        &project.join(".infra/project.toml"),
    )?)?;
    let parsed: ProjectConfig = toml::from_str(&text).configuration_display()?;
    if parsed.profiles.contains_key(name) {
        Ok(())
    } else {
        Err(config(format!("unknown profile: {name}")))
    }
}

/// Resolve a manifest target inside its owning tool's project directory.
pub(crate) fn owned_target(role: &str, target: &str) -> Result<String> {
    crate::util::safe_relative(target)?;
    if target.starts_with(".infra/") {
        Ok(target.to_owned())
    } else {
        Ok(format!(".infra/{role}/{target}"))
    }
}
