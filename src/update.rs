// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Resolve a stable release and prepare an ownership-checked update
//! transaction.
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::Value;
use serde_json::json;

use crate::archive::unpack;
use crate::bootstrap;
use crate::catalog::Catalog;
use crate::catalog::Channel;
use crate::download::fetch;
use crate::download::verified;
use crate::error_context::ErrorContext;
use crate::manifest::Manifest;
use crate::required::Required;
use crate::snapshot;
use crate::snapshot::LOCK;
use crate::snapshot::MANIFEST;
use crate::tools_error::Result;
use crate::tools_error::config;
use crate::util::hash;
use crate::util::json_bytes;
use crate::util::parse_json;
use crate::util::verify_hash;
use crate::util::within;
const DEFAULT_CHANNEL: &str = "https://raw.githubusercontent.com/qubit-ltd/cpp-infra-tools/main/channels/stable.json";
pub(crate) fn resolve(bundle: Option<&str>, offline: bool) -> Result<Catalog> {
    if let Ok(url) = std::env::var("CPP_INFRA_CATALOG_URL") {
        let bytes = fetch(&url, offline)?;
        let mut catalog: Catalog = parse_json(&bytes, "catalog")?;
        catalog.validate()?;
        if bundle.is_some_and(|value| value != catalog.bundle_id) {
            return Err(config("explicit catalog does not contain the requested bundle"));
        }
        catalog.catalog_url = Some(url);
        catalog.catalog_sha256 = Some(hash(&bytes));
        return Ok(catalog);
    }
    let channel_url = std::env::var("CPP_INFRA_CHANNEL_URL").unwrap_or_else(|_| DEFAULT_CHANNEL.to_owned());
    let channel: Channel = parse_json(&fetch(&channel_url, offline)?, "stable channel")?;
    if channel.schema != 1 {
        return Err(config("unsupported stable channel schema"));
    }
    let selected = bundle.unwrap_or(&channel.bundle_id);
    let reference = if selected == channel.bundle_id {
        &channel.catalog
    } else {
        channel
            .bundles
            .get(selected)
            .required(format_args!("bundle {selected} is not listed in the stable channel"))?
    };
    let bytes = verified(&reference.url, &reference.sha256, offline)?;
    let mut catalog: Catalog = parse_json(&bytes, "catalog")?;
    catalog.validate()?;
    if catalog.bundle_id != selected {
        return Err(config("channel/catalog bundle mismatch"));
    }
    catalog.catalog_url = Some(reference.url.clone());
    catalog.catalog_sha256 = Some(reference.sha256.clone());
    Ok(catalog)
}
pub(crate) fn prepare(project: &Path, catalog: &Catalog, initial: bool, dry_run: bool, offline: bool) -> Result<Value> {
    let old = if project.join(LOCK).exists() {
        Some(snapshot::manifest(project)?)
    } else {
        None
    };
    if initial && old.is_some() {
        return Err(config("project already initialized; use update"));
    }
    if !initial && old.is_none() {
        return Err(config("missing snapshot; use init"));
    }
    if let Some(manifest) = &old {
        for entry in &manifest.files {
            let path = within(project, &entry.target)?;
            let bytes =
                fs::read(&path).configuration_context(format_args!("managed file conflict: {}", entry.target))?;
            verify_hash(&bytes, &entry.sha256)
                .configuration(format_args!("managed file conflict: {}", entry.target))?;
            #[cfg(unix)]
            if crate::util::is_executable(&path)? != entry.executable {
                return Err(config(format!("managed executable mode conflict: {}", entry.target)));
            }
        }
    }
    // Fail before writing if any selected bundle tool has neither an asset nor
    // explicit source path.
    for tool in &catalog.tools {
        if !tool.assets.contains_key(&crate::cache::host()) && tool.source.is_none() {
            return Err(config(format!(
                "bundle {} has no asset for {}",
                catalog.bundle_id, tool.name
            )));
        }
    }
    let (mut files, mut managed) = bootstrap::files(
        project,
        catalog.catalog_url.as_deref().required("missing catalog URL")?,
        catalog.catalog_sha256.as_deref().required("missing catalog digest")?,
    )?;
    if !initial {
        files.remove(".infra/project.toml");
        files.remove("CMakePresets.json");
    }
    let mut seen: BTreeSet<String> = managed.iter().map(|file| file.target.clone()).collect();
    for package in &catalog.packages {
        let archive = unpack(&verified(&package.url, &package.sha256, offline)?, "tar.gz")?;
        let manifest: Manifest = parse_json(
            archive
                .get("manifest.json")
                .required("public package is missing manifest.json")?,
            "public manifest",
        )?;
        if manifest.schema != 1 || manifest.owner.as_ref().is_some_and(|owner| owner != &package.owner) {
            return Err(config("manifest schema/owner mismatch"));
        }
        for mut entry in manifest.files {
            let role = package
                .owner
                .strip_prefix("cpp-infra-")
                .required("invalid package owner")?;
            entry.target = snapshot::owned_target(role, &entry.target)?;
            snapshot::validate_target(&entry.target)?;
            if !entry.target.starts_with(&format!(".infra/{role}/")) {
                return Err(config(format!(
                    "{} claims another tool's ownership: {}",
                    package.owner, entry.target
                )));
            }
            if !seen.insert(entry.target.clone()) {
                return Err(config(format!("manifest ownership conflict: {}", entry.target)));
            }
            crate::util::safe_relative(&entry.source)?;
            let contents = archive
                .get(&entry.source)
                .required(format_args!("missing manifest source: {}", entry.source))?;
            verify_hash(contents, &entry.sha256)?;
            files.insert(entry.target.clone(), (contents.clone(), entry.executable));
            managed.push(crate::managed_file::ManagedFile {
                source: format!("{}:{}", package.owner, entry.source),
                ..entry
            });
        }
        if initial {
            for mut entry in manifest.project_files {
                let role = package
                    .owner
                    .strip_prefix("cpp-infra-")
                    .required("invalid package owner")?;
                entry.target = snapshot::owned_target(role, &entry.target)?;
                if !entry.target.starts_with(&format!(".infra/{role}/"))
                    || entry.target.contains("/policy/")
                    || !seen.insert(entry.target.clone())
                {
                    return Err(config("invalid or duplicate initial project config target"));
                }
                let path = within(project, &entry.target)?;
                if path.exists() {
                    continue;
                }
                crate::util::safe_relative(&entry.source)?;
                let contents = archive
                    .get(&entry.source)
                    .required("missing initial configuration source")?;
                verify_hash(contents, &entry.sha256)?;
                if entry.target.ends_with("/config.toml") {
                    let value: toml::Value = toml::from_str(std::str::from_utf8(contents).configuration_display()?)
                        .configuration_context(format_args!("initial configuration"))?;
                    if value.get("schema").and_then(toml::Value::as_integer) != Some(1) {
                        return Err(config("initial configuration must use schema = 1"));
                    }
                }
                files.insert(entry.target, (contents.clone(), false));
            }
        }
    }
    if let Some((project_text, _)) = files.get(".infra/project.toml") {
        snapshot::validate_project_text(project, std::str::from_utf8(project_text).configuration_display()?)?;
    } else {
        snapshot::validate_project(project)?;
    }
    let old_names: BTreeSet<String> = old
        .as_ref()
        .map(|manifest| manifest.files.iter().map(|f| f.target.clone()).collect())
        .unwrap_or_default();
    let new_names: BTreeSet<String> = managed.iter().map(|file| file.target.clone()).collect();
    for (target, (contents, _)) in &files {
        let path = within(project, target)?;
        if path.exists()
            && !old_names.contains(target)
            && ![".infra/project.toml", "CMakePresets.json"].contains(&target.as_str())
            && fs::read(&path)? != *contents
        {
            return Err(config(format!("unmanaged file conflict: {target}")));
        }
    }
    let retired: Vec<String> = old_names.difference(&new_names).cloned().collect();
    managed.sort_by(|a, b| a.target.cmp(&b.target));
    let manifest = Manifest {
        schema: 1,
        owner: Some("cpp-infra-tools".to_owned()),
        files: managed,
        project_files: Vec::new(),
    };
    files.insert(LOCK.to_owned(), (json_bytes(catalog)?, false));
    files.insert(MANIFEST.to_owned(), (json_bytes(&manifest)?, false));
    let changed: Vec<String> = files
        .iter()
        .filter_map(|(target, (contents, _))| match fs::read(project.join(target)) {
            Ok(previous) if previous == *contents => None,
            _ => Some(target.clone()),
        })
        .collect();
    let plan = json!({"schema":1,"bundle_id":catalog.bundle_id,"dry_run":dry_run,"files":changed,"retired":retired});
    if !dry_run {
        crate::transaction::apply(project, &files, &retired)?;
    }
    Ok(plan)
}
