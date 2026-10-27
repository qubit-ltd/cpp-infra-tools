// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Track main explicitly while fixing immutable revisions for each invocation.
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;
use std::fs::OpenOptions;
use std::io;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use serde_json::json;

use crate::catalog::Catalog;
use crate::catalog::ExternalTool;
use crate::cli::Action;
use crate::cli::Cli;
use crate::error_context::ErrorContext;
use crate::manifest::Manifest;
use crate::report::Report;
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
use crate::util::safe_relative;
use crate::util::verify_hash;
use crate::util::within;
const ROLES: &[&str] = &[
    "tools",
    "ci",
    "verify",
    "style",
    "analyze",
    "dependency",
    "coverage",
    "pages",
];
const MAIN_SNAPSHOT: &str = ".infra/tools/main-snapshot.json";
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Revisions {
    schema: u32,
    tools: Vec<Revision>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Revision {
    name: String,
    repository: String,
    revision: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MainTool {
    name: String,
    repository: String,
    revision: String,
    version: String,
    protocol: u32,
    binary: Option<PathBuf>,
    executable_sha256: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MainSnapshot {
    schema: u32,
    bundle_id: String,
    tools: Vec<MainTool>,
    revisions: Revisions,
    offline: bool,
    stale: bool,
    host: String,
    rustc: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Defaults {
    schema: u32,
    external_tools: BTreeMap<String, ExternalTool>,
}
pub(crate) fn dispatch(cli: &Cli, project: &Path, report: &mut Report, action: &Action) -> Result<()> {
    match action {
        Action::Init { dry_run } => {
            crate::application::require_cpp(project)?;
            let names = all_names();
            let refs = resolve(&names, cli.offline, report)?;
            let sources = checkouts(&refs, &names, cli.offline, report)?;
            report.bundle_id = bundle_id(&refs)?;
            report.data = update(project, &refs, &sources, true, *dry_run, cli.offline)?;
            println!("{}", report.data);
        }
        Action::Update { dry_run, bundle } => {
            if bundle.is_some() {
                return Err(config("--bundle requires explicit --locked mode"));
            }
            crate::snapshot::check_managed(project)?;
            ensure_previous_authority(project, cli.offline, report)?;
            let names = all_names();
            let refs = resolve(&names, cli.offline, report)?;
            let sources = checkouts(&refs, &names, cli.offline, report)?;
            report.bundle_id = bundle_id(&refs)?;
            report.data = update(project, &refs, &sources, false, *dry_run, cli.offline)?;
            println!("{}", report.data);
        }
        Action::Check => {
            crate::snapshot::check_managed(project)?;
            let snapshot: MainSnapshot = read_main_snapshot(project)?;
            validate_refs(&snapshot.revisions)?;
            if snapshot.schema != 1 || snapshot.bundle_id != bundle_id(&snapshot.revisions)? {
                return Err(config("main snapshot identity mismatch"));
            }
            if cli.run_context.is_none() {
                report.bundle_id = snapshot.bundle_id;
            }
            report.data = json!({"valid":true,"mode":"main","network":false});
            println!("{}", report.data);
        }
        Action::Status => {
            crate::snapshot::check_managed(project)?;
            let current: MainSnapshot = read_main_snapshot(project)?;
            let latest = resolve(&all_names(), cli.offline, report)?;
            report.bundle_id = current.bundle_id.clone();
            report.data = json!({"mode":"main","current":current.bundle_id,"latest":bundle_id(&latest)?,"update_available":current.bundle_id!=bundle_id(&latest)?,"offline":cli.offline,"stale":cli.offline});
            println!("{}", report.data);
        }
        Action::Resolve { tool, output } => {
            let names = if tool.is_empty() {
                all_names()
            } else {
                let mut names = tool.iter().cloned().collect::<BTreeSet<_>>();
                names.insert("cpp-infra-tools".to_owned());
                names
            };
            let refs = resolve(&names, cli.offline, report)?;
            let output = if output.is_absolute() {
                output.clone()
            } else {
                project.join(output)
            };
            let output = validate_revisions_output(project, &output)?;
            atomic_write(&output, &json_bytes(&refs)?)?;
            report.bundle_id = bundle_id(&refs)?;
            report.data =
                json!({"revisions_file":output,"bundle_id":report.bundle_id,"offline":cli.offline,"stale":cli.offline});
            println!("{}", report.data);
        }
        Action::Prewarm { tool } => {
            let names = if tool.is_empty() {
                all_names()
            } else {
                let mut names = BTreeSet::new();
                for name in tool {
                    names.extend(closure(project, name, &[])?);
                }
                names
            };
            let refs = resolve(&names, cli.offline, report)?;
            let snapshot = build_snapshot(&refs, &names, cli.offline, report)?;
            report.bundle_id = snapshot.bundle_id.clone();
            let path = report.work.join("runtime-snapshot.json");
            atomic_write(&path, &json_bytes(&snapshot)?)?;
            report.data = json!({"mode":"main","runtime_snapshot":path,"offline":cli.offline,"stale":cli.offline});
            println!("{}", report.data);
        }
        Action::Run { tool, args } | Action::Latest { tool, args } => run(project, cli, tool, args, report)?,
        Action::Doctor { task, profile } => {
            crate::snapshot::validate_project(project)?;
            if let Some(profile) = profile {
                crate::snapshot::validate_profile(project, profile)?;
            }
            let defaults: Defaults = toml::from_str(&crate::util::read_configuration_text(&within(
                project,
                ".infra/tools/defaults.toml",
            )?)?)
            .configuration_context(format_args!("defaults.toml"))?;
            if defaults.schema != 1 {
                return Err(config("unsupported defaults schema"));
            }
            let catalog = Catalog {
                schema: 1,
                bundle_id: "main".to_owned(),
                protocol_major: 1,
                tools: Vec::new(),
                packages: Vec::new(),
                external_tools: defaults.external_tools,
                catalog_url: None,
                catalog_sha256: None,
            };
            report.data = crate::doctor::check(
                &catalog,
                project,
                task.as_deref(),
                &report.work.join("logs"),
                &mut report.commands,
                &mut report.external_versions,
            )?;
            println!("{}", report.data);
        }
    }
    Ok(())
}
fn validate_revisions_output(project: &Path, output: &Path) -> Result<PathBuf> {
    if let Ok(relative) = output.strip_prefix(project) {
        let text = relative.to_str().required("output path is not UTF-8")?;
        crate::report_paths::location(project, text)?;
        within(project, text)?;
    }
    let output = crate::report_paths::absolute_output(output)?;
    if output.extension().and_then(|v| v.to_str()) != Some("json") {
        return Err(config("revisions output must be a JSON file"));
    }
    if let Ok(relative) = output.strip_prefix(project) {
        crate::report_paths::location(project, relative.to_str().required("output path is not UTF-8")?)?;
    }
    if output.exists() {
        if !fs::symlink_metadata(&output)?.file_type().is_file() {
            return Err(config("revisions output is not a regular file"));
        }
        let existing: Revisions =
            read_json(&output).configuration("refusing to overwrite unknown business file with revisions")?;
        validate_refs(&existing)?;
    }
    Ok(output)
}
fn ensure_previous_authority(project: &Path, offline: bool, report: &mut Report) -> Result<()> {
    let old = crate::snapshot::manifest(project)?;
    if crate::transaction::snapshot_known(project, &old)? {
        return Ok(());
    }
    let snapshot = read_main_snapshot(project)?;
    validate_refs(&snapshot.revisions)?;
    if snapshot.bundle_id != bundle_id(&snapshot.revisions)? {
        return Err(config("old main snapshot identity mismatch"));
    }
    let mut trusted = snapshot.revisions.clone();
    for tool in &mut trusted.tools {
        tool.repository = repository(&tool.name);
    }
    let names = trusted.tools.iter().map(|tool| tool.name.clone()).collect();
    let sources = checkouts(&trusted, &names, offline, report)?;
    for tool in &trusted.tools {
        verify_main_history(
            tool,
            sources.get(&tool.name).required("missing historical source")?,
            offline,
            report,
        )?;
    }
    let manager = snapshot
        .revisions
        .tools
        .iter()
        .find(|t| t.name == "cpp-infra-tools")
        .required("old snapshot lacks manager revision")?;
    let (_, mut files) = crate::bootstrap::source_assets(
        sources.get("cpp-infra-tools").required("missing old manager source")?,
        &manager.repository,
        &manager.revision,
        true,
        None,
    )?;
    for reference in &snapshot.revisions.tools {
        let source = sources.get(&reference.name).required("missing old tool source")?;
        let manifest: Manifest = read_json(&crate::util::within(source, "conf/manifest.json")?)?;
        if manifest.schema != 1 || manifest.owner.as_ref().is_some_and(|owner| owner != &reference.name) {
            return Err(config("invalid old source manifest owner/schema"));
        }
        let role = reference
            .name
            .strip_prefix("cpp-infra-")
            .required("invalid old tool owner")?;
        for mut entry in manifest.files {
            entry.target = crate::snapshot::owned_target(role, &entry.target)?;
            crate::snapshot::validate_target(&entry.target)?;
            if !entry.target.starts_with(&format!(".infra/{role}/")) {
                return Err(config("old source manifest ownership conflict"));
            }
            let bytes = manifest_source(source, &entry.source)?;
            verify_hash(&bytes, &entry.sha256)?;
            entry.source = format!("{}@{}:{}", reference.name, reference.revision, entry.source);
            files.push(entry);
        }
    }
    files.sort_by(|a, b| a.target.cmp(&b.target));
    let authority = Manifest {
        schema: 1,
        owner: Some("cpp-infra-tools".to_owned()),
        files,
        project_files: Vec::new(),
    };
    crate::transaction::establish_snapshot(project, &old, &authority)
}
fn verify_main_history(tool: &Revision, source: &Path, offline: bool, report: &mut Report) -> Result<()> {
    if offline {
        return Err(infra(
            "offline: no trusted committed receipt to authenticate historical main ownership",
        ));
    }
    let lock_path = crate::cache::root()?
        .join("main/locks")
        .join(format!("source-{}-{}.lock", tool.name, tool.revision));
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)?;
    crate::util::acquire_lock(&lock, true)?;
    let reference = "refs/cpp-infra/authority-main";
    let fetch = |unshallow: bool, report: &mut Report| -> Result<()> {
        let mut args = vec!["fetch".to_owned(), "--no-tags".to_owned()];
        if unshallow {
            args.push("--unshallow".to_owned());
        }
        args.extend([tool.repository.clone(), format!("+refs/heads/main:{reference}")]);
        let (code, _, stderr) = git_retry(&args, source, report)?;
        if code != 0 {
            return Err(config(format!(
                "cannot authenticate trusted main history: {}",
                String::from_utf8_lossy(&stderr)
            )));
        }
        Ok(())
    };
    fetch(false, report)?;
    let arguments = vec![
        "merge-base".to_owned(),
        "--is-ancestor".to_owned(),
        tool.revision.clone(),
        reference.to_owned(),
    ];
    let (mut code, _, _) = git_retry(&arguments, source, report)?;
    if code != 0 {
        let (_, stdout, _) = git_retry(
            &["rev-parse".to_owned(), "--is-shallow-repository".to_owned()],
            source,
            report,
        )?;
        if String::from_utf8_lossy(&stdout).trim() == "true" {
            fetch(true, report)?;
            code = git_retry(&arguments, source, report)?.0;
        }
    }
    if code != 0 {
        return Err(config(format!(
            "historical revision {} is not authenticated by the configured trusted main history",
            tool.revision
        )));
    }
    Ok(())
}

pub(crate) fn locked_previous_authority(project: &Path, offline: bool, report: &mut Report) -> Result<()> {
    let old = crate::snapshot::manifest(project)?;
    if crate::transaction::snapshot_known(project, &old)? {
        return Ok(());
    }
    let locked = crate::snapshot::load(project)?;
    let url = locked
        .catalog_url
        .as_deref()
        .required("old snapshot lacks catalog provenance")?;
    let digest = locked
        .catalog_sha256
        .as_deref()
        .required("old snapshot lacks catalog SHA")?;
    let parsed = reqwest::Url::parse(url).configuration_display()?;
    let base = std::env::var("CPP_INFRA_REPOSITORY_BASE").unwrap_or_else(|_| "https://github.com/qubit-ltd".to_owned());
    let published = match reqwest::Url::parse(&base) {
        Ok(base) if base.scheme() == "https" && base.query().is_none() && base.fragment().is_none() => {
            let path = format!(
                "{}/cpp-infra-tools/releases/download/",
                base.path().trim_end_matches('/')
            );
            parsed.origin() == base.origin() && parsed.path().starts_with(&path)
        }
        _ => false,
    };
    let explicit = std::env::var("CPP_INFRA_CATALOG_URL")
        .ok()
        .and_then(|current| reqwest::Url::parse(&current).ok())
        .is_some_and(|current| {
            if current.scheme() == "file" && parsed.scheme() == "file" {
                match (current.to_file_path(), parsed.to_file_path()) {
                    (Ok(current), Ok(old)) => match (current.canonicalize(), old.canonicalize()) {
                        (Ok(current), Ok(old)) => {
                            current.parent().is_some_and(|parent| old.starts_with(parent)) && !old.starts_with(project)
                        }
                        _ => false,
                    },
                    _ => false,
                }
            } else {
                current == parsed
            }
        });
    if !published && !explicit {
        return Err(config(
            "old catalog is outside the explicitly configured trusted release source",
        ));
    }
    let bytes = crate::download::verified(url, digest, offline)?;
    let mut catalog: Catalog = crate::util::parse_json(&bytes, "historical catalog")?;
    catalog.validate()?;
    catalog.catalog_url = Some(url.to_owned());
    catalog.catalog_sha256 = Some(digest.to_owned());
    if json_bytes(&catalog)? != json_bytes(&locked)? {
        return Err(config("old snapshot differs from immutable historical catalog"));
    }
    let manager = catalog.tool("cpp-infra-tools")?;
    let reference = Revision {
        name: manager.name.clone(),
        repository: repository(&manager.name),
        revision: manager.revision.clone(),
    };
    let source = checkout(&reference, offline, report)?;
    let (_, mut files) = crate::bootstrap::source_assets(&source, url, &manager.revision, false, Some(digest))?;
    for package in &catalog.packages {
        let archive = crate::archive::unpack(
            &crate::download::verified(&package.url, &package.sha256, offline)?,
            "tar.gz",
        )?;
        let manifest: Manifest = crate::util::parse_json(
            archive
                .get("manifest.json")
                .required("historical package lacks manifest")?,
            "historical package manifest",
        )?;
        if manifest.schema != 1 || manifest.owner.as_ref().is_some_and(|o| o != &package.owner) {
            return Err(config("invalid historical package owner/schema"));
        }
        let role = package
            .owner
            .strip_prefix("cpp-infra-")
            .required("invalid historical package owner")?;
        for mut entry in manifest.files {
            entry.target = crate::snapshot::owned_target(role, &entry.target)?;
            crate::snapshot::validate_target(&entry.target)?;
            if !entry.target.starts_with(&format!(".infra/{role}/")) {
                return Err(config("historical package ownership conflict"));
            }
            let bytes = archive
                .get(&entry.source)
                .required("missing historical package source")?;
            verify_hash(bytes, &entry.sha256)?;
            entry.source = format!("{}:{}", package.owner, entry.source);
            files.push(entry);
        }
    }
    files.sort_by(|a, b| a.target.cmp(&b.target));
    let authority = Manifest {
        schema: 1,
        owner: Some("cpp-infra-tools".to_owned()),
        files,
        project_files: Vec::new(),
    };
    crate::transaction::establish_snapshot(project, &old, &authority)
}
fn read_main_snapshot(project: &Path) -> Result<MainSnapshot> {
    let path = within(project, MAIN_SNAPSHOT)?;
    if !path.is_file() {
        return Err(config("missing main snapshot; run init/update"));
    }
    read_json(&path)
}
fn all_names() -> BTreeSet<String> {
    ROLES.iter().map(|r| format!("cpp-infra-{r}")).collect()
}
fn validate_name(name: &str) -> Result<()> {
    if all_names().contains(name) {
        Ok(())
    } else {
        Err(config(format!("unknown infrastructure tool: {name}")))
    }
}
fn repository(name: &str) -> String {
    let base = std::env::var("CPP_INFRA_REPOSITORY_BASE").unwrap_or_else(|_| "https://github.com/qubit-ltd".to_owned());
    format!("{}/{name}.git", base.trim_end_matches('/'))
}
fn valid_revision(revision: &str) -> bool {
    revision.len() == 40 && revision.bytes().all(|c| c.is_ascii_hexdigit())
}
fn validate_refs(refs: &Revisions) -> Result<()> {
    if refs.schema != 1 || refs.tools.is_empty() {
        return Err(config("invalid portable revisions schema/empty group"));
    }
    let mut names = BTreeSet::new();
    for tool in &refs.tools {
        validate_name(&tool.name)?;
        if !valid_revision(&tool.revision) || tool.repository.is_empty() || !names.insert(&tool.name) {
            return Err(config("invalid/duplicate portable tool revision"));
        }
    }
    Ok(())
}
fn bundle_id(refs: &Revisions) -> Result<String> {
    let mut pairs: Vec<_> = refs.tools.iter().map(|t| (&t.name, &t.revision)).collect();
    pairs.sort();
    Ok(format!("main-{}", hash(&json_bytes(&pairs)?)))
}
fn resolve(names: &BTreeSet<String>, offline: bool, report: &mut Report) -> Result<Revisions> {
    for name in names {
        validate_name(name)?;
    }
    if let Some(value) = &report.runtime_revisions {
        let refs: Revisions =
            serde_json::from_value(value.clone()).configuration_context(format_args!("context revisions"))?;
        validate_refs(&refs)?;
        for name in names {
            if !refs.tools.iter().any(|t| &t.name == name) {
                return Err(config(format!("run context revisions do not contain {name}")));
            }
        }
        return Ok(refs);
    }
    if let Some(path) = std::env::var_os("CPP_INFRA_REVISIONS_FILE") {
        let refs: Revisions = read_json(Path::new(&path))?;
        validate_refs(&refs)?;
        for name in names {
            if !refs.tools.iter().any(|t| &t.name == name) {
                return Err(config(format!("portable revisions do not include {name}")));
            }
        }
        return Ok(refs);
    }
    let key = hash(&json_bytes(
        &names.iter().map(|name| (name, repository(name))).collect::<Vec<_>>(),
    )?);
    let latest = crate::cache::root()?
        .join("main/resolutions")
        .join(format!("{key}.json"));
    if offline {
        if !latest.is_file() {
            return Err(infra(
                "offline: no previously resolved main snapshot for the requested tools",
            ));
        }
        let refs: Revisions = read_json(&latest)?;
        validate_refs(&refs)?;
        return Ok(refs);
    }
    let mut tools = Vec::new();
    for name in names {
        let source = repository(name);
        let args = vec!["ls-remote".to_owned(), source.clone(), "refs/heads/main".to_owned()];
        let (code, stdout, stderr) = git_retry(&args, &report.project.clone(), report)?;
        if code != 0 {
            return Err(infra(format!(
                "resolve {name} main failed: {}",
                String::from_utf8_lossy(&stderr)
            )));
        }
        let text = String::from_utf8(stdout).configuration_display()?;
        let revision = text
            .split_whitespace()
            .next()
            .required(format_args!("{name} main has no published revision"))?;
        if !valid_revision(revision) {
            return Err(config("main returned an invalid Git SHA"));
        }
        tools.push(Revision {
            name: name.clone(),
            repository: source,
            revision: revision.to_ascii_lowercase(),
        });
    }
    let refs = Revisions { schema: 1, tools };
    atomic_write(&latest, &json_bytes(&refs)?)?;
    Ok(refs)
}
fn git_retry(args: &[String], cwd: &Path, report: &mut Report) -> Result<(i32, Vec<u8>, Vec<u8>)> {
    for attempt in 0..4 {
        let output = command(
            Path::new("git"),
            args,
            cwd,
            &BTreeMap::from([("GIT_TERMINAL_PROMPT".to_owned(), "0".to_owned())]),
            &report.work.join("logs"),
            Duration::from_secs(90),
            &mut report.commands,
        )?;
        if output.0 == 0 {
            return Ok(output);
        }
        let error = String::from_utf8_lossy(&output.2).to_ascii_lowercase();
        if attempt == 3
            || ![
                "timed out",
                "could not resolve host",
                "connection reset",
                "remote end hung up",
                "http 502",
                "http 503",
            ]
            .iter()
            .any(|needle| error.contains(needle))
        {
            return Ok(output);
        }
        for _ in 0..(20 << attempt) {
            crate::util::cancelled()?;
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    Err(infra("Git retry exhausted"))
}
fn checkout(tool: &Revision, offline: bool, report: &mut Report) -> Result<PathBuf> {
    let root = crate::cache::root()?.join("main");
    let directory = root.join("sources").join(&tool.name).join(&tool.revision);
    let locks = root.join("locks");
    fs::create_dir_all(&locks)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(locks.join(format!("source-{}-{}.lock", tool.name, tool.revision)))?;
    crate::util::acquire_lock(&lock, true)?;
    fs::create_dir_all(&directory)?;
    if directory.join(".git").is_dir() {
        let args = vec!["rev-parse".to_owned(), "HEAD".to_owned()];
        let (code, stdout, _) = git_retry(&args, &directory, report)?;
        if code == 0 && String::from_utf8_lossy(&stdout).trim() == tool.revision {
            let (code, stdout, _) = git_retry(
                &[
                    "status".to_owned(),
                    "--porcelain".to_owned(),
                    "--untracked-files=all".to_owned(),
                ],
                &directory,
                report,
            )?;
            if code != 0 || !stdout.is_empty() {
                return Err(config("cached source checkout has local modifications"));
            }
            return Ok(directory);
        }
    }
    if offline {
        return Err(infra(format!(
            "offline: missing pinned source checkout {}@{}",
            tool.name, tool.revision
        )));
    }
    if !directory.join(".git").is_dir() {
        for args in [
            vec!["init".to_owned(), "--quiet".to_owned()],
            vec![
                "remote".to_owned(),
                "add".to_owned(),
                "origin".to_owned(),
                tool.repository.clone(),
            ],
        ] {
            let (code, _, stderr) = git_retry(&args, &directory, report)?;
            if code != 0 {
                return Err(infra(format!(
                    "initialize source checkout: {}",
                    String::from_utf8_lossy(&stderr)
                )));
            }
        }
    }
    for args in [
        vec![
            "fetch".to_owned(),
            "--depth=1".to_owned(),
            "origin".to_owned(),
            tool.revision.clone(),
        ],
        vec![
            "checkout".to_owned(),
            "--quiet".to_owned(),
            "--detach".to_owned(),
            tool.revision.clone(),
        ],
    ] {
        let (code, _, stderr) = git_retry(&args, &directory, report)?;
        if code != 0 {
            return Err(infra(format!(
                "checkout {}@{}: {}",
                tool.name,
                tool.revision,
                String::from_utf8_lossy(&stderr)
            )));
        }
    }
    Ok(directory)
}
fn checkouts(
    refs: &Revisions,
    names: &BTreeSet<String>,
    offline: bool,
    report: &mut Report,
) -> Result<BTreeMap<String, PathBuf>> {
    let mut result = BTreeMap::new();
    for tool in &refs.tools {
        if names.contains(&tool.name) {
            result.insert(tool.name.clone(), checkout(tool, offline, report)?);
        }
    }
    Ok(result)
}
fn cargo_version(source: &Path) -> Result<String> {
    let value: toml::Value = toml::from_str(&fs::read_to_string(source.join("Cargo.toml"))?)
        .configuration_context(format_args!("Cargo metadata"))?;
    let version = value
        .get("package")
        .and_then(|p| p.get("version"))
        .and_then(toml::Value::as_str)
        .required("tool Cargo.toml requires a literal package.version")?;
    semver::Version::parse(version).configuration_display()?;
    Ok(version.to_owned())
}
fn build_snapshot(
    refs: &Revisions,
    names: &BTreeSet<String>,
    offline: bool,
    report: &mut Report,
) -> Result<MainSnapshot> {
    let sources = checkouts(refs, names, offline, report)?;
    let mut tools = Vec::new();
    let mut fingerprints = Vec::new();
    let mut hosts = BTreeSet::new();
    for tool in &refs.tools {
        if !names.contains(&tool.name) {
            continue;
        }
        let source = sources.get(&tool.name).required("missing prepared source")?;
        let version = cargo_version(source)?;
        let (code, stdout, stderr) = command(
            Path::new("rustc"),
            &["--version".to_owned(), "--verbose".to_owned()],
            source,
            &BTreeMap::new(),
            &report.work.join("logs"),
            Duration::from_secs(15),
            &mut report.commands,
        )?;
        if code != 0 {
            return Err(infra(format!(
                "rustc in tool source: {}",
                String::from_utf8_lossy(&stderr)
            )));
        }
        let rustc = String::from_utf8(stdout).configuration_display()?;
        let host = rustc
            .lines()
            .find_map(|line| line.strip_prefix("host: "))
            .required("rustc did not report its host")?
            .to_owned();
        fingerprints.push((tool.name.clone(), rustc.clone()));
        hosts.insert(host.clone());
        report.external_versions.insert(
            format!("rustc:{}", tool.name),
            json!({"actual":rustc.trim(),"host":host,"cwd":source}),
        );

        let directory = crate::cache::root()?
            .join("main/tools")
            .join(&tool.name)
            .join(&tool.revision)
            .join(&host)
            .join(hash(rustc.as_bytes()));
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
        let marker = directory.join("build.json");
        let cargo_lock = hash(&fs::read(source.join("Cargo.lock"))?);
        let binary_sha = if binary.is_file() {
            Some(hash(&fs::read(&binary)?))
        } else {
            None
        };
        let ready = if binary.is_file() && marker.is_file() {
            match read_json::<Value>(&marker) {
                Ok(value) => {
                    value["schema"] == 1
                        && value["revision"] == tool.revision
                        && value["rustc"] == rustc
                        && value["host"] == host
                        && value["cargo_lock_sha256"] == cargo_lock
                        && value["executable_sha256"]
                            .as_str()
                            .is_some_and(|expected| binary_sha.as_deref() == Some(expected))
                }
                Err(_) => false,
            }
        } else {
            false
        };
        if !ready {
            let args = vec![
                "build".to_owned(),
                "--release".to_owned(),
                "--locked".to_owned(),
                "--bin".to_owned(),
                tool.name.clone(),
                "--target-dir".to_owned(),
                directory.join("target").to_string_lossy().into_owned(),
            ];
            let mut environment = BTreeMap::from([
                ("CPP_INFRA_BUILD_REVISION".to_owned(), tool.revision.clone()),
                ("CARGO_BUILD_JOBS".to_owned(), "2".to_owned()),
            ]);
            if offline {
                environment.insert("CARGO_NET_OFFLINE".to_owned(), "true".to_owned());
            }
            let (code, _, stderr) = crate::runtime::command_network(
                Path::new("cargo"),
                &args,
                source,
                &environment,
                &report.work.join("logs"),
                Duration::from_secs(1800),
                &mut report.commands,
            )?;
            if code != 0 {
                return Err(crate::ToolsError::Gate(format!(
                    "{} pinned main build failed: {}",
                    tool.name,
                    String::from_utf8_lossy(&stderr)
                )));
            }
            let built = directory
                .join("target/release")
                .join(binary.file_name().required("invalid binary name")?);
            let bytes = fs::read(built)?;
            atomic_write(&binary, &bytes)?;
            executable(&binary, true)?;
            atomic_write(
                &marker,
                &json_bytes(
                    &json!({"schema":1,"revision":tool.revision,"host":host,"rustc":rustc,"cargo_lock_sha256":cargo_lock,"executable_sha256":hash(&bytes)}),
                )?,
            )?;
        }
        let (code, stdout, stderr) = command(
            &binary,
            &["--version".to_owned(), "--json".to_owned()],
            source,
            &BTreeMap::new(),
            &report.work.join("logs"),
            Duration::from_secs(15),
            &mut report.commands,
        )?;
        if code != 0 {
            return Err(config(format!(
                "main executable version query failed: {}",
                String::from_utf8_lossy(&stderr)
            )));
        }
        let identity: Value = parse_json(&stdout, "main executable identity")?;
        if identity["name"] != tool.name
            || identity["version"] != version
            || identity["revision"] != tool.revision
            || identity["protocol"] != 1
        {
            return Err(config(format!("{} main executable identity mismatch", tool.name)));
        }
        tools.push(MainTool {
            name: tool.name.clone(),
            repository: tool.repository.clone(),
            revision: tool.revision.clone(),
            version,
            protocol: 1,
            executable_sha256: Some(hash(&fs::read(&binary)?)),
            binary: Some(binary),
        });
    }
    let host = if hosts.len() == 1 {
        hosts.into_iter().next().required("missing host")?
    } else {
        return Err(config("tool sources select incompatible Rust hosts"));
    };
    let rustc = if fingerprints.len() == 1 {
        fingerprints.remove(0).1
    } else {
        String::from_utf8(json_bytes(&fingerprints)?).configuration_display()?
    };
    Ok(MainSnapshot {
        schema: 1,
        bundle_id: bundle_id(refs)?,
        tools,
        revisions: refs.clone(),
        offline,
        stale: offline,
        host,
        rustc,
    })
}
pub(crate) fn closure(project: &Path, selected: &str, args: &[String]) -> Result<BTreeSet<String>> {
    let profile = if args.is_empty() {
        crate::profile_selection::prewarm(project, selected)?
    } else {
        crate::profile_selection::selected(project, selected, args)?
    };
    closure_for_profile(project, selected, profile.as_deref())
}
pub(crate) fn closure_for_profile(project: &Path, selected: &str, profile: Option<&str>) -> Result<BTreeSet<String>> {
    validate_name(selected)?;
    // CI owns doctor, quality-summary and matrix expansion of its task graph.
    if selected == "cpp-infra-ci" {
        return Ok(all_names());
    }
    let mut names = BTreeSet::from([selected.to_owned()]);
    if matches!(selected, "cpp-infra-analyze" | "cpp-infra-coverage") {
        names.insert("cpp-infra-verify".to_owned());
    }
    if let Some(profile) = profile {
        let path = project.join(".infra/project.toml");
        if path.is_file() {
            let project_value: toml::Value = toml::from_str(&crate::util::read_configuration_text(
                &crate::util::tracked_config(project, &path)?,
            )?)
            .configuration_display()?;
            if project_value
                .get("profiles")
                .and_then(|profiles| profiles.get(profile))
                .and_then(|value| value.get("conan_context"))
                .and_then(toml::Value::as_str)
                .is_some_and(|context| !context.is_empty())
            {
                names.insert("cpp-infra-dependency".to_owned());
            }
        }
    }
    Ok(names)
}

fn run(project: &Path, cli: &Cli, selected: &str, args: &[String], report: &mut Report) -> Result<()> {
    let inspection = crate::introspection::classify(selected, args);
    let profile = crate::profile_selection::selected(project, selected, args)?;
    if inspection.is_none() && project.join(crate::snapshot::MANIFEST).exists() {
        crate::snapshot::check_managed(project)?;
    }
    let names = if inspection.is_some() {
        validate_name(selected)?;
        BTreeSet::from([selected.to_owned()])
    } else {
        closure_for_profile(project, selected, profile.as_deref())?
    };
    let refs = resolve(&names, cli.offline, report)?;
    let snapshot = build_snapshot(&refs, &names, cli.offline, report)?;
    report.bundle_id = snapshot.bundle_id.clone();
    let path = report.work.join("runtime-snapshot.json");
    let bytes = json_bytes(&snapshot)?;
    atomic_write(&path, &bytes)?;
    let bin = crate::cache::root()?
        .join("main/bundles")
        .join(&snapshot.bundle_id)
        .join(hash(snapshot.rustc.as_bytes()))
        .join(&snapshot.host)
        .join("bin");
    fs::create_dir_all(&bin)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(bin.join(".lock"))?;
    crate::util::acquire_lock(&lock, true)?;
    for tool in &snapshot.tools {
        let binary = tool.binary.as_ref().required("runtime entry has no binary")?;
        let destination = bin.join(binary.file_name().required("invalid binary path")?);
        let contents = fs::read(binary)?;
        if !destination.exists() || fs::read(&destination)? != contents {
            atomic_write(&destination, &contents)?;
            executable(&destination, true)?;
        }
    }
    drop(lock);
    let context = report.context(profile.as_deref())?;
    let mut value: Value = read_json(&context)?;
    value["runtime_snapshot"] = json!(path);
    value["runtime_snapshot_digest"] = json!(hash(&bytes));
    atomic_write(&context, &json_bytes(&value)?)?;
    let expected_context = value;
    if let Some(kind) = inspection {
        let selected_tool = snapshot
            .tools
            .iter()
            .find(|tool| tool.name == selected)
            .required("missing selected introspection identity")?;
        let binary = selected_tool.binary.as_ref().required("missing introspection binary")?;
        let environment = BTreeMap::from([
            ("CPP_INFRA_BIN_DIR".to_owned(), bin.to_string_lossy().into_owned()),
            (
                "CPP_INFRA_RUNTIME_SNAPSHOT".to_owned(),
                path.to_string_lossy().into_owned(),
            ),
        ]);
        report.data = json!({"mode":"main","tool":selected,"runtime_snapshot":path,"run_context":context,"offline":cli.offline,"stale":cli.offline});
        report.artifacts.push(json!({"kind":"runtime-snapshot","path":report.artifact_path(&path)?,"sha256":hash(&bytes),"required":true}));
        return crate::introspection::execute(
            binary,
            args,
            &environment,
            report,
            &expected_context,
            &json!({"name":selected_tool.name,"version":selected_tool.version,"revision":selected_tool.revision,"protocol":selected_tool.protocol}),
            kind,
        );
    }
    let artifact_report = report.work.join(format!("{selected}.json"));
    let child_report = crate::report_paths::child_output(cli, project)?.unwrap_or_else(|| artifact_report.clone());
    let binary = snapshot
        .tools
        .iter()
        .find(|t| t.name == selected)
        .and_then(|t| t.binary.as_ref())
        .required("selected runtime binary missing")?;
    crate::child_report::prepare(&child_report, report)?;
    let inputs = crate::input_guard::InputGuard::capture(project, selected, args, &expected_context)?;
    let mut child_args = vec![
        "--project".to_owned(),
        project.to_string_lossy().into_owned(),
        "--report".to_owned(),
        child_report.to_string_lossy().into_owned(),
        "--run-context".to_owned(),
        context.to_string_lossy().into_owned(),
    ];
    if cli.offline {
        child_args.push("--offline".to_owned());
    }
    child_args.extend(crate::report_paths::child_args(args));
    let environment = BTreeMap::from([
        ("CPP_INFRA_BIN_DIR".to_owned(), bin.to_string_lossy().into_owned()),
        (
            "CPP_INFRA_RUNTIME_SNAPSHOT".to_owned(),
            path.to_string_lossy().into_owned(),
        ),
    ]);
    let (code, stdout, stderr) = command(
        binary,
        &child_args,
        project,
        &environment,
        &report.work.join("logs"),
        Duration::from_secs(3600),
        &mut report.commands,
    )?;
    io::stdout().write_all(&stdout)?;
    io::stderr().write_all(&stderr)?;
    report.artifacts.push(
        json!({"kind":"runtime-snapshot","path":report.artifact_path(&path)?,"sha256":hash(&bytes),"required":true}),
    );
    report.data = json!({"mode":"main","tool":selected,"runtime_snapshot":path,"run_context":context,"offline":cli.offline,"stale":cli.offline,"exit_code":code});
    crate::child_report::archive(&child_report, &artifact_report, report)?;
    let expected_context = inputs.finish(project, report, &expected_context)?;
    if code == 0 {
        let selected_tool = snapshot
            .tools
            .iter()
            .find(|t| t.name == selected)
            .required("missing selected identity")?;
        crate::child_report::validate(
            &child_report,
            &json!({"name":selected_tool.name,"version":selected_tool.version,"revision":selected_tool.revision,"protocol":selected_tool.protocol}),
            &expected_context,
        )?;
    } else {
        return Err(match code {
            2 => config(format!("{selected} configuration/protocol failure")),
            3 => infra(format!("{selected} infrastructure failure")),
            130 => crate::ToolsError::Cancelled,
            _ => crate::ToolsError::Gate(format!("{selected} exited {code}")),
        });
    }

    Ok(())
}
fn update(
    project: &Path,
    refs: &Revisions,
    sources: &BTreeMap<String, PathBuf>,
    initial: bool,
    dry_run: bool,
    offline: bool,
) -> Result<Value> {
    let old = if project.join(crate::snapshot::MANIFEST).is_file() {
        Some(crate::snapshot::manifest(project)?)
    } else {
        None
    };
    if initial && old.is_some() {
        return Err(config("project already initialized; use update"));
    }
    if !initial && old.is_none() {
        return Err(config("missing managed snapshot; use init"));
    }
    if old.is_some() {
        crate::snapshot::check_managed(project)?;
    }
    let manager = refs
        .tools
        .iter()
        .find(|t| t.name == "cpp-infra-tools")
        .required("configuration update requires manager revision")?;
    let manager_source = sources.get("cpp-infra-tools").required("manager source is missing")?;
    let (mut files, mut managed) =
        crate::bootstrap::main_files(project, manager_source, &manager.repository, &manager.revision)?;
    if !initial {
        files.remove(".infra/project.toml");
        files.remove("CMakePresets.json");
    }
    let mut seen: BTreeSet<_> = managed.iter().map(|f| f.target.clone()).collect();
    let mut tools = Vec::new();
    for reference in &refs.tools {
        let Some(source) = sources.get(&reference.name) else {
            continue;
        };
        let path = source.join("conf/manifest.json");
        if !path.is_file() {
            return Err(config(format!("{} main has no conf/manifest.json", reference.name)));
        }
        let manifest: Manifest = read_json(&path)?;
        if manifest.schema != 1 || manifest.owner.as_ref().is_some_and(|o| o != &reference.name) {
            return Err(config("main manifest schema/owner mismatch"));
        }
        let role = reference.name.strip_prefix("cpp-infra-").required("invalid owner")?;
        for mut entry in manifest.files {
            entry.target = crate::snapshot::owned_target(role, &entry.target)?;
            crate::snapshot::validate_target(&entry.target)?;
            if !entry.target.starts_with(&format!(".infra/{role}/")) || !seen.insert(entry.target.clone()) {
                return Err(config(format!("main manifest ownership conflict: {}", entry.target)));
            }
            let bytes = manifest_source(source, &entry.source)?;
            verify_hash(&bytes, &entry.sha256)?;
            files.insert(entry.target.clone(), (bytes, entry.executable));
            managed.push(crate::managed_file::ManagedFile {
                source: format!("{}@{}:{}", reference.name, reference.revision, entry.source),
                ..entry
            });
        }
        if initial {
            for mut entry in manifest.project_files {
                entry.target = crate::snapshot::owned_target(role, &entry.target)?;
                if !entry.target.starts_with(&format!(".infra/{role}/"))
                    || entry.target.contains("/policy/")
                    || !seen.insert(entry.target.clone())
                {
                    return Err(config("invalid initial project config target"));
                }
                if within(project, &entry.target)?.exists() {
                    continue;
                }
                let bytes = manifest_source(source, &entry.source)?;
                verify_hash(&bytes, &entry.sha256)?;
                files.insert(entry.target, (bytes, false));
            }
        }
        tools.push(MainTool {
            name: reference.name.clone(),
            repository: reference.repository.clone(),
            revision: reference.revision.clone(),
            version: cargo_version(source)?,
            protocol: 1,
            binary: None,
            executable_sha256: None,
        });
    }
    if let Some((bytes, _)) = files.get(".infra/project.toml") {
        crate::snapshot::validate_project_text(project, std::str::from_utf8(bytes).configuration_display()?)?;
    }
    for (target, (bytes, _)) in &files {
        if target.ends_with("/config.toml") {
            let value: toml::Value =
                toml::from_str(std::str::from_utf8(bytes).configuration_display()?).configuration_display()?;
            if value.get("schema").and_then(toml::Value::as_integer) != Some(1) {
                return Err(config("initial config requires schema=1"));
            }
        }
    }
    let old_names: BTreeSet<_> = old
        .as_ref()
        .map(|m| m.files.iter().map(|f| f.target.clone()).collect())
        .unwrap_or_default();
    let new_names: BTreeSet<_> = managed.iter().map(|f| f.target.clone()).collect();
    for (target, (bytes, _)) in &files {
        let path = within(project, target)?;
        if path.exists() && !old_names.contains(target) && fs::read(path)? != *bytes {
            return Err(config(format!("unmanaged file conflict: {target}")));
        }
    }
    let retired: Vec<_> = old_names.difference(&new_names).cloned().collect();
    managed.sort_by(|a, b| a.target.cmp(&b.target));
    let manifest = Manifest {
        schema: 1,
        owner: Some("cpp-infra-tools".to_owned()),
        files: managed,
        project_files: Vec::new(),
    };
    let snapshot = MainSnapshot {
        schema: 1,
        bundle_id: bundle_id(refs)?,
        tools,
        revisions: refs.clone(),
        offline,
        stale: offline,
        host: String::new(),
        rustc: String::new(),
    };
    files.insert(MAIN_SNAPSHOT.to_owned(), (json_bytes(&snapshot)?, false));
    files.insert(crate::snapshot::MANIFEST.to_owned(), (json_bytes(&manifest)?, false));
    let changed: Vec<_> = files
        .iter()
        .filter_map(|(target, (bytes, _))| match fs::read(project.join(target)) {
            Ok(previous) if previous == *bytes => None,
            _ => Some(target.clone()),
        })
        .collect();
    let plan = json!({"schema":1,"mode":"main","bundle_id":snapshot.bundle_id,"files":changed,"retired":retired,"dry_run":dry_run,"offline":offline,"stale":offline});
    if !dry_run {
        crate::transaction::apply(project, &files, &retired)?;
    }
    Ok(plan)
}
fn manifest_source(source: &Path, path: &str) -> Result<Vec<u8>> {
    safe_relative(path)?;
    let root = if path.starts_with("conf/") {
        source.to_path_buf()
    } else {
        source.join("conf")
    };
    Ok(fs::read(within(&root, path)?)?)
}
