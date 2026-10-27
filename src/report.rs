// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Complete common reports and validated execution identity.
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

use serde_json::Value;
use serde_json::json;

use crate::cli::Cli;
use crate::error_context::ErrorContext;
use crate::required::Required;
use crate::tools_error::Result;
use crate::tools_error::config;
use crate::util::atomic_write;
use crate::util::digests;
use crate::util::json_bytes;
use crate::util::read_json;
use crate::util::safe_relative;
use crate::util::unique;
pub(crate) struct Report {
    pub started: Instant,
    pub started_at: String,
    pub run_id: String,
    pub task_id: String,
    pub bundle_id: String,
    pub project: PathBuf,
    pub root: PathBuf,
    pub work: PathBuf,
    pub source_excluded_paths: Vec<String>,
    pub commands: Vec<Value>,
    pub external_versions: BTreeMap<String, Value>,
    pub artifacts: Vec<Value>,
    pub data: Value,
    pub runtime_revisions: Option<Value>,
    pub profile: Option<String>,
}
impl Report {
    pub fn new(project: &Path, cli: &Cli) -> Result<Self> {
        let run_id = unique();
        let root = project.join("build/infra-runs").join(&run_id);
        let inherited = if let Some(path) = &cli.run_context {
            read_json::<Value>(path)?
                .get("source_excluded_paths")
                .cloned()
                .map(serde_json::from_value::<Vec<String>>)
                .transpose()
                .configuration_context(format_args!("source_excluded_paths"))?
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let excluded = crate::report_paths::merge(project, &inherited, cli.report.as_deref())?;
        let child = crate::report_paths::child_output(cli, project)?;
        let excluded = crate::report_paths::merge(project, &excluded, child.as_deref())?;
        if let (Some(parent), Some(child)) = (&cli.report, &child)
            && crate::report_paths::absolute_output(parent)? == crate::report_paths::absolute_output(child)?
        {
            return Err(config("parent and child report outputs must be distinct"));
        }
        let work = root.join("tools").join(unique());
        let mut report = Self {
            started: Instant::now(),
            started_at: chrono::Utc::now().to_rfc3339(),
            run_id,
            task_id: "tools".to_owned(),
            bundle_id: "development".to_owned(),
            project: project.to_path_buf(),
            root,
            work,
            source_excluded_paths: excluded,
            commands: Vec::new(),
            external_versions: BTreeMap::new(),
            artifacts: Vec::new(),
            data: Value::Null,
            runtime_revisions: None,
            profile: None,
        };
        if let Some(path) = &cli.run_context {
            let context: Value = read_json(path)?;
            if context["schema"] != 1 {
                return Err(config("unsupported run context schema"));
            }
            report.profile = match context.get("profile") {
                Some(Value::Null) => None,
                Some(Value::String(value)) if !value.is_empty() => Some(value.clone()),
                _ => return Err(config("run context profile must be null or a nonempty string")),
            };
            let context_project = context["project"].as_str().required("missing context project")?;
            if Path::new(context_project).canonicalize()? != project || Path::new(context_project) != project {
                return Err(config("run context project is not the canonical project path"));
            }
            let (source, configuration) = digests(project, &report.source_excluded_paths)?;
            if context["source_digest"] != source || context["config_digest"] != configuration {
                return Err(config("stale run context source/config digest"));
            }
            let bundle = context["bundle_id"].as_str().required("missing context bundle_id")?;
            report.bundle_id = bundle.to_owned();
            report.run_id = context["run_id"]
                .as_str()
                .required("missing context run_id")?
                .to_owned();
            if safe_relative(&report.run_id)?.components().count() != 1 {
                return Err(config("unsafe run_id"));
            }
            report.task_id = context["task_id"]
                .as_str()
                .required("missing context task_id")?
                .to_owned();
            report.root = PathBuf::from(
                context["report_root"]
                    .as_str()
                    .required("missing context report_root")?,
            );
            if !report.root.starts_with(project.join("build/infra-runs")) {
                return Err(config("run context report_root is outside project run roots"));
            }
            let relative = report
                .root
                .strip_prefix(project)
                .configuration_display()?
                .to_str()
                .required("report root is not UTF-8")?;
            crate::util::within(project, relative)?;
            report.work = report.root.join("tools").join(unique());
            if let Some(path) = context.get("runtime_snapshot").and_then(Value::as_str) {
                let snapshot_path = Path::new(path);
                let relative = snapshot_path
                    .strip_prefix(&report.root)
                    .configuration("runtime snapshot is outside report_root")?
                    .to_str()
                    .required("snapshot path is not UTF-8")?;
                let snapshot_path = crate::util::within(&report.root, relative)?;
                let bytes = fs::read(&snapshot_path)?;
                crate::util::verify_hash(
                    &bytes,
                    context["runtime_snapshot_digest"]
                        .as_str()
                        .required("missing runtime snapshot digest")?,
                )?;
                let snapshot: Value = crate::util::parse_json(&bytes, "runtime snapshot")?;
                if snapshot["schema"] != 1 || snapshot["bundle_id"] != bundle {
                    return Err(config("runtime snapshot schema/bundle mismatch"));
                }
                report.runtime_revisions = snapshot.get("revisions").cloned();
                let tools = snapshot["tools"]
                    .as_array()
                    .required("runtime snapshot tools is not an array")?;
                if let Some(manager) = tools.iter().find(|t| t["name"] == "cpp-infra-tools") {
                    let version = crate::application::version();
                    for key in ["name", "version", "revision", "protocol"] {
                        if manager[key] != version[key] {
                            return Err(config("runtime manager identity mismatch"));
                        }
                    }
                    let entry = Path::new(manager["binary"].as_str().required("missing manager binary path")?);
                    if !entry.is_absolute() || !entry.is_file() {
                        return Err(config("manager snapshot binary is not an existing absolute executable"));
                    }
                    crate::util::verify_hash(
                        &fs::read(entry)?,
                        manager["executable_sha256"]
                            .as_str()
                            .required("missing manager executable SHA")?,
                    )?;
                    let binary = std::env::current_exe()?;
                    crate::util::verify_hash(
                        &fs::read(binary)?,
                        manager["executable_sha256"]
                            .as_str()
                            .required("missing manager executable SHA")?,
                    )?;
                }
            } else if bundle != "development" {
                let snapshot = crate::snapshot::check(project)?;
                if snapshot.bundle_id != bundle {
                    return Err(config("run context bundle does not match the snapshot"));
                }
            }
        }
        Ok(report)
    }
    pub fn context(&self, selected: Option<&str>) -> Result<PathBuf> {
        let (source, configuration) = digests(&self.project, &self.source_excluded_paths)?;
        let path = self.work.join("run-context.json");
        if let (Some(parent), Some(selected)) = (&self.profile, selected)
            && parent != selected
        {
            return Err(config("run context profile conflicts with selected business profile"));
        }
        let profile = selected.map(str::to_owned).or_else(|| self.profile.clone());
        atomic_write(
            &path,
            &json_bytes(
                &json!({"schema":1,"run_id":self.run_id,"task_id":self.task_id,"bundle_id":self.bundle_id,"project":self.project,"profile":profile,"source_digest":source,"config_digest":configuration,"report_root":self.root,"source_excluded_paths":self.source_excluded_paths}),
            )?,
        )?;
        Ok(path)
    }
    pub fn artifact_path(&self, path: &Path) -> Result<String> {
        Ok(path
            .strip_prefix(&self.root)
            .configuration("artifact is outside report_root")?
            .to_str()
            .required("artifact path is not UTF-8")?
            .replace('\\', "/"))
    }
    pub fn write(&mut self, cli: &Cli, result: &Result<()>) -> Result<()> {
        let Some(path) = &cli.report else {
            return Ok(());
        };
        crate::report_paths::merge(&self.project, &self.source_excluded_paths, Some(path))?;
        let (source, configuration) = match digests(&self.project, &self.source_excluded_paths) {
            Ok((source, configuration)) => (Some(source), Some(configuration)),
            Err(error) if result.is_err() => {
                eprintln!("report digest unavailable: {error}");
                (None, None)
            }
            Err(error) => return Err(error),
        };
        for (index, record) in self.commands.iter_mut().enumerate() {
            if let Some(path) = record["log"].as_str() {
                let source = Path::new(path);
                if source.is_file() {
                    let destination = self.work.join(format!("logs/command-{index}.log"));
                    atomic_write(&destination, &fs::read(source)?)?;
                    record["log"] = json!(
                        destination
                            .strip_prefix(&self.root)
                            .configuration_display()?
                            .to_string_lossy()
                            .replace('\\', "/")
                    );
                }
            }
        }
        let status = match result {
            Ok(()) => "passed",
            Err(crate::ToolsError::Gate(_)) => "failed",
            Err(crate::ToolsError::Cancelled) => "cancelled",
            Err(_) => "error",
        };
        let diagnostics = match result {
            Ok(()) => Vec::new(),
            Err(error) => vec![
                json!({"code":format!("TOOLS_E{}",error.exit_code()),"severity":"error","message":error.to_string(),"project_relative_path":Value::Null}),
            ],
        };
        let mut git = Command::new("git");
        crate::runtime::isolate_git(&mut git);
        let revision = git
            .args(["rev-parse", "HEAD"])
            .current_dir(&self.project)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .map(|text| text.trim().to_owned());
        let output = json!({"schema":1,"tool":crate::application::version(),"bundle_id":self.bundle_id,"run_id":self.run_id,"task_id":self.task_id,"project":self.project,"report_root":self.root,"source_revision":revision,"source_digest":source,"config_digest":configuration,"status":status,"started_at":self.started_at,"duration_ms":self.started.elapsed().as_millis(),"external_versions":self.external_versions,"commands":self.commands,"diagnostics":diagnostics,"artifacts":self.artifacts,"data":self.data,"source_excluded_paths":self.source_excluded_paths});
        atomic_write(path, &json_bytes(&output)?)
    }
}
