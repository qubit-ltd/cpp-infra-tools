// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Concrete errors and process exit semantics.
use thiserror::Error;

/// A configuration, execution, or infrastructure failure.
#[derive(Debug, Error)]
pub enum ToolsError {
    /// Invalid or incompatible project, snapshot, or release metadata.
    #[error("{0}")]
    Configuration(String),
    /// A requested tool or quality gate failed.
    #[error("{0}")]
    Gate(String),
    /// Filesystem, network, or process startup failure.
    #[error("{0}")]
    Infrastructure(String),
    /// Execution was interrupted.
    #[error("cancelled")]
    Cancelled,
}
impl ToolsError {
    /// Return the common CLI protocol exit code.
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Configuration(_) => 2,
            Self::Gate(_) => 1,
            Self::Infrastructure(_) => 3,
            Self::Cancelled => 130,
        }
    }
}
impl From<std::io::Error> for ToolsError {
    fn from(value: std::io::Error) -> Self {
        Self::Infrastructure(value.to_string())
    }
}
pub(crate) type Result<T> = std::result::Result<T, ToolsError>;
pub(crate) fn config(message: impl Into<String>) -> ToolsError {
    ToolsError::Configuration(message.into())
}
pub(crate) fn infra(message: impl Into<String>) -> ToolsError {
    ToolsError::Infrastructure(message.into())
}
