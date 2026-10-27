// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
use std::fs;

use tempfile::TempDir;

use crate::profile_selection::selected;

#[test]
fn test_profiles_are_typed_and_missing_fields_never_panic() {
    let root = TempDir::new().expect("project");
    for (role, args, configuration) in [
        ("analyze", vec!["plan"], "schema=1\n"),
        ("coverage", vec!["collect"], "schema=1\nprofile=5\n"),
        (
            "dependency",
            vec!["plan", "--context", "unknown"],
            "schema=1\n[contexts.debug]\nprofile='debug'\n",
        ),
    ] {
        let directory = root.path().join(format!(".infra/{role}"));
        fs::create_dir_all(&directory).expect("config directory");
        fs::write(directory.join("config.toml"), configuration).expect("config");
        let args = args.into_iter().map(str::to_owned).collect::<Vec<_>>();
        assert_eq!(
            selected(root.path(), &format!("cpp-infra-{role}"), &args)
                .expect_err("invalid profile")
                .exit_code(),
            2
        );
    }
}

#[test]
fn test_profileless_queries_and_missing_business_configuration() {
    let root = TempDir::new().expect("project");
    for role in ["analyze", "coverage"] {
        let tool = format!("cpp-infra-{role}");
        assert_eq!(
            selected(root.path(), &tool, &["plan".into()])
                .expect_err("missing config")
                .exit_code(),
            2
        );
        for args in [vec!["--version", "--json"], vec!["plan", "--help"]] {
            let args = args.into_iter().map(str::to_owned).collect::<Vec<_>>();
            assert!(selected(root.path(), &tool, &args).expect("introspection").is_none());
        }
    }
    for tool in ["cpp-infra-style", "cpp-infra-ci", "cpp-infra-pages"] {
        assert!(
            selected(root.path(), tool, &["plan".into()])
                .expect("profileless role")
                .is_none()
        );
    }
    assert!(
        selected(
            root.path(),
            "cpp-infra-dependency",
            &["sync".into(), "--dry-run".into()]
        )
        .expect("global sync")
        .is_none()
    );
}
