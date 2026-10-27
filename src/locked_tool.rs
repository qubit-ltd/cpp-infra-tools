// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Immutable tool identity and supported platform assets.
use std::collections::BTreeMap;

use serde::Deserialize;
use serde::Serialize;

use crate::asset::Asset;
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LockedTool {
    pub name: String,
    pub version: String,
    pub revision: String,
    pub protocol: u32,
    #[serde(default)]
    pub assets: BTreeMap<String, Asset>,
    #[serde(default)]
    pub source: Option<Source>,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Source {
    pub url: String,
    pub sha256: String,
    pub cargo_lock_sha256: String,
    #[serde(default)]
    pub executable_sha256: Option<String>,
}
