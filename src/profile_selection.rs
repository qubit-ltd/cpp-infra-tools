// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Resolve the business profile used by both preparation and child transport.
use std::fs;
use std::path::Path;

use crate::error_context::ErrorContext;
use crate::required::Required;
use crate::tools_error::Result;
use crate::tools_error::config;

fn option<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    let mut words = args.iter();
    while let Some(word) = words.next() {
        if word == "--" {
            break;
        }
        if word == flag {
            return words.next().map(String::as_str);
        }
        if let Some(value) = word.strip_prefix(&format!("{flag}=")) {
            return Some(value);
        }
    }
    None
}

fn command(args: &[String]) -> Option<&str> {
    let mut words = args.iter();
    while let Some(word) = words.next() {
        if ["--project", "--report", "--run-context"].contains(&word.as_str()) {
            words.next();
        } else if !word.starts_with('-') {
            return Some(word);
        }
    }
    None
}

fn configuration(project: &Path, role: &str) -> Result<toml::Value> {
    let path = project.join(format!(".infra/{role}/config.toml"));
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => {
            return Err(config(format!(
                "configuration is not a regular file: {}",
                path.display()
            )));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(config(format!("missing configuration: {}", path.display())));
        }
        Err(error) => return Err(error.into()),
    }
    let bytes = fs::read(crate::util::tracked_config(project, &path)?)?;
    let text = std::str::from_utf8(&bytes).configuration_context(format_args!("configuration {}", path.display()))?;
    toml::from_str(text).configuration_display()
}

pub(crate) fn prewarm(project: &Path, tool: &str) -> Result<Option<String>> {
    let role = match tool {
        "cpp-infra-analyze" => "analyze",
        "cpp-infra-coverage" => "coverage",
        _ => return Ok(None),
    };
    if project.join(format!(".infra/{role}/config.toml")).exists() {
        selected(project, tool, &["plan".to_owned()])
    } else {
        Ok(None)
    }
}

pub(crate) fn selected(project: &Path, tool: &str, args: &[String]) -> Result<Option<String>> {
    if crate::introspection::classify(tool, args).is_some() {
        return Ok(None);
    }
    let value = match tool {
        "cpp-infra-analyze" => {
            if !matches!(command(args), Some("plan" | "check")) && !args.is_empty() {
                return Ok(None);
            }
            if let Some(profile) = option(args, "--profile") {
                profile.to_owned()
            } else {
                configuration(project, "analyze")?
                    .get("profile")
                    .and_then(toml::Value::as_str)
                    .required("analyze config profile missing")?
                    .to_owned()
            }
        }
        "cpp-infra-coverage" => configuration(project, "coverage")?
            .get("profile")
            .and_then(toml::Value::as_str)
            .required("coverage config profile missing")?
            .to_owned(),
        "cpp-infra-verify" => match option(args, "--profile") {
            Some(profile) => profile.to_owned(),
            None => return Ok(None),
        },
        "cpp-infra-dependency" => {
            if command(args) == Some("sync") || args.is_empty() {
                return Ok(None);
            }
            let Some(context) = option(args, "--context") else {
                return Ok(None);
            };
            configuration(project, "dependency")?
                .get("contexts")
                .and_then(|contexts| contexts.get(context))
                .and_then(|value| value.get("profile"))
                .and_then(toml::Value::as_str)
                .required("dependency context profile missing")?
                .to_owned()
        }
        _ => return Ok(None),
    };
    if value.is_empty() {
        return Err(config("selected business profile is empty"));
    }
    Ok(Some(value))
}
