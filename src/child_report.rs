// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Bind child results to the executed identity and immutable invocation
//! context.
use std::fs;
use std::path::Path;

use serde_json::Value;
use serde_json::json;

use crate::error_context::ErrorContext;
use crate::report::Report;
use crate::required::Required;
use crate::tools_error::Result;
use crate::tools_error::config;
use crate::util::artifact_digest;
use crate::util::atomic_write;
use crate::util::digests;
use crate::util::hash;
use crate::util::read_json;
use crate::util::within;

pub(crate) fn prepare(path: &Path, parent: &mut Report) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    if !fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(config("existing child report must be a non-symlink regular file"));
    }
    let value: Value = read_json(path)?;
    crate::report_paths::ensure_family_report(&value)?;
    let bytes = fs::read(path)?;
    let previous = parent.work.join("previous-child-report.json");
    atomic_write(&previous, &bytes)?;
    parent.artifacts.push(json!({"kind":"previous-tool-report","path":parent.artifact_path(&previous)?,"sha256":hash(&bytes),"required":false}));
    fs::remove_file(path)?;
    Ok(())
}
pub(crate) fn archive(path: &Path, destination: &Path, parent: &mut Report) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    if !fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(config("child report must be a non-symlink regular file"));
    }
    let bytes = fs::read(path)?;
    if path != destination {
        atomic_write(destination, &bytes)?;
    }
    parent.artifacts.push(
        json!({"kind":"tool-report","path":parent.artifact_path(destination)?,"sha256":hash(&bytes),"required":true}),
    );
    Ok(())
}
pub(crate) fn validate(path: &Path, tool: &Value, expected: &Value) -> Result<()> {
    if !path.is_file() {
        return Err(config("successful child did not write the required report"));
    }
    if fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(config("child report is a symlink"));
    }
    let report: Value = read_json(path)?;
    crate::report_paths::ensure_family_report(&report)?;
    if report["status"] != "passed" {
        return Err(config("successful child reported a non-passed status"));
    }
    for key in ["name", "version", "revision"] {
        if report["tool"][key] != tool[key] {
            return Err(config(format!("child report tool {key} mismatch")));
        }
    }
    if report["tool"].get("protocol").is_some_and(|p| p != &tool["protocol"]) {
        return Err(config("child report tool protocol mismatch"));
    }
    for key in [
        "schema",
        "bundle_id",
        "run_id",
        "task_id",
        "project",
        "report_root",
        "source_digest",
        "config_digest",
    ] {
        if report[key] != expected[key] {
            return Err(config(format!("child report context {key} mismatch")));
        }
    }
    for profile in [report.get("profile"), report.pointer("/data/profile")]
        .into_iter()
        .flatten()
    {
        if profile != &expected["profile"] {
            return Err(config("child report context profile mismatch"));
        }
    }
    let excluded: Vec<String> = report
        .get("source_excluded_paths")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .configuration_display()?
        .unwrap_or_default();
    let expected_excluded: Vec<String> = expected
        .get("source_excluded_paths")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .configuration_display()?
        .unwrap_or_default();
    let project = Path::new(expected["project"].as_str().required("invalid expected project")?);
    if crate::report_paths::merge(project, &excluded, None)? != expected_excluded {
        return Err(config("child report source exclusion list mismatch"));
    }
    let (source, configuration) = digests(project, &excluded)?;
    if report["source_digest"] != source || report["config_digest"] != configuration {
        return Err(config("child report source/config digest mismatch"));
    }
    let root = Path::new(
        expected["report_root"]
            .as_str()
            .required("invalid expected report root")?,
    );
    for artifact in report["artifacts"]
        .as_array()
        .required("child artifacts must be an array")?
    {
        if artifact["kind"].as_str().is_none() {
            return Err(config("artifact kind must be a string"));
        }
        let relative = artifact["path"].as_str().required("artifact path must be relative")?;
        let artifact_path = within(root, relative)?;
        let required = artifact["required"]
            .as_bool()
            .required("artifact required must be boolean")?;
        let digest = artifact["sha256"].as_str().required("artifact SHA must be a string")?;
        crate::util::valid_digest(digest)?;
        if !artifact_path.exists() {
            if required {
                return Err(config(format!("required artifact missing: {relative}")));
            }
            continue;
        }
        if artifact_digest(&artifact_path)? != digest {
            return Err(config(format!("artifact SHA-256 mismatch: {relative}")));
        }
    }
    Ok(())
}
