// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Ownership and content identity of one delivered file.
use serde::Deserialize;
use serde::Serialize;
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManagedFile {
    #[serde(alias = "path")]
    pub target: String,
    pub source: String,
    pub sha256: String,
    pub executable: bool,
}
