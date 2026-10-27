// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Command-line surface shared with bundle wrappers.
use std::path::PathBuf;

use clap::Parser;
use clap::Subcommand;

#[derive(Debug, Parser)]
#[command(
    name = "cpp-infra-tools",
    about = "Track latest main tools with verified per-run revisions",
    disable_version_flag = true
)]
pub(crate) struct Cli {
    #[arg(long, global = true, default_value = ".")]
    pub project: PathBuf,
    #[arg(long, global = true)]
    pub report: Option<PathBuf>,
    #[arg(long, global = true)]
    pub offline: bool,
    #[arg(long, global = true)]
    pub run_context: Option<PathBuf>,
    #[arg(long, global = true)]
    pub from_source: bool,
    #[arg(long, global = true, help = "Use explicit stable catalog/lock compatibility mode")]
    pub locked: bool,
    #[arg(long, global = true)]
    pub version: bool,
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub command: Option<Action>,
}
#[derive(Debug, Subcommand)]
pub(crate) enum Action {
    /// Install the C++ bootstrap and managed configuration from main.
    Init {
        #[arg(long)]
        dry_run: bool,
    },
    /// Transactionally synchronize managed files from main.
    Update {
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        bundle: Option<String>,
    },
    /// Verify installed managed files using only local state.
    Check,
    /// Compare installed configuration with current main revisions.
    Status,
    /// Check external tool versions required for a task.
    Doctor {
        #[arg(long)]
        task: Option<String>,
        #[arg(long)]
        profile: Option<String>,
    },
    /// Populate the verified executable cache without running a task.
    Prewarm {
        #[arg(long)]
        tool: Vec<String>,
    },
    /// Resolve immutable main revisions for reuse across CI runners.
    Resolve {
        #[arg(long)]
        tool: Vec<String>,
        #[arg(long)]
        output: PathBuf,
    },
    /// Execute the latest main revisions, fixing the group for this invocation.
    Latest {
        #[arg(long)]
        tool: String,
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Execute the selected tool, without PATH fallback.
    Run {
        #[arg(long)]
        tool: String,
        #[arg(last = true)]
        args: Vec<String>,
    },
}
