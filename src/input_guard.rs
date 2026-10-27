// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Prove explicit mutation scope instead of accepting arbitrary post-run
//! digests.
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;
use std::path::Component;
use std::path::Path;

use globset::Glob;
use globset::GlobSet;
use globset::GlobSetBuilder;
use serde_json::Value;
use serde_json::json;
use walkdir::WalkDir;

use crate::error_context::ErrorContext;
use crate::report::Report;
use crate::required::Required;
use crate::tools_error::Result;
use crate::tools_error::config;
use crate::util::config_location;
use crate::util::digests;
use crate::util::hash;
use crate::util::is_executable;
use crate::util::safe_relative;
use crate::util::tracked_config;
#[derive(Clone, PartialEq)]
struct Fingerprint {
    digest: String,
    executable: bool,
    symlink: bool,
}
pub(crate) struct InputGuard {
    before: BTreeMap<String, Fingerprint>,
    allowed: BTreeSet<String>,
    create: BTreeSet<String>,
    source_before: String,
    config_before: String,
}
fn path_text(path: &Path) -> Result<String> {
    let name = path
        .components()
        .map(|part| match part {
            Component::Normal(value) => value.to_str().required("input path is not UTF-8"),
            _ => Err(config("input path alias")),
        })
        .collect::<Result<Vec<_>>>()?
        .join("/");
    safe_relative(&name)?;
    Ok(name)
}
fn snapshot(project: &Path, excluded: &[String]) -> Result<BTreeMap<String, Fingerprint>> {
    let mut result = BTreeMap::new();
    for item in WalkDir::new(project)
        .follow_links(false)
        .into_iter()
        .filter_entry(|item| {
            let Ok(relative) = item.path().strip_prefix(project) else {
                return false;
            };
            if relative.as_os_str().is_empty() {
                return true;
            }
            if item.depth() == 1 && [".git", "build", "target"].contains(&item.file_name().to_str().unwrap_or("")) {
                return false;
            }
            ![".infra/.tmp", ".infra/transactions", ".infra/.transactions"]
                .iter()
                .any(|dir| relative.starts_with(dir))
        })
    {
        let item = item.configuration_display()?;
        if item.file_type().is_dir() {
            continue;
        }
        let name = path_text(item.path().strip_prefix(project).configuration_display()?)?;
        if excluded.contains(&name) {
            continue;
        }
        let symlink = item.file_type().is_symlink();
        let bytes = if symlink {
            fs::read_link(item.path())?
                .to_str()
                .required("symlink target is not UTF-8")?
                .as_bytes()
                .to_vec()
        } else if item.file_type().is_file() {
            fs::read(item.path())?
        } else {
            return Err(config("special file in tracked input tree"));
        };
        let executable = if symlink { false } else { is_executable(item.path())? };
        result.insert(
            name,
            Fingerprint {
                digest: hash(&bytes),
                executable,
                symlink,
            },
        );
    }
    Ok(result)
}
fn option<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter().enumerate().find_map(|(i, arg)| {
        if arg == flag {
            args.get(i + 1).map(String::as_str)
        } else {
            arg.strip_prefix(flag).and_then(|v| v.strip_prefix('='))
        }
    })
}
fn words(args: &[String]) -> Vec<&str> {
    let mut result = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if ["--project", "--report", "--run-context", "--profile", "--context"].contains(&arg.as_str()) {
            i += 2;
            continue;
        }
        if arg.starts_with('-') {
            i += 1;
            continue;
        }
        result.push(arg.as_str());
        i += 1;
    }
    result
}
fn toml_input(project: &Path, relative: &str) -> Result<toml::Value> {
    toml::from_str(&crate::util::read_configuration_text(&tracked_config(
        project,
        &project.join(relative),
    )?)?)
    .configuration_display()
}
fn strings(value: &toml::Value, key: &str) -> Result<Vec<String>> {
    value
        .get(key)
        .and_then(toml::Value::as_array)
        .required(format_args!("mutation scope needs {key} array"))?
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .required("scope array element is not a string")
        })
        .collect()
}
fn globs(patterns: &[String]) -> Result<GlobSet> {
    let mut result = GlobSetBuilder::new();
    for pattern in patterns {
        result.add(Glob::new(pattern).configuration_display()?);
    }
    result.build().configuration_display()
}
impl InputGuard {
    pub fn capture(project: &Path, tool: &str, args: &[String], expected: &Value) -> Result<Self> {
        let excluded: Vec<String> = expected
            .get("source_excluded_paths")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .configuration_display()?
            .unwrap_or_default();
        let (source_before, config_before) = digests(project, &excluded)?;
        if expected["source_digest"] != source_before || expected["config_digest"] != config_before {
            return Err(config("inputs changed before child execution"));
        }
        let before = snapshot(project, &excluded)?;
        let mut allowed = BTreeSet::new();
        let mut create = BTreeSet::new();
        let commands = words(args);
        let dry = args
            .iter()
            .any(|arg| arg == "--dry-run" || arg.starts_with("--dry-run="));
        if !dry && tool == "cpp-infra-style" && commands.first() == Some(&"fix") {
            let project_config = toml_input(project, ".infra/project.toml")?;
            let style = toml_input(project, ".infra/style/config.toml")?;
            let mut roots = strings(&project_config, "source_roots")?;
            roots.extend(strings(&project_config, "test_roots")?);
            let roots = roots.iter().map(|p| safe_relative(p)).collect::<Result<Vec<_>>>()?;
            let extensions = strings(&style, "extensions")?;
            let include = globs(&strings(&style, "include")?)?;
            let exclude = globs(&strings(&style, "exclude")?)?;
            let mut protected = BTreeSet::from([
                ".infra/project.toml".to_owned(),
                ".infra/style/config.toml".to_owned(),
                ".infra/style/policy/rules.toml".to_owned(),
                ".infra/style/policy/clang-format.yaml".to_owned(),
            ]);
            if let Some(path) = style.get("exceptions").and_then(toml::Value::as_str) {
                config_location(project, &project.join(path))?;
                protected.insert(path.to_owned());
            }
            for (path, state) in &before {
                let relative = Path::new(path);
                if !state.symlink
                    && !protected.contains(path)
                    && !relative.starts_with(".infra")
                    && roots.iter().any(|root| relative.starts_with(root))
                    && relative
                        .extension()
                        .and_then(|v| v.to_str())
                        .is_some_and(|ext| extensions.iter().any(|item| item == ext))
                    && include.is_match(relative)
                    && !exclude.is_match(relative)
                {
                    allowed.insert(path.clone());
                }
            }
        }
        if !dry
            && tool == "cpp-infra-dependency"
            && (commands.first() == Some(&"sync") || commands.get(0..2) == Some(&["lock", "update"]))
        {
            let dependency = toml_input(project, ".infra/dependency/config.toml")?;
            let path = if commands.first() == Some(&"sync") {
                dependency
                    .get("recipe")
                    .and_then(toml::Value::as_str)
                    .filter(|p| Path::new(p).file_name().and_then(|e| e.to_str()) == Some("conanfile.txt"))
                    .map(str::to_owned)
            } else if commands.get(0..2) == Some(&["lock", "update"]) {
                let context = option(args, "--context").required("lock update requires context")?;
                dependency
                    .get("contexts")
                    .and_then(|c| c.get(context))
                    .and_then(|c| c.get("lockfile"))
                    .and_then(toml::Value::as_str)
                    .map(str::to_owned)
            } else {
                None
            };
            if let Some(path) = path {
                safe_relative(&path)?;
                config_location(project, &project.join(&path))?;
                let target = Path::new(&path);
                let mut protected = BTreeSet::from([
                    ".infra/project.toml".to_owned(),
                    ".infra/managed-files.json".to_owned(),
                    ".infra/dependency/config.toml".to_owned(),
                ]);
                if let Some(policy) = dependency.get("policy").and_then(toml::Value::as_str) {
                    protected.insert(policy.to_owned());
                }
                if let Some(contexts) = dependency.get("contexts").and_then(toml::Value::as_table) {
                    for context in contexts.values() {
                        for key in ["build_profile", "host_profile"] {
                            if let Some(profile) = context.get(key).and_then(toml::Value::as_str) {
                                protected.insert(profile.to_owned());
                            }
                        }
                    }
                }
                if commands.get(0..2) == Some(&["lock", "update"]) {
                    if let Some(recipe) = dependency.get("recipe").and_then(toml::Value::as_str) {
                        protected.insert(recipe.to_owned());
                    }
                    let project_config = toml_input(project, ".infra/project.toml")?;
                    for key in ["source_roots", "test_roots", "generated_roots", "third_party_roots"] {
                        for root in strings(&project_config, key)? {
                            if target.starts_with(safe_relative(&root)?) {
                                return Err(config("lock output overlaps a declared project input root"));
                            }
                        }
                    }
                    if target.file_name().and_then(|v| v.to_str()).is_some_and(|name| {
                        name.starts_with("README")
                            || [
                                "CMakeLists.txt",
                                "CMakePresets.json",
                                "CMakeUserPresets.json",
                                "conanfile.py",
                                "conanfile.txt",
                            ]
                            .contains(&name)
                    }) || target.starts_with(".infra")
                        && (!target.starts_with(".infra/dependency")
                            || target.starts_with(".infra/dependency/policy")
                            || target.starts_with(".infra/dependency/profiles"))
                    {
                        return Err(config("lock output overlaps protected configuration"));
                    }
                    create.insert(path.clone());
                }
                if protected.contains(&path) {
                    return Err(config("mutating output overlaps active configuration"));
                }
                allowed.insert(path);
            }
        }
        Ok(Self {
            before,
            allowed,
            create,
            source_before,
            config_before,
        })
    }
    pub fn finish(self, project: &Path, parent: &mut Report, expected: &Value) -> Result<Value> {
        let after = snapshot(project, &parent.source_excluded_paths)?;
        let (source_after, config_after) = digests(project, &parent.source_excluded_paths)?;
        let paths: BTreeSet<_> = self.before.keys().chain(after.keys()).cloned().collect();
        let changed: Vec<_> = paths
            .into_iter()
            .filter(|p| self.before.get(p) != after.get(p))
            .collect();
        parent.data["inputs_before"] = json!({"source_digest":self.source_before,"config_digest":self.config_before});
        parent.data["inputs_after"] = json!({"source_digest":source_after,"config_digest":config_after});
        parent.data["changed_inputs"] = json!(changed);
        for path in &changed {
            let old = self.before.get(path);
            let new = after.get(path);
            let valid = self.allowed.contains(path)
                && new.is_some_and(|new| !new.symlink)
                && match (old, new) {
                    (Some(old), Some(new)) => !old.symlink && old.executable == new.executable,
                    (None, Some(_)) => self.create.contains(path),
                    _ => false,
                };
            if !valid {
                return Err(config(format!(
                    "child changed an input outside its explicit mutation scope: {path}"
                )));
            }
        }
        let mut expected = expected.clone();
        expected["source_digest"] = json!(source_after);
        expected["config_digest"] = json!(config_after);
        Ok(expected)
    }
}
