// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Validate individual report exclusions without hiding project inputs.
use std::collections::BTreeSet;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use serde_json::Value;

use crate::cli::Action;
use crate::cli::Cli;
use crate::error_context::ErrorContext;
use crate::required::Required;
use crate::tools_error::Result;
use crate::tools_error::config;
use crate::util::read_json;
use crate::util::safe_relative;
use crate::util::within;

pub(crate) fn absolute_output(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    if absolute.exists() {
        return Ok(absolute.canonicalize()?);
    }
    let mut ancestor = absolute.as_path();
    let mut suffix = Vec::new();
    while !ancestor.exists() {
        suffix.push(ancestor.file_name().required("invalid output path")?.to_os_string());
        ancestor = ancestor.parent().required("missing output ancestor")?;
    }
    let mut result = ancestor.canonicalize()?;
    for component in suffix.into_iter().rev() {
        if component == ".." {
            result.pop();
        } else if component != "." {
            result.push(component);
        }
    }
    Ok(result)
}
pub(crate) fn location(project: &Path, relative: &str) -> Result<()> {
    let path = safe_relative(relative)?;
    if path.extension().and_then(|v| v.to_str()) != Some("json") {
        return Err(config("source exclusion must name one JSON report file"));
    }
    let first = path.components().next().required("empty report exclusion")?;
    if [
        ".infra",
        ".git",
        "conan",
        "src",
        "include",
        "tests",
        "test",
        "generated",
        "third_party",
        "examples",
    ]
    .iter()
    .any(|name| first == Component::Normal(std::ffi::OsStr::new(name)))
    {
        return Err(config(format!(
            "report output/exclusion overlaps protected input: {relative}"
        )));
    }
    if [
        "CMakeLists.txt",
        "CMakePresets.json",
        "CMakeUserPresets.json",
        "conanfile.py",
        "conanfile.txt",
    ]
    .contains(&relative)
        || relative.to_ascii_uppercase().starts_with("README")
    {
        return Err(config("report output/exclusion overlaps reserved project input"));
    }
    let project_config = project.join(".infra/project.toml");
    if project_config.is_file() {
        let value: toml::Value = toml::from_str(&crate::util::read_configuration_text(&crate::util::tracked_config(
            project,
            &project_config,
        )?)?)
        .configuration_display()?;
        for key in ["source_roots", "test_roots", "generated_roots", "third_party_roots"] {
            if let Some(roots) = value.get(key).and_then(toml::Value::as_array) {
                for root in roots {
                    let root = root.as_str().required("configured root is not a string")?;
                    if path.starts_with(safe_relative(root)?) {
                        return Err(config(format!("report output/exclusion is inside {key}: {relative}")));
                    }
                }
            }
        }
    }
    Ok(())
}
pub(crate) fn validate(project: &Path, relative: &str) -> Result<()> {
    location(project, relative)?;
    let target = within(project, relative)?;
    if target.exists() {
        if !target.is_file() {
            return Err(config("report output/exclusion is a directory"));
        }
        let report: Value = read_json(&target).configuration(format_args!(
            "refusing to overwrite/exclude non-report JSON: {relative}"
        ))?;
        ensure_family_report(&report).configuration(format_args!(
            "refusing to overwrite/exclude unknown business JSON: {relative}"
        ))?;
    }
    Ok(())
}
pub(crate) fn ensure_family_report(report: &Value) -> Result<()> {
    let fields = [
        "schema",
        "tool",
        "bundle_id",
        "run_id",
        "task_id",
        "project",
        "report_root",
        "source_revision",
        "source_digest",
        "config_digest",
        "status",
        "started_at",
        "duration_ms",
        "external_versions",
        "commands",
        "diagnostics",
        "artifacts",
    ];
    if report["schema"] != 1
        || !report["tool"]["name"]
            .as_str()
            .is_some_and(|n| n.starts_with("cpp-infra-"))
        || !fields.iter().all(|f| report.get(f).is_some())
        || !["bundle_id", "run_id", "task_id", "project", "report_root", "started_at"]
            .iter()
            .all(|field| report[*field].as_str().is_some_and(|s| !s.is_empty()))
        || report["tool"]["version"]
            .as_str()
            .is_none_or(|v| semver::Version::parse(v).is_err())
        || !(report["tool"]["revision"].is_null()
            || report["tool"]["revision"]
                .as_str()
                .is_some_and(|v| v.len() == 40 && v.bytes().all(|b| b.is_ascii_hexdigit())))
        || !["source_digest", "config_digest"].iter().all(|field| {
            report[*field]
                .as_str()
                .is_some_and(|s| crate::util::valid_digest(s).is_ok())
                || (report["status"] != "passed" && report[*field].is_null())
        })
        || report["duration_ms"].as_u64().is_none()
        || !report["external_versions"].is_object()
        || !["commands", "diagnostics", "artifacts"]
            .iter()
            .all(|field| report[*field].is_array())
        || !matches!(
            report["status"].as_str(),
            Some("passed" | "failed" | "error" | "cancelled" | "skipped")
        )
    {
        return Err(config("invalid public family report"));
    }
    Ok(())
}

pub(crate) fn merge(project: &Path, inherited: &[String], output: Option<&Path>) -> Result<Vec<String>> {
    let mut paths = BTreeSet::new();
    for path in inherited {
        validate(project, path)?;
        paths.insert(path.clone());
    }
    if let Some(output) = output {
        let output = absolute_output(output)?;
        if let Ok(relative) = output.strip_prefix(project) {
            let first = relative.components().next();
            if !matches!(first,Some(Component::Normal(name))if name=="build"||name=="target") {
                let relative = relative
                    .to_str()
                    .required("report path is not UTF-8")?
                    .replace('\\', "/");
                validate(project, &relative)?;
                paths.insert(relative);
            }
        }
    }
    Ok(paths.into_iter().collect())
}
pub(crate) fn child_output(cli: &Cli, project: &Path) -> Result<Option<PathBuf>> {
    let args = match &cli.command {
        Some(Action::Run { args, .. } | Action::Latest { args, .. }) => args,
        _ => return Ok(None),
    };
    let mut output = None;
    let mut index = 0;
    while index < args.len() {
        let value = if args[index] == "--report" {
            index += 1;
            Some(args.get(index).required("child --report needs a path")?.as_str())
        } else {
            args[index].strip_prefix("--report=")
        };
        if let Some(value) = value {
            if output.is_some() {
                return Err(config("child --report specified more than once"));
            }
            let path = Path::new(value);
            output = Some(if path.is_absolute() {
                path.to_path_buf()
            } else {
                project.join(path)
            });
        }
        index += 1;
    }
    Ok(output)
}
pub(crate) fn child_args(args: &[String]) -> Vec<String> {
    let mut result = Vec::new();
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--report" {
            index += 2;
        } else if args[index].starts_with("--report=") {
            index += 1;
        } else {
            result.push(args[index].clone());
            index += 1;
        }
    }
    result
}
