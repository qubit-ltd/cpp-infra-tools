// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Strict catalog and stable channel protocol.
use std::collections::BTreeMap;
use std::collections::BTreeSet;

use serde::Deserialize;
use serde::Serialize;

use crate::error_context::ErrorContext;
use crate::locked_tool::LockedTool;
use crate::required::Required;
use crate::tools_error::Result;
use crate::tools_error::config;
use crate::util::safe_relative;
use crate::util::valid_digest;
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Catalog {
    pub schema: u32,
    pub bundle_id: String,
    pub protocol_major: u32,
    pub tools: Vec<LockedTool>,
    #[serde(default)]
    pub packages: Vec<Package>,
    #[serde(default)]
    pub external_tools: BTreeMap<String, ExternalTool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_sha256: Option<String>,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Package {
    pub owner: String,
    pub url: String,
    pub sha256: String,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExternalTool {
    pub program: String,
    pub version: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub tasks: Vec<String>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Channel {
    pub schema: u32,
    pub bundle_id: String,
    pub catalog: Reference,
    #[serde(default)]
    pub bundles: BTreeMap<String, Reference>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Reference {
    pub url: String,
    pub sha256: String,
}
impl Catalog {
    pub fn validate(&self) -> Result<()> {
        if self.schema != 1 || self.protocol_major != 1 || self.bundle_id.is_empty() {
            return Err(config("unsupported catalog schema/protocol or empty bundle_id"));
        }
        if self.tools.is_empty() {
            return Err(config("catalog has no tools"));
        }
        let mut names = BTreeSet::new();
        for tool in &self.tools {
            if !tool.name.starts_with("cpp-infra-")
                || !safe_relative(&tool.name)?.components().count().eq(&1)
                || !names.insert(&tool.name)
            {
                return Err(config("invalid or duplicate tool name"));
            }
            if tool.protocol != 1 || tool.revision.len() != 40 || !tool.revision.bytes().all(|c| c.is_ascii_hexdigit())
            {
                return Err(config(format!("invalid revision or protocol for {}", tool.name)));
            }
            semver::Version::parse(&tool.version).configuration_context(format_args!("invalid tool version"))?;
            for asset in tool.assets.values() {
                valid_digest(&asset.sha256)?;
                valid_digest(&asset.executable_sha256)?;
                let binary = safe_relative(&asset.binary)?;
                if binary.components().count() != 1 || !["raw", "tar.gz", "zip"].contains(&asset.format.as_str()) {
                    return Err(config("unsupported asset binary or format"));
                }
            }
            if let Some(source) = &tool.source {
                valid_digest(&source.sha256)?;
                valid_digest(&source.cargo_lock_sha256)?;
                if let Some(digest) = &source.executable_sha256 {
                    valid_digest(digest)?;
                }
            }
        }
        let mut owners = BTreeSet::new();
        for package in &self.packages {
            valid_digest(&package.sha256)?;
            if !names.contains(&package.owner) || !owners.insert(&package.owner) {
                return Err(config("invalid or duplicate package owner"));
            }
        }
        for external in self.external_tools.values() {
            semver::VersionReq::parse(&external.version)
                .configuration_context(format_args!("invalid external tool version constraint"))?;
            if external.program.is_empty() {
                return Err(config("external program is empty"));
            }
        }
        Ok(())
    }
    pub fn tool(&self, name: &str) -> Result<&LockedTool> {
        self.tools
            .iter()
            .find(|t| t.name == name)
            .required(format_args!("tool {name} is not in the pinned snapshot"))
    }
}
