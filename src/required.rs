// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Required optional configuration values with lazy diagnostic formatting.
use std::fmt::Display;

use crate::tools_error::Result;
use crate::tools_error::config;

pub(crate) trait Required<T> {
    /// Format the diagnostic only when the required value is absent.
    fn required(self, message: impl Display) -> Result<T>;
}

impl<T> Required<T> for Option<T> {
    fn required(self, message: impl Display) -> Result<T> {
        match self {
            Some(value) => Ok(value),
            None => Err(config(message.to_string())),
        }
    }
}
