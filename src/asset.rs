// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Platform-specific release asset metadata.
use serde::Deserialize;
use serde::Serialize;
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Asset {
    pub url: String,
    pub sha256: String,
    pub executable_sha256: String,
    pub binary: String,
    #[serde(default = "default_format")]
    pub format: String,
}
fn default_format() -> String {
    "tar.gz".to_owned()
}
