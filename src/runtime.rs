// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Timed child processes with logs and cancellation propagation.
use std::collections::BTreeMap;
use std::fs;
use std::fs::File;
use std::path::Path;
use std::process::Command;
use std::process::Stdio;
use std::time::Duration;
use std::time::Instant;

use serde_json::Value;
use serde_json::json;

use crate::error_context::ErrorContext;
use crate::tools_error::Result;
use crate::tools_error::infra;
use crate::util::cancelled;
use crate::util::unique;
pub(crate) fn command(
    program: &Path,
    args: &[String],
    cwd: &Path,
    environment: &BTreeMap<String, String>,
    log_root: &Path,
    timeout: Duration,
    records: &mut Vec<Value>,
) -> Result<(i32, Vec<u8>, Vec<u8>)> {
    cancelled()?;
    fs::create_dir_all(log_root)?;
    let label = unique();
    let stdout_path = log_root.join(format!("{label}.stdout.log"));
    let stderr_path = log_root.join(format!("{label}.stderr.log"));
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .envs(environment)
        .stdout(Stdio::from(File::create(&stdout_path)?))
        .stderr(Stdio::from(File::create(&stderr_path)?));
    if program
        .file_name()
        .is_some_and(|name| name == "git" || name == "git.exe" || name == "cargo" || name == "cargo.exe")
    {
        isolate_git(&mut command);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let start = Instant::now();
    let mut child = command
        .spawn()
        .infrastructure_context(format_args!("start {}", program.display()))?;
    let result = loop {
        if cancelled().is_err() || start.elapsed() > timeout {
            #[cfg(unix)]
            {
                let _ = Command::new("kill")
                    .args(["-TERM", "--", &format!("-{}", child.id())])
                    .status();
            }
            #[cfg(windows)]
            {
                let _ = Command::new("taskkill")
                    .args(["/PID", &child.id().to_string(), "/T", "/F"])
                    .status();
            }
            let _ = child.kill();
            let _ = child.wait();
            break if cancelled().is_err() {
                Err(crate::ToolsError::Cancelled)
            } else {
                Err(infra(format!("command timed out: {}", program.display())))
            };
        }
        if let Some(status) = child.try_wait()? {
            break Ok(status.code().unwrap_or(1));
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let exit_code = result.as_ref().ok().copied();
    let environment: BTreeMap<_, _> = environment
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                if ["TOKEN", "PASSWORD", "SECRET", "KEY"]
                    .iter()
                    .any(|s| key.to_ascii_uppercase().contains(s))
                {
                    "[REDACTED]".to_owned()
                } else {
                    value.clone()
                },
            )
        })
        .collect();
    records.push(json!({"program":program,"args":args,"cwd":cwd,"env":environment,"exit_code":exit_code,"duration_ms":start.elapsed().as_millis(),"log":stderr_path}));
    Ok((result?, fs::read(stdout_path)?, fs::read(stderr_path)?))
}

/// Retry only transient dependency-download failures, preserving the pinned
/// inputs.
pub(crate) fn command_network(
    program: &Path,
    args: &[String],
    cwd: &Path,
    environment: &BTreeMap<String, String>,
    log_root: &Path,
    timeout: Duration,
    records: &mut Vec<Value>,
) -> Result<(i32, Vec<u8>, Vec<u8>)> {
    for attempt in 0..4 {
        let output = command(program, args, cwd, environment, log_root, timeout, records)?;
        if output.0 == 0 {
            return Ok(output);
        }
        let error = String::from_utf8_lossy(&output.2).to_ascii_lowercase();
        let transient = !["404", "not found", "checksum", "sha-256"]
            .iter()
            .any(|s| error.contains(s))
            && [
                "timed out",
                "connection reset",
                "could not resolve host",
                "http 502",
                "http 503",
                "connection refused",
                "temporary failure",
            ]
            .iter()
            .any(|s| error.contains(s));
        if !transient {
            return Ok(output);
        }
        if attempt == 3 {
            return Err(infra(format!(
                "dependency download failed after four attempts: {}",
                String::from_utf8_lossy(&output.2)
            )));
        }
        for _ in 0..(20 << attempt) {
            cancelled()?;
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    Err(infra("dependency download retries exhausted"))
}

/// Keep cache Git operations scoped to their explicit cwd, not caller
/// repository overrides.
pub(crate) fn isolate_git(command: &mut Command) {
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    ] {
        command.env_remove(key);
    }
}
