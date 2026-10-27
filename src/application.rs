// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! CLI execution and process/report coordination.
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;
use std::fs::OpenOptions;
use std::io;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use clap::Parser;
use serde_json::Value;
use serde_json::json;

use crate::bootstrap;
use crate::cache;
use crate::catalog::Catalog;
use crate::child_report;
use crate::cli::Action;
use crate::cli::Cli;
use crate::doctor;
use crate::input_guard;
use crate::main_mode;
use crate::report::Report;
use crate::report_paths;
use crate::required::Required;
use crate::runtime;
use crate::snapshot;
use crate::tools_error::Result;
use crate::tools_error::ToolsError;
use crate::tools_error::config;
use crate::tools_error::infra;
use crate::transaction;
use crate::update;
use crate::util;

pub(crate) fn version() -> Value {
    json!({"name":"cpp-infra-tools","version":env!("CARGO_PKG_VERSION"),"revision":option_env!("CPP_INFRA_BUILD_REVISION"),"protocol":1})
}

/// Execute the CLI and return its protocol exit code.
///
/// This function reads process arguments and may fetch immutable tool
/// revisions, mutate explicitly managed project files for init/update, or spawn
/// requested tools. Errors are rendered on stderr and, when requested, in the
/// common JSON report.
pub fn execute() -> i32 {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let code = error.exit_code();
            let _ = error.print();
            return code;
        }
    };
    if cli.version {
        if cli.json {
            println!("{}", version());
        } else {
            println!("cpp-infra-tools {}", env!("CARGO_PKG_VERSION"));
        }
        return 0;
    }
    let _ = ctrlc::set_handler(|| util::CANCELLED.store(true, std::sync::atomic::Ordering::SeqCst));
    let project = match cli.project.canonicalize() {
        Ok(path) if path.is_dir() => path,
        Ok(_) => {
            eprintln!("project is not a directory");
            return 2;
        }
        Err(error) => {
            eprintln!("project: {error}");
            return 3;
        }
    };
    let mut report = match Report::new(&project, &cli) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("{error}");
            let code = error.exit_code();
            let fallback = Cli {
                project: cli.project.clone(),
                report: cli.report.clone(),
                offline: cli.offline,
                run_context: None,
                from_source: cli.from_source,
                locked: cli.locked,
                version: false,
                json: cli.json,
                command: None,
            };
            if let Ok(mut report) = Report::new(&project, &fallback)
                && let Err(write) = report.write(&fallback, &Err(error))
            {
                eprintln!("report: {write}");
                return 3;
            }
            return code;
        }
    };
    // Keep the project lock through the final report's input digests and write.
    let mut project_lock = None;
    let result = dispatch(&cli, &project, &mut report, &mut project_lock);
    if let Err(error) = &result {
        eprintln!("{error}");
    }
    if let Err(error) = report.write(&cli, &result) {
        eprintln!("report: {error}");
        return 3;
    }
    match result {
        Ok(()) => 0,
        Err(error) => error.exit_code(),
    }
}
fn dispatch(cli: &Cli, project: &Path, report: &mut Report, project_lock: &mut Option<fs::File>) -> Result<()> {
    let action = cli.command.as_ref().required("a command is required; use --help")?;
    if matches!(action, Action::Init { dry_run: true }) {
        require_cpp(project)?;
        let plan = json!({"schema":1,"dry_run":true,"files":bootstrap::local_files(),"release_files":"discovered only by explicit catalog update","network":false});
        println!("{}", plan);
        report.data = plan;
        return Ok(());
    }
    // Only an explicit mutating update may recover an interrupted transaction.
    fs::create_dir_all(project.join("build"))?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(project.join("build/.cpp-infra-project.lock"))?;
    let mutating = matches!(
        action,
        Action::Init { dry_run: false } | Action::Update { dry_run: false, .. }
    );
    crate::util::acquire_lock(&lock, mutating)?;
    *project_lock = Some(lock);
    if transaction::pending(project)? {
        if matches!(
            action,
            Action::Init { dry_run: false } | Action::Update { dry_run: false, .. }
        ) {
            transaction::recover(project)?;
        } else {
            return Err(config(
                "unfinished transaction; run explicit update to validate and recover it",
            ));
        }
    }
    if cli.run_context.is_some() {
        let _validated = Report::new(project, cli)?;
    }
    if !cli.locked {
        return main_mode::dispatch(cli, project, report, action);
    }
    match action {
        Action::Init { dry_run } => {
            require_cpp(project)?;
            let catalog = update::resolve(None, cli.offline)?;
            report.bundle_id = catalog.bundle_id.clone();
            report.data = update::prepare(project, &catalog, true, *dry_run, cli.offline)?;
            println!("{}", report.data);
        }
        Action::Update { dry_run, bundle } => {
            snapshot::check(project)?;
            main_mode::locked_previous_authority(project, cli.offline, report)?;
            let catalog = update::resolve(bundle.as_deref(), cli.offline)?;
            report.bundle_id = catalog.bundle_id.clone();
            report.data = update::prepare(project, &catalog, false, *dry_run, cli.offline)?;
            println!("{}", report.data);
        }
        Action::Check => {
            let catalog = snapshot::check(project)?;
            report.bundle_id = catalog.bundle_id.clone();
            report.data = json!({"bundle_id":catalog.bundle_id,"valid":true});
            println!("{}", report.data);
        }
        Action::Status => {
            let current = snapshot::check(project)?;
            report.bundle_id = current.bundle_id.clone();
            let latest = update::resolve(None, cli.offline)?;
            report.data = json!({"current":current.bundle_id,"stable":latest.bundle_id,"update_available":current.bundle_id != latest.bundle_id});
            println!("{}", report.data);
        }
        Action::Doctor { task, profile } => {
            let catalog = snapshot::check(project)?;
            report.bundle_id = catalog.bundle_id.clone();
            if let Some(profile) = profile {
                snapshot::validate_profile(project, profile)?;
            }
            report.data = doctor::check(
                &catalog,
                project,
                task.as_deref(),
                &report.work.join("logs"),
                &mut report.commands,
                &mut report.external_versions,
            )?;
            println!("{}", report.data);
        }
        Action::Prewarm { tool } => {
            let catalog = snapshot::check(project)?;
            report.bundle_id = catalog.bundle_id.clone();
            let selected: Vec<_> = if tool.is_empty() {
                catalog.tools.iter().map(|t| t.name.as_str()).collect()
            } else {
                tool.iter().map(String::as_str).collect()
            };
            let mut binaries = Vec::new();
            for name in selected {
                let binary = cache::prepare(catalog.tool(name)?, cli.offline, cli.from_source, &mut report.commands)?;
                binaries.push((name.to_owned(), binary));
            }
            let bin = cache::bundle_bin(&catalog, &binaries)?;
            report.data = json!({"prepared":binaries.iter().map(|(name,_)|name).collect::<Vec<_>>(),"bin_dir":bin});
            println!("{}", report.data);
        }
        Action::Resolve { .. } | Action::Latest { .. } => {
            return Err(config("resolve/latest are available in main mode; omit --locked"));
        }
        Action::Run { tool, args } => {
            let catalog = snapshot::check(project)?;
            report.bundle_id = catalog.bundle_id.clone();
            let inspection = crate::introspection::classify(tool, args);
            let profile = crate::profile_selection::selected(project, tool, args)?;
            let closure = if inspection.is_some() {
                catalog.tool(tool)?;
                BTreeSet::from([tool.clone()])
            } else {
                tool_closure(project, &catalog, tool, profile.as_deref())?
            };
            let mut binaries = Vec::new();
            for name in &closure {
                let binary = cache::prepare(catalog.tool(name)?, cli.offline, cli.from_source, &mut report.commands)?;
                binaries.push((name.clone(), binary));
            }
            let bin = cache::bundle_bin(&catalog, &binaries)?;
            let binary = binaries
                .iter()
                .find(|(name, _)| name == tool)
                .map(|(_, path)| path.clone())
                .required("selected tool is missing")?;
            let context = report.context(profile.as_deref())?;
            let expected_context: Value = util::read_json(&context)?;
            if let Some(kind) = inspection {
                let selected = catalog.tool(tool)?;
                let environment =
                    BTreeMap::from([("CPP_INFRA_BIN_DIR".to_owned(), bin.to_string_lossy().into_owned())]);
                report.data = json!({"mode":"locked","tool":tool});
                return crate::introspection::execute(
                    &binary,
                    args,
                    &environment,
                    report,
                    &expected_context,
                    &json!({"name":selected.name,"version":selected.version,"revision":selected.revision,"protocol":selected.protocol}),
                    kind,
                );
            }
            let artifact_report = report.work.join(format!("{}.json", tool));
            let child_report = report_paths::child_output(cli, project)?.unwrap_or_else(|| artifact_report.clone());
            child_report::prepare(&child_report, report)?;
            let inputs = input_guard::InputGuard::capture(project, tool, args, &expected_context)?;
            let mut child_args = vec![
                "--project".to_owned(),
                project.to_string_lossy().into_owned(),
                "--report".to_owned(),
                child_report.to_string_lossy().into_owned(),
                "--run-context".to_owned(),
                context.to_string_lossy().into_owned(),
            ];
            if cli.offline {
                child_args.push("--offline".to_owned());
            }
            child_args.extend(report_paths::child_args(args));
            let environment = BTreeMap::from([("CPP_INFRA_BIN_DIR".to_owned(), bin.to_string_lossy().into_owned())]);
            let (code, stdout, stderr) = runtime::command(
                &binary,
                &child_args,
                project,
                &environment,
                &report.work.join("logs"),
                Duration::from_secs(3600),
                &mut report.commands,
            )?;
            io::stdout().write_all(&stdout)?;
            io::stderr().write_all(&stderr)?;
            report.data = json!({"tool":tool,"exit_code":code,"child_report":child_report});
            child_report::archive(&child_report, &artifact_report, report)?;
            let expected_context = inputs.finish(project, report, &expected_context)?;
            if code == 0 {
                let selected = catalog.tool(tool)?;
                child_report::validate(
                    &child_report,
                    &json!({"name":selected.name,"version":selected.version,"revision":selected.revision,"protocol":selected.protocol}),
                    &expected_context,
                )?;
            }
            if code != 0 {
                return Err(match code {
                    2 => config(format!("{tool} failed with configuration/protocol error")),
                    3 => infra(format!("{tool} infrastructure failure")),
                    130 => ToolsError::Cancelled,
                    _ => ToolsError::Gate(format!("{tool} exited {code}")),
                });
            }
        }
    }
    Ok(())
}
pub(crate) fn require_cpp(project: &Path) -> Result<()> {
    if !project.join("CMakeLists.txt").is_file() {
        Err(config("C++ initialization requires CMakeLists.txt"))
    } else {
        Ok(())
    }
}
fn tool_closure(project: &Path, catalog: &Catalog, selected: &str, profile: Option<&str>) -> Result<BTreeSet<String>> {
    let tools = main_mode::closure_for_profile(project, selected, profile)?;
    for name in &tools {
        catalog.tool(name)?;
    }
    Ok(tools)
}
