// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Narrow, read-only help/version requests that do not claim a quality gate.
use std::collections::BTreeMap;
use std::io;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use serde_json::Value;
use serde_json::json;

use crate::input_guard::InputGuard;
use crate::report::Report;
use crate::required::Required;
use crate::tools_error::Result;
use crate::tools_error::config;

pub(crate) fn classify(tool: &str, args: &[String]) -> Option<&'static str> {
    let words = args.iter().map(String::as_str).collect::<Vec<_>>();
    let commands: &[&str] = match tool {
        "cpp-infra-style" => &["", "check", "fix"],
        "cpp-infra-ci" => &["", "plan", "check", "matrix", "summarize", "execute"],
        "cpp-infra-verify" => &["", "prepare", "run", "plan"],
        "cpp-infra-dependency" => &["", "prepare", "check", "sync", "lock", "lock update", "plan"],
        "cpp-infra-analyze" => &["", "check", "doctor", "plan"],
        "cpp-infra-coverage" => &["", "collect", "plan"],
        "cpp-infra-pages" => &["", "build", "plan"],
        "cpp-infra-tools" => &[
            "", "init", "update", "check", "status", "doctor", "prewarm", "resolve", "latest", "run",
        ],
        _ => &[],
    };
    if [
        vec!["--version"],
        vec!["--version", "--json"],
        vec!["--json", "--version"],
    ]
    .contains(&words)
    {
        return Some("version");
    }
    if let Some(index) = words.iter().position(|arg| *arg == "--version") {
        let suffix = &words[index..];
        if (suffix == ["--version"] || suffix == ["--version", "--json"])
            && commands.contains(&words[..index].join(" ").as_str())
        {
            return Some("version");
        }
    }
    if !matches!(words.last(), Some(&"--help" | &"-h")) {
        return None;
    }
    let path = words[..words.len() - 1].join(" ");
    commands.contains(&path.as_str()).then_some("help")
}
pub(crate) fn execute(
    binary: &Path,
    args: &[String],
    environment: &BTreeMap<String, String>,
    parent: &mut Report,
    expected: &Value,
    tool: &Value,
    kind: &str,
) -> Result<()> {
    let project = parent.project.clone();
    let guard = InputGuard::capture(
        &project,
        tool["name"].as_str().required("missing introspection tool")?,
        &[],
        expected,
    )?;
    let (code, stdout, stderr) = crate::runtime::command(
        binary,
        args,
        &project,
        environment,
        &parent.work.join("logs"),
        Duration::from_secs(30),
        &mut parent.commands,
    )?;
    parent.data["operation"] = json!("introspection");
    parent.data["introspection"] = json!(kind);
    parent.data["exit_code"] = json!(code);
    guard.finish(&project, parent, expected)?;
    io::stderr().write_all(&stderr)?;
    if code != 0 {
        return Err(match code {
            2 => config("tool rejected introspection arguments"),
            3 => crate::tools_error::infra("tool introspection infrastructure failure"),
            130 => crate::ToolsError::Cancelled,
            _ => crate::ToolsError::Gate(format!("tool introspection exited {code}")),
        });
    }
    if kind == "version" && args.iter().any(|arg| arg == "--json") {
        let identity: Value = crate::util::parse_json(&stdout, "tool version response")?;
        for key in ["name", "version", "revision", "protocol"] {
            if identity[key] != tool[key] {
                return Err(config("introspection version identity mismatch"));
            }
        }
    }
    io::stdout().write_all(&stdout)?;
    Ok(())
}
