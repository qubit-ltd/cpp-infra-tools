// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
use std::cell::Cell;
use std::fmt;

use crate::required::Required;

struct Diagnostic<'a>(&'a Cell<usize>);
impl fmt::Display for Diagnostic<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.set(self.0.get() + 1);
        formatter.write_str("missing context project")
    }
}

#[test]
fn test_required_formats_diagnostics_only_for_absent_values() {
    let calls = Cell::new(0);
    assert_eq!(Some("value").required(Diagnostic(&calls)).expect("present"), "value");
    assert_eq!(calls.get(), 0);
    let error = None::<&str>
        .required(Diagnostic(&calls))
        .expect_err("required absent value");
    assert_eq!(calls.get(), 1);
    assert_eq!(error.to_string(), "missing context project");
    assert_eq!(error.exit_code(), 2);
}
