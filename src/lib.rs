// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Bootstrap and maintain C++ infrastructure tools from verified main
//! revisions.
mod archive;
mod asset;
mod bootstrap;
mod cache;
mod catalog;
mod child_report;
mod cli;
mod doctor;
mod download;
mod error_context;
mod input_guard;
mod introspection;
mod locked_tool;
mod main_mode;
mod managed_file;
mod manifest;
mod profile_selection;
mod report;
mod report_paths;
mod required;
mod runtime;
mod snapshot;
mod tools_error;
mod transaction;
mod update;
mod util;

#[cfg(test)]
mod tests;

mod application;
pub use crate::application::execute;
pub use crate::tools_error::ToolsError;
