// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
use std::cell::Cell;
use std::fmt;

use crate::error_context::ErrorContext;

struct Diagnostic<'a>(&'a Cell<usize>);
impl fmt::Display for Diagnostic<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.set(self.0.get() + 1);
        formatter.write_str("context")
    }
}

#[test]
fn test_error_context_preserves_lazy_formatting_and_error_classification() {
    let calls = Cell::new(0);
    let success = || Ok::<_, Diagnostic<'_>>(7);
    assert_eq!(success().configuration(Diagnostic(&calls)).expect("success"), 7);
    assert_eq!(success().configuration_context(Diagnostic(&calls)).expect("success"), 7);
    assert_eq!(success().configuration_display().expect("success"), 7);
    assert_eq!(
        success().infrastructure_context(Diagnostic(&calls)).expect("success"),
        7
    );
    assert_eq!(success().infrastructure_display().expect("success"), 7);
    assert_eq!(calls.get(), 0, "successful conversions never format diagnostics");

    let error = Err::<(), _>(Diagnostic(&calls))
        .configuration("replacement")
        .expect_err("error");
    assert_eq!(
        calls.get(),
        0,
        "replacement diagnostics do not format a discarded cause"
    );
    assert_eq!((error.exit_code(), error.to_string()), (2, "replacement".to_owned()));

    let error = Err::<(), _>(Diagnostic(&calls))
        .configuration_context(Diagnostic(&calls))
        .expect_err("error");
    assert_eq!(calls.get(), 2, "context and cause are formatted exactly once");
    assert_eq!(
        (error.exit_code(), error.to_string()),
        (2, "context: context".to_owned())
    );

    let error = Err::<(), _>("cause").configuration_display().expect_err("error");
    assert_eq!((error.exit_code(), error.to_string()), (2, "cause".to_owned()));
    let error = Err::<(), _>("cause")
        .infrastructure_context("start program")
        .expect_err("error");
    assert_eq!(
        (error.exit_code(), error.to_string()),
        (3, "start program: cause".to_owned())
    );
    let error = Err::<(), _>("cause").infrastructure_display().expect_err("error");
    assert_eq!((error.exit_code(), error.to_string()), (3, "cause".to_owned()));
}
