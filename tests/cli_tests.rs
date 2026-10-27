// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
use std::fs;
use std::process::Command;
use std::process::Output;

use serde_json::Value;
use tempfile::TempDir;

fn invoke(project: &TempDir, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cpp-infra-tools"))
        .arg("--project")
        .arg(project.path())
        .args(args)
        .output()
        .expect("run tools")
}

#[test]
fn test_version_json_has_protocol_and_unpublished_revision() {
    let project = TempDir::new().expect("temporary project");
    let output = invoke(&project, &["--version", "--json"]);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let version: Value = serde_json::from_slice(&output.stdout).expect("version JSON");
    assert_eq!(version["name"], "cpp-infra-tools");
    assert_eq!(version["protocol"], 1);
    assert!(version["revision"].is_null());
}

#[test]
fn test_init_dry_run_is_local_and_does_not_mutate_project() {
    let project = TempDir::new().expect("temporary project");
    fs::write(
        project.path().join("CMakeLists.txt"),
        "cmake_minimum_required(VERSION 3.25)\nproject(example)\n",
    )
    .expect("CMake project");
    let output = invoke(&project, &["--offline", "init", "--dry-run"]);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let plan: Value = serde_json::from_slice(&output.stdout).expect("plan JSON");
    assert!(
        plan["files"]
            .as_array()
            .expect("planned files")
            .iter()
            .any(|p| p == ".infra/bootstrap.py")
    );
    assert!(!project.path().join(".infra").exists());
}

#[test]
fn test_check_missing_snapshot_reports_configuration_error() {
    let project = TempDir::new().expect("temporary project");
    let report = project.path().join("report.json");
    let output = invoke(&project, &["--report", report.to_str().expect("path"), "check"]);
    assert_eq!(output.status.code(), Some(2));
    let report: Value = serde_json::from_slice(&fs::read(report).expect("error report")).expect("JSON report");
    assert_eq!(report["status"], "error");
    assert_eq!(report["schema"], 1);
    assert!(!report["diagnostics"].as_array().expect("diagnostics").is_empty());
}

#[test]
fn test_init_requires_real_cpp_project() {
    let project = TempDir::new().expect("temporary project");
    let output = invoke(&project, &["init", "--dry-run"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("CMakeLists.txt"));
}

#[test]
fn test_uninitialized_doctor_and_status_are_configuration_errors() {
    let project = TempDir::new().expect("project");
    for command in ["doctor", "status"] {
        let output = invoke(&project, &[command]);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{command}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn test_help_version_and_invalid_arguments_do_not_initialize_a_project() {
    let project = TempDir::new().expect("project");
    for (arguments, expected_code, message) in [
        (vec!["--help"], 0, "Track latest main"),
        (vec!["--version"], 0, "cpp-infra-tools 0.1.0"),
        (vec!["--unknown-argument"], 2, "unexpected argument"),
    ] {
        let result = invoke(&project, &arguments);
        assert_eq!(result.status.code(), Some(expected_code));
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(text.contains(message), "{text}");
        assert_eq!(fs::read_dir(project.path()).expect("project contents").count(), 0);
    }
}

#[test]
fn test_filesystem_error_preserves_a_non_directory_build_path() {
    let project = TempDir::new().expect("project");
    let build = project.path().join("build");
    fs::write(&build, "existing user data").expect("build path obstruction");
    let result = invoke(&project, &["check"]);
    assert_eq!(
        result.status.code(),
        Some(3),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!result.stderr.is_empty());
    assert_eq!(
        fs::read_to_string(build).expect("preserved obstruction"),
        "existing user data"
    );
}
