// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Explicit managed ownership and initial project configuration entries.
use serde::Deserialize;
use serde::Serialize;

use crate::managed_file::ManagedFile;
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Manifest {
    pub schema: u32,
    #[serde(default)]
    pub owner: Option<String>,
    pub files: Vec<ManagedFile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub project_files: Vec<ManagedFile>,
}
