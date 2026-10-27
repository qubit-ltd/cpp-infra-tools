// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! C++ template delivery, isolated from this Rust repository's own
//! infrastructure.
use std::collections::BTreeMap;
use std::path::Path;

use serde_json::json;

use crate::error_context::ErrorContext;
use crate::managed_file::ManagedFile;
use crate::required::Required;
use crate::tools_error::Result;
use crate::util::hash;
use crate::util::json_bytes;
pub(crate) fn local_files() -> Vec<&'static str> {
    vec![
        "update-infra.sh",
        ".infra/bootstrap.py",
        ".infra/bootstrap-source.json",
        ".infra/bin/ci-check.sh",
        ".infra/bin/style-check.sh",
        ".infra/bin/align-ci.sh",
        ".infra/bin/coverage.sh",
        ".infra/cmake/presets.json",
        ".infra/tools/defaults.toml",
        ".infra/project.toml",
        "CMakePresets.json",
    ]
}
type FileSet = BTreeMap<String, (Vec<u8>, bool)>;
pub(crate) fn files(project: &Path, catalog_url: &str, catalog_digest: &str) -> Result<(FileSet, Vec<ManagedFile>)> {
    let mut files = BTreeMap::new();
    files.insert(
        "update-infra.sh".to_owned(),
        (include_bytes!("../assets/cpp-project/update-infra.sh").to_vec(), true),
    );
    files.insert(
        ".infra/bootstrap.py".to_owned(),
        (include_bytes!("../assets/cpp-project/bootstrap.py").to_vec(), true),
    );
    files.insert(
        ".infra/bootstrap-source.json".to_owned(),
        (
            json_bytes(&json!({"schema":1,"catalog_url":catalog_url,"catalog_sha256":catalog_digest,"protocol":1}))?,
            false,
        ),
    );
    for (name, bytes) in [
        (
            "ci-check.sh",
            include_bytes!("../assets/cpp-project/bin/ci-check.sh").as_slice(),
        ),
        (
            "style-check.sh",
            include_bytes!("../assets/cpp-project/bin/style-check.sh").as_slice(),
        ),
        (
            "align-ci.sh",
            include_bytes!("../assets/cpp-project/bin/align-ci.sh").as_slice(),
        ),
        (
            "coverage.sh",
            include_bytes!("../assets/cpp-project/bin/coverage.sh").as_slice(),
        ),
    ] {
        files.insert(format!(".infra/bin/{name}"), (bytes.to_vec(), true));
    }
    files.insert(
        ".infra/cmake/presets.json".to_owned(),
        (include_bytes!("../assets/cpp-project/presets.json").to_vec(), false),
    );
    files.insert(
        ".infra/tools/defaults.toml".to_owned(),
        (include_bytes!("../conf/defaults.toml").to_vec(), false),
    );
    files.insert(
        ".infra/tools/schemas/catalog.schema.json".to_owned(),
        (include_bytes!("../schemas/catalog.schema.json").to_vec(), false),
    );
    files.insert(
        ".infra/tools/schemas/channel.schema.json".to_owned(),
        (include_bytes!("../schemas/channel.schema.json").to_vec(), false),
    );
    files.insert(
        ".infra/tools/schemas/manifest.schema.json".to_owned(),
        (include_bytes!("../schemas/manifest.schema.json").to_vec(), false),
    );
    files.insert(
        ".infra/tools/schemas/report.schema.json".to_owned(),
        (include_bytes!("../schemas/report.schema.json").to_vec(), false),
    );
    files.insert(
        ".infra/tools/schemas/revisions.schema.json".to_owned(),
        (include_bytes!("../schemas/revisions.schema.json").to_vec(), false),
    );
    files.insert(
        ".infra/tools/schemas/run-context.schema.json".to_owned(),
        (include_bytes!("../schemas/run-context.schema.json").to_vec(), false),
    );
    files.insert(
        ".infra/tools/schemas/runtime-snapshot.schema.json".to_owned(),
        (
            include_bytes!("../schemas/runtime-snapshot.schema.json").to_vec(),
            false,
        ),
    );
    files.insert(
        ".infra/tools/schemas/tools-lock.schema.json".to_owned(),
        (include_bytes!("../schemas/tools-lock.schema.json").to_vec(), false),
    );
    let managed = files
        .iter()
        .map(|(target, (bytes, mode))| ManagedFile {
            target: target.clone(),
            source: format!("cpp-infra-tools:{target}"),
            sha256: hash(bytes),
            executable: *mode,
        })
        .collect();
    if !project.join(".infra/project.toml").exists() {
        let roots: Vec<_> = ["src", "include"]
            .iter()
            .filter(|name| project.join(name).is_dir())
            .copied()
            .collect();
        let tests: Vec<_> = ["tests"]
            .iter()
            .filter(|name| project.join(name).is_dir())
            .copied()
            .collect();
        let config = format!(
            "schema = 1\nkind = \"library\"\nsource_roots = {roots:?}\ntest_roots = {tests:?}\ngenerated_roots = []\nthird_party_roots = []\n\n[profiles.debug]\nconfigure = \"infra-debug\"\nbuild = \"infra-debug\"\ntest = \"infra-debug\"\n"
        );
        files.insert(".infra/project.toml".to_owned(), (config.into_bytes(), false));
    }
    if !project.join("CMakePresets.json").exists() {
        files.insert(
            "CMakePresets.json".to_owned(),
            (
                b"{\"version\":6,\"include\":[\".infra/cmake/presets.json\"]}\n".to_vec(),
                false,
            ),
        );
    }
    Ok((files, managed))
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct AssetManifest {
    schema: u32,
    files: Vec<ManagedFile>,
    templates: Vec<ManagedFile>,
}
/// Reconstruct exact delivery bytes from an immutable manager Git source.
pub(crate) fn source_assets(
    source: &Path,
    repository: &str,
    revision: &str,
    main: bool,
    catalog_digest: Option<&str>,
) -> Result<(FileSet, Vec<ManagedFile>)> {
    let definition: AssetManifest =
        crate::util::read_json(&crate::util::within(source, "assets/cpp-project/manifest.json")?)?;
    if definition.schema != 1 {
        return Err(crate::tools_error::config("unsupported manager asset manifest schema"));
    }
    let mut files = FileSet::new();
    let mut managed = Vec::new();
    for (entry, template) in definition
        .files
        .into_iter()
        .map(|e| (e, false))
        .chain(definition.templates.into_iter().map(|e| (e, true)))
    {
        crate::snapshot::validate_target(&entry.target)?;
        if files.contains_key(&entry.target) {
            return Err(crate::tools_error::config("duplicate manager asset target"));
        }
        let mut bytes = std::fs::read(crate::util::within(source, &entry.source)?)?;
        crate::util::verify_hash(&bytes, &entry.sha256)?;
        if template {
            if entry.target != ".infra/bootstrap-source.json" {
                return Err(crate::tools_error::config("unsupported dynamic manager asset"));
            }
            bytes = if main {
                let text = std::str::from_utf8(&bytes).configuration_display()?;
                text.replace(
                    "@CPP_INFRA_REPOSITORY_JSON@",
                    &serde_json::to_string(repository).configuration_display()?,
                )
                .replace(
                    "@CPP_INFRA_REVISION_JSON@",
                    &serde_json::to_string(revision).configuration_display()?,
                )
                .into_bytes()
            } else {
                json_bytes(
                    &json!({"schema":1,"catalog_url":repository,"catalog_sha256":catalog_digest.required("missing old catalog digest")?,"protocol":1}),
                )?
            };
        }
        let citation = if main {
            format!("cpp-infra-tools@{revision}:{}", entry.target)
        } else {
            format!("cpp-infra-tools:{}", entry.target)
        };
        managed.push(ManagedFile {
            target: entry.target.clone(),
            source: citation,
            sha256: hash(&bytes),
            executable: entry.executable,
        });
        files.insert(entry.target, (bytes, entry.executable));
    }
    Ok((files, managed))
}
/// Add only missing project-owned initialization files to source-derived
/// assets.
pub(crate) fn main_files(
    project: &Path,
    source: &Path,
    repository: &str,
    revision: &str,
) -> Result<(FileSet, Vec<ManagedFile>)> {
    let (mut result, managed) = source_assets(source, repository, revision, true, None)?;
    let (mut initial, _) = files(project, repository, revision)?;
    for target in [".infra/project.toml", "CMakePresets.json"] {
        if let Some(value) = initial.remove(target) {
            result.insert(target.to_owned(), value);
        }
    }
    Ok((result, managed))
}
