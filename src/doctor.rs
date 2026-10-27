// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Actual external tool version checks against a pinned bundle.
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use serde_json::Value;
use serde_json::json;

use crate::catalog::Catalog;
use crate::error_context::ErrorContext;
use crate::required::Required;
use crate::runtime::command;
use crate::tools_error::Result;
use crate::tools_error::config;
pub(crate) fn check(
    catalog: &Catalog,
    project: &Path,
    task: Option<&str>,
    logs: &Path,
    records: &mut Vec<Value>,
    versions: &mut BTreeMap<String, Value>,
) -> Result<Value> {
    let mut selected = 0;
    for (name, tool) in &catalog.external_tools {
        if task.is_some_and(|task| !tool.tasks.is_empty() && !tool.tasks.iter().any(|t| t == task)) {
            continue;
        }
        selected += 1;
        let args = if tool.args.is_empty() {
            vec!["--version".to_owned()]
        } else {
            tool.args.clone()
        };
        let (code, stdout, stderr) = command(
            Path::new(&tool.program),
            &args,
            project,
            &BTreeMap::new(),
            logs,
            Duration::from_secs(30),
            records,
        )?;
        if code != 0 {
            return Err(config(format!(
                "{} version query failed: {}",
                tool.program,
                String::from_utf8_lossy(&stderr)
            )));
        }
        let output = format!(
            "{}{}",
            String::from_utf8_lossy(&stdout),
            String::from_utf8_lossy(&stderr)
        );
        let actual = output
            .split(|c: char| !c.is_ascii_digit() && c != '.')
            .filter(|piece| piece.contains('.'))
            .find_map(|piece| {
                let parts: Vec<&str> = piece.trim_matches('.').split('.').collect();
                if parts.len() < 2 {
                    return None;
                }
                let normalized = parts.iter().take(3).copied().collect::<Vec<_>>().join(".");
                semver::Version::parse(&if parts.len() == 2 {
                    format!("{normalized}.0")
                } else {
                    normalized
                })
                .ok()
            })
            .required(format_args!("cannot parse version of {}", tool.program))?;
        let constraint = semver::VersionReq::parse(&tool.version).configuration_display()?;
        versions.insert(
            name.clone(),
            json!({"program":tool.program,"required":tool.version,"actual":actual.to_string(),"raw":output.trim()}),
        );
        if !constraint.matches(&actual) {
            return Err(config(format!(
                "{} {} violates pinned constraint {}",
                tool.program, actual, tool.version
            )));
        }
    }
    if selected == 0 {
        return Err(config(
            "no external tool constraints are configured for the selected task",
        ));
    }
    Ok(json!({"checked":selected,"task":task}))
}
