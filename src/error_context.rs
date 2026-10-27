// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Preserve error classifications while formatting diagnostics only on failure.
use std::fmt::Display;

use crate::tools_error::Result;
use crate::tools_error::config;
use crate::tools_error::infra;

pub(crate) trait ErrorContext<T> {
    type Cause;

    fn configuration(self, message: impl Display) -> Result<T>;
    fn configuration_context(self, context: impl Display) -> Result<T>
    where
        Self::Cause: Display;
    fn configuration_display(self) -> Result<T>
    where
        Self::Cause: Display;
    fn infrastructure_context(self, context: impl Display) -> Result<T>
    where
        Self::Cause: Display;
    fn infrastructure_display(self) -> Result<T>
    where
        Self::Cause: Display;
}

impl<T, E> ErrorContext<T> for std::result::Result<T, E> {
    type Cause = E;

    fn configuration(self, message: impl Display) -> Result<T> {
        match self {
            Ok(value) => Ok(value),
            Err(_) => Err(config(message.to_string())),
        }
    }

    fn configuration_context(self, context: impl Display) -> Result<T>
    where
        E: Display,
    {
        match self {
            Ok(value) => Ok(value),
            Err(error) => Err(config(format!("{context}: {error}"))),
        }
    }

    fn configuration_display(self) -> Result<T>
    where
        E: Display,
    {
        match self {
            Ok(value) => Ok(value),
            Err(error) => Err(config(error.to_string())),
        }
    }

    fn infrastructure_context(self, context: impl Display) -> Result<T>
    where
        E: Display,
    {
        match self {
            Ok(value) => Ok(value),
            Err(error) => Err(infra(format!("{context}: {error}"))),
        }
    }

    fn infrastructure_display(self) -> Result<T>
    where
        E: Display,
    {
        match self {
            Ok(value) => Ok(value),
            Err(error) => Err(infra(error.to_string())),
        }
    }
}
