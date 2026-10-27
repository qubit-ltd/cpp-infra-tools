// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Bounded, digest-verified release downloads.
use std::fs;
use std::time::Duration;

use crate::error_context::ErrorContext;
use crate::tools_error::Result;
use crate::tools_error::config;
use crate::tools_error::infra;
use crate::util::cancelled;
use crate::util::verify_hash;
pub(crate) fn fetch(url: &str, offline: bool) -> Result<Vec<u8>> {
    cancelled()?;
    let parsed = reqwest::Url::parse(url).configuration_context(format_args!("invalid release URL"))?;
    if parsed.scheme() == "file" {
        let path = parsed.to_file_path().configuration("invalid local release URL")?;
        return Ok(fs::read(path)?);
    }
    if offline {
        return Err(infra(format!("offline: unavailable release metadata {url}")));
    }
    if parsed.scheme() != "https"
        && !(parsed.scheme() == "http" && matches!(parsed.host_str(), Some("127.0.0.1" | "localhost" | "::1")))
    {
        return Err(config(
            "release sources require HTTPS (loopback HTTP fixtures are permitted)",
        ));
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .infrastructure_display()?;
    let mut last = String::new();
    for attempt in 0..4 {
        cancelled()?;
        match client.get(parsed.clone()).send() {
            Ok(response) if response.status().is_success() => {
                if response.url().scheme() != parsed.scheme() && response.url().scheme() != "https" {
                    return Err(config("insecure download redirect"));
                }
                return response
                    .bytes()
                    .map(|b| b.to_vec())
                    .infrastructure_context(format_args!("download body"));
            }
            Ok(response) => {
                let status = response.status();
                last = format!("HTTP {status}: {url}");
                if !status.is_server_error() && status.as_u16() != 429 {
                    return Err(infra(last));
                }
            }
            Err(error) => {
                if !error.is_timeout() && !error.is_connect() {
                    return Err(infra(format!("download: {error}")));
                }
                last = error.to_string();
            }
        }
        if attempt < 3 {
            for _ in 0..(20 << attempt) {
                cancelled()?;
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
    Err(infra(format!("download failed after four attempts: {last}")))
}
pub(crate) fn verified(url: &str, digest: &str, offline: bool) -> Result<Vec<u8>> {
    let bytes = fetch(url, offline)?;
    verify_hash(&bytes, digest)?;
    Ok(bytes)
}
