// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
use std::path::Path;
use std::process::Command;
#[test]
fn test_python_bootstrap_review_boundaries() {
    let result = Command::new("python3")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/bootstrap_review_tests.py"))
        .output()
        .expect("Python bootstrap regressions");
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
}
