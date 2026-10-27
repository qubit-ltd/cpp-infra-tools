// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
use std::fs;
use std::path::Path;
use std::process::Command;
use std::process::Output;

use serde_json::Value;
use serde_json::json;
use sha2::Digest;
use sha2::Sha256;
use tempfile::TempDir;

fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn tree_state(root: &Path) -> Vec<(String, String, Vec<u8>)> {
    let mut pending = vec![root.to_path_buf()];
    let mut state = Vec::new();
    while let Some(path) = pending.pop() {
        let metadata = fs::symlink_metadata(&path).expect("snapshot metadata");
        let name = path
            .strip_prefix(root)
            .expect("snapshot relative path")
            .to_string_lossy()
            .into_owned();
        let kind_and_mode = format!("{:?}:{:?}", metadata.file_type(), metadata.permissions());
        let bytes = if metadata.is_file() {
            fs::read(&path).expect("snapshot contents")
        } else if metadata.file_type().is_symlink() {
            fs::read_link(&path)
                .expect("snapshot link")
                .to_string_lossy()
                .as_bytes()
                .to_vec()
        } else {
            Vec::new()
        };
        if metadata.is_dir() {
            pending.extend(
                fs::read_dir(path)
                    .expect("snapshot directory")
                    .map(|entry| entry.expect("snapshot entry").path()),
            );
        }
        state.push((name, kind_and_mode, bytes));
    }
    state.sort();
    state
}

struct Fixture {
    root: TempDir,
    project: TempDir,
    cache: TempDir,
    catalog: String,
}
impl Fixture {
    fn new(bundle: &str) -> Self {
        let root = TempDir::new().expect("distribution");
        let project = TempDir::new().expect("project");
        let cache = TempDir::new().expect("cache");
        fs::write(
            project.path().join("CMakeLists.txt"),
            "cmake_minimum_required(VERSION 3.25)\nproject(example)\n",
        )
        .expect("CMake");
        fs::create_dir(project.path().join("src")).expect("source root");
        let revision = "1234567890123456789012345678901234567890";
        let executable = format!(
            r#"#!/usr/bin/env python3
import json, os, sys
version = {{"name":"cpp-infra-style","version":"0.1.0","revision":"{revision}","protocol":1}}
if sys.argv[1:] == ["--version", "--json"]:
    print(json.dumps(version))
    sys.exit(0)
args = sys.argv[1:]
context = json.load(open(args[args.index("--run-context")+1]))
report = dict(context, tool=version, status="passed", source_revision=None, started_at="2026-01-01T00:00:00Z", duration_ms=0, external_versions={{}}, commands=[], diagnostics=[], artifacts=[])
with open(args[args.index("--report")+1], "w") as output:
    json.dump(report, output)
print(os.environ["CPP_INFRA_BIN_DIR"])
"#
        );
        let binary = root.path().join("cpp-infra-style");
        fs::write(&binary, executable.as_bytes()).expect("binary");
        let policy = b"schema = 1\nminimum = 1\n";
        let manifest = json!({"schema":1,"owner":"cpp-infra-style","files":[{"target":"policy/default.toml","source":"policy.toml","sha256":digest(policy),"executable":false}],"project_files":[{"target":"config.toml","source":"config.toml","sha256":digest(b"schema = 1\n"),"executable":false}]});
        let archive_path = root.path().join("public.tar.gz");
        let file = fs::File::create(&archive_path).expect("archive");
        let gzip = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut tar = tar::Builder::new(gzip);
        for (name, bytes) in [
            ("manifest.json", serde_json::to_vec(&manifest).expect("manifest")),
            ("policy.toml", policy.to_vec()),
            ("config.toml", b"schema = 1\n".to_vec()),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, name, bytes.as_slice()).expect("tar entry");
        }
        tar.into_inner().expect("tar finish").finish().expect("gzip finish");
        let host = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
        let catalog = json!({"schema":1,"bundle_id":bundle,"protocol_major":1,"tools":[{"name":"cpp-infra-style","version":"0.1.0","revision":revision,"protocol":1,"assets":{host:{"url":format!("file://{}",binary.display()),"sha256":digest(executable.as_bytes()),"executable_sha256":digest(executable.as_bytes()),"binary":"cpp-infra-style","format":"raw"}}}],"packages":[{"owner":"cpp-infra-style","url":format!("file://{}",archive_path.display()),"sha256":digest(&fs::read(&archive_path).expect("archive bytes"))}],"external_tools":{}});
        let catalog_file = root.path().join("catalog.json");
        fs::write(&catalog_file, serde_json::to_vec(&catalog).expect("catalog")).expect("catalog file");
        let catalog = format!("file://{}", catalog_file.display());
        Self {
            root,
            project,
            cache,
            catalog,
        }
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_cpp-infra-tools"))
            .arg("--locked")
            .arg("--project")
            .arg(self.project.path())
            .env("CPP_INFRA_CATALOG_URL", &self.catalog)
            .env("CPP_INFRA_CACHE_DIR", self.cache.path())
            .args(args)
            .output()
            .expect("tools")
    }
    fn init(&self) {
        let result = self.run(&["init"]);
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    }
    fn catalog_value(&self) -> Value {
        serde_json::from_slice(&fs::read(self.root.path().join("catalog.json")).expect("catalog file"))
            .expect("catalog")
    }
    fn write_catalog(&self, value: &Value) {
        fs::write(
            self.root.path().join("catalog.json"),
            serde_json::to_vec(value).expect("catalog JSON"),
        )
        .expect("catalog write");
    }
    fn replace_binary(&self, bytes: &[u8]) {
        fs::write(self.root.path().join("cpp-infra-style"), bytes).expect("replace asset");
        let mut catalog = self.catalog_value();
        for asset in catalog["tools"][0]["assets"]
            .as_object_mut()
            .expect("assets")
            .values_mut()
        {
            asset["sha256"] = json!(digest(bytes));
            asset["executable_sha256"] = json!(digest(bytes));
        }
        self.write_catalog(&catalog);
    }
    fn lock(&self) -> Value {
        serde_json::from_slice(&fs::read(self.project.path().join(".infra/tools/tools.lock.json")).expect("lock"))
            .expect("lock json")
    }
}

#[test]
fn test_init_installs_template_and_verified_policy_without_cpp_source_changes() {
    let fixture = Fixture::new("stable-one");
    fixture.init();
    assert_eq!(fixture.lock()["bundle_id"], "stable-one");
    assert!(fixture.project.path().join(".infra/bootstrap.py").is_file());
    assert!(fixture.project.path().join(".infra/bin/style-check.sh").is_file());
    assert!(
        fixture
            .project
            .path()
            .join(".infra/style/policy/default.toml")
            .is_file()
    );
    assert!(fixture.project.path().join(".infra/project.toml").is_file());
    assert!(!fixture.project.path().join(".infra/rs-infra").exists());
    assert!(fixture.run(&["--offline", "check"]).status.success());
}

#[test]
fn test_update_preserves_project_configuration_and_detects_managed_conflict() {
    let fixture = Fixture::new("stable-one");
    fixture.init();
    let config = fixture.project.path().join(".infra/style/config.toml");
    fs::write(&config, "schema = 1\ncustom = true\n").expect("custom configuration");
    assert!(fixture.run(&["update"]).status.success());
    assert_eq!(
        fs::read_to_string(config).expect("config"),
        "schema = 1\ncustom = true\n"
    );
    fs::write(
        fixture.project.path().join(".infra/style/policy/default.toml"),
        "user change\n",
    )
    .expect("edited managed policy");
    let output = fixture.run(&["update"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("conflict"));
}

#[test]
fn test_offline_run_uses_only_pinned_cache_and_checks_version_identity() {
    let fixture = Fixture::new("stable-one");
    fixture.init();
    let output = fixture.run(&["--offline", "run", "--tool", "cpp-infra-style", "--", "check"]);
    assert_eq!(output.status.code(), Some(3));
    assert!(fixture.run(&["prewarm", "--tool", "cpp-infra-style"]).status.success());
    let output = fixture.run(&["--offline", "run", "--tool", "cpp-infra-style", "--", "check"]);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let bindir = String::from_utf8(output.stdout).expect("child stdout");
    assert!(Path::new(bindir.trim()).join("cpp-infra-style").is_file());
    assert!(fixture.run(&["--offline", "check"]).status.success());
}

#[test]
fn test_bad_asset_digest_never_becomes_runnable_cache() {
    let fixture = Fixture::new("stable-one");
    fixture.init();
    fs::write(fixture.root.path().join("cpp-infra-style"), "tampered").expect("corrupt release");
    let output = fixture.run(&["prewarm", "--tool", "cpp-infra-style"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("SHA-256"));
    assert_eq!(
        fixture
            .run(&["--offline", "run", "--tool", "cpp-infra-style"])
            .status
            .code(),
        Some(3)
    );
}

#[test]
fn test_success_report_contains_common_identity_and_digests() {
    let fixture = Fixture::new("stable-one");
    fixture.init();
    let report = fixture.project.path().join("build/result.json");
    let output = fixture.run(&["--report", report.to_str().expect("path"), "check"]);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let report: Value = serde_json::from_slice(&fs::read(report).expect("report")).expect("JSON");
    for field in [
        "schema",
        "tool",
        "bundle_id",
        "run_id",
        "task_id",
        "project",
        "report_root",
        "source_revision",
        "source_digest",
        "config_digest",
        "status",
        "started_at",
        "duration_ms",
        "external_versions",
        "commands",
        "diagnostics",
        "artifacts",
    ] {
        assert!(report.get(field).is_some(), "missing {field}");
    }
    assert_eq!(report["source_digest"].as_str().expect("digest").len(), 64);
    assert_eq!(report["config_digest"].as_str().expect("digest").len(), 64);
}

#[test]
fn test_run_rejects_successful_child_without_required_report() {
    let fixture = Fixture::new("stable-one");
    let bytes = fs::read(fixture.root.path().join("cpp-infra-style")).expect("asset");
    let script = String::from_utf8(bytes).expect("python").replace(
        "with open(args[args.index(\"--report\")+1], \"w\") as output:\n    json.dump(report, output)",
        "pass",
    );
    fixture.replace_binary(script.as_bytes());
    fixture.init();
    let output = fixture.run(&["run", "--tool", "cpp-infra-style", "--", "check"]);
    assert_eq!(output.status.code(), Some(2), "missing child report cannot pass");
}

#[test]
fn test_run_rejects_child_report_with_foreign_bundle_identity() {
    let fixture = Fixture::new("stable-one");
    let bytes = fs::read(fixture.root.path().join("cpp-infra-style")).expect("asset");
    let script = String::from_utf8(bytes).expect("python").replace(
        "report = dict(context,",
        "context['bundle_id'] = 'foreign-bundle'\nreport = dict(context,",
    );
    fixture.replace_binary(script.as_bytes());
    fixture.init();
    assert_eq!(
        fixture.run(&["run", "--tool", "cpp-infra-style"]).status.code(),
        Some(2)
    );
}

#[test]
fn test_version_protocol_mismatch_is_not_installed() {
    let fixture = Fixture::new("stable-one");
    let script = fs::read_to_string(fixture.root.path().join("cpp-infra-style"))
        .expect("script")
        .replace("\"protocol\":1", "\"protocol\":2");
    fixture.replace_binary(script.as_bytes());
    fixture.init();
    let output = fixture.run(&["prewarm", "--tool", "cpp-infra-style"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("protocol mismatch"));
}

#[test]
fn test_project_configuration_typo_and_overlapping_roots_are_rejected() {
    let fixture = Fixture::new("stable-one");
    fixture.init();
    let path = fixture.project.path().join(".infra/project.toml");
    let original = fs::read_to_string(&path).expect("config");
    fs::write(&path, format!("typo = true\n{original}")).expect("typo");
    assert_eq!(fixture.run(&["check"]).status.code(), Some(2));
    fs::write(&path, original.replace("test_roots = []", "test_roots = [\"src\"]")).expect("overlap");
    let output = fixture.run(&["check"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("overlapping"));
}

#[test]
fn test_update_dry_run_does_not_change_snapshot_or_managed_files() {
    let fixture = Fixture::new("stable-one");
    fixture.init();
    let lock_path = fixture.project.path().join(".infra/tools/tools.lock.json");
    let original = fs::read(&lock_path).expect("lock");
    let mut next = fixture.catalog_value();
    next["bundle_id"] = json!("stable-two");
    fixture.write_catalog(&next);
    assert!(fixture.run(&["update", "--dry-run"]).status.success());
    assert_eq!(fs::read(lock_path).expect("lock"), original);
    assert_eq!(fixture.lock()["bundle_id"], "stable-one");
}

#[test]
fn test_local_check_does_not_resolve_newest_channel() {
    let fixture = Fixture::new("old-stable");
    fixture.init();
    fs::write(fixture.root.path().join("catalog.json"), "invalid latest channel").expect("unavailable upstream");
    assert!(fixture.run(&["--offline", "check"]).status.success());
}

#[test]
fn test_status_reports_update_without_changing_current_lock() {
    let fixture = Fixture::new("stable-one");
    fixture.init();
    let mut next = fixture.catalog_value();
    next["bundle_id"] = json!("stable-two");
    fixture.write_catalog(&next);
    let output = fixture.run(&["status"]);
    assert!(output.status.success());
    let result: Value = serde_json::from_slice(&output.stdout).expect("status");
    assert_eq!(result["update_available"], true);
    assert_eq!(fixture.lock()["bundle_id"], "stable-one");
}

#[test]
fn test_missing_platform_asset_fails_without_path_fallback() {
    let fixture = Fixture::new("stable-one");
    let mut catalog = fixture.catalog_value();
    catalog["tools"][0]["assets"] = json!({});
    fixture.write_catalog(&catalog);
    let output = fixture.run(&["init"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(!fixture.project.path().join(".infra/tools/tools.lock.json").exists());
}

#[test]
fn test_untrusted_legacy_transaction_is_preserved_by_readonly_check() {
    let fixture = Fixture::new("stable-one");
    fixture.init();
    let target = ".infra/style/policy/default.toml";
    let original = fs::read(fixture.project.path().join(target)).expect("policy");
    let stage = fixture.project.path().join(".infra/transactions/tools-update");
    fs::create_dir_all(stage.join("backup")).expect("stage");
    fs::write(stage.join("backup/0"), &original).expect("backup");
    fs::write(
        stage.join("owner.json"),
        serde_json::to_vec(&json!({"schema":1,"owner":"cpp-infra-tools"})).expect("owner"),
    )
    .expect("owner file");
    fs::write(stage.join("journal.json"),serde_json::to_vec(&json!({"schema":1,"committed":false,"entries":[{"target":target,"existed":true,"executable":false,"backup":"backup/0"}]})).expect("journal")).expect("journal file");
    fs::write(fixture.project.path().join(target), "partially replaced").expect("interrupted replacement");
    let result = fixture.run(&["check"]);
    assert_eq!(result.status.code(), Some(2));
    assert_eq!(
        fs::read_to_string(fixture.project.path().join(target)).expect("preserved"),
        "partially replaced"
    );
    assert!(stage.exists());
    assert_eq!(fs::read(stage.join("backup/0")).expect("preserved backup"), original);
}

#[cfg(unix)]
#[test]
fn test_init_rejects_symlink_escape_without_writing_outside_project() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new("stable-one");
    let outside = TempDir::new().expect("outside");
    symlink(outside.path(), fixture.project.path().join(".infra")).expect("symlink");
    let output = fixture.run(&["init"]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(fs::read_dir(outside.path()).expect("outside entries").count(), 0);
}

#[test]
fn test_doctor_queries_real_cmake_and_enforces_bundle_version_constraint() {
    let fixture = Fixture::new("stable-one");
    let mut catalog = fixture.catalog_value();
    catalog["external_tools"] = json!({"cmake":{"program":"cmake","version":">=3.25","tasks":["verify"]}});
    fixture.write_catalog(&catalog);
    fixture.init();
    assert!(
        fixture
            .run(&["doctor", "--task", "verify", "--profile", "debug"])
            .status
            .success()
    );
    let mut catalog = fixture.catalog_value();
    catalog["external_tools"]["cmake"]["version"] = json!(">=999.0.0");
    fixture.write_catalog(&catalog);
    assert!(fixture.run(&["update"]).status.success());
    let result = fixture.run(&["doctor", "--task", "verify"]);
    assert_eq!(result.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&result.stderr).contains("violates"));
}

#[test]
fn test_run_context_rejects_changed_source() {
    let fixture = Fixture::new("stable-one");
    fixture.init();
    let report = fixture.project.path().join("build/check.json");
    assert!(
        fixture
            .run(&["--report", report.to_str().expect("report path"), "check"])
            .status
            .success()
    );
    let identity: Value = serde_json::from_slice(&fs::read(report).expect("report")).expect("identity");
    let context = fixture.project.path().join("build/context.json");
    let mut value = identity.clone();
    value["profile"] = json!("debug");
    fs::write(&context, serde_json::to_vec(&value).expect("context")).expect("context file");
    fs::write(fixture.project.path().join("src/new.cpp"), "int value = 1;\n").expect("source change");
    let output = fixture.run(&["--run-context", context.to_str().expect("path"), "check"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("stale"));
}

#[test]
fn test_concurrent_projects_download_same_verified_http_asset_once() {
    use std::io::Read;
    use std::io::Write;
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use std::time::Duration;
    let fixture = Fixture::new("shared-cache");
    let bytes = fs::read(fixture.root.path().join("cpp-infra-style")).expect("asset");
    let listener = TcpListener::bind("127.0.0.1:0").expect("HTTP fixture");
    listener.set_nonblocking(true).expect("nonblocking");
    let address = listener.local_addr().expect("address");
    let count = Arc::new(AtomicUsize::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let server_count = Arc::clone(&count);
    let server_stop = Arc::clone(&stop);
    let server = std::thread::spawn(move || {
        while !server_stop.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let mut buffer = [0; 4096];
                    let received = stream.read(&mut buffer).expect("request");
                    assert!(received > 0, "HTTP request must not be empty");
                    server_count.fetch_add(1, Ordering::SeqCst);
                    write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        bytes.len()
                    )
                    .expect("response");
                    stream.write_all(&bytes).expect("asset body");
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5))
                }
                Err(error) => panic!("accept: {error}"),
            }
        }
    });
    let mut catalog = fixture.catalog_value();
    for asset in catalog["tools"][0]["assets"]
        .as_object_mut()
        .expect("assets")
        .values_mut()
    {
        asset["url"] = json!(format!("http://{address}/asset"));
    }
    fixture.write_catalog(&catalog);
    fixture.init();
    let second = TempDir::new().expect("second project");
    fs::write(
        second.path().join("CMakeLists.txt"),
        "cmake_minimum_required(VERSION 3.25)\nproject(second)\n",
    )
    .expect("second CMake");
    let spawn = |project: &Path, command: &str| {
        Command::new(env!("CARGO_BIN_EXE_cpp-infra-tools"))
            .arg("--locked")
            .arg("--project")
            .arg(project)
            .env("CPP_INFRA_CATALOG_URL", &fixture.catalog)
            .env("CPP_INFRA_CACHE_DIR", fixture.cache.path())
            .arg(command)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("child")
    };
    let init = spawn(second.path(), "init").wait_with_output().expect("second init");
    assert!(init.status.success(), "{}", String::from_utf8_lossy(&init.stderr));
    let first = spawn(fixture.project.path(), "prewarm");
    let second = spawn(second.path(), "prewarm");
    let first = first.wait_with_output().expect("first");
    let second = second.wait_with_output().expect("second");
    stop.store(true, Ordering::SeqCst);
    server.join().expect("HTTP server");
    assert!(first.status.success(), "{}", String::from_utf8_lossy(&first.stderr));
    assert!(second.status.success(), "{}", String::from_utf8_lossy(&second.stderr));
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[test]
fn test_public_archive_rejects_symbolic_links_before_installing_snapshot() {
    let fixture = Fixture::new("bad-archive");
    let path = fixture.root.path().join("public.tar.gz");
    let gzip = flate2::write::GzEncoder::new(
        fs::File::create(&path).expect("archive"),
        flate2::Compression::default(),
    );
    let mut archive = tar::Builder::new(gzip);
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Symlink);
    header.set_size(0);
    header.set_mode(0o777);
    archive
        .append_link(&mut header, "policy.toml", "../../outside")
        .expect("malicious link fixture");
    archive.into_inner().expect("tar finish").finish().expect("gzip finish");
    let mut catalog = fixture.catalog_value();
    catalog["packages"][0]["sha256"] = json!(digest(&fs::read(path).expect("archive bytes")));
    fixture.write_catalog(&catalog);
    let result = fixture.run(&["init"]);
    assert_eq!(result.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&result.stderr).contains("archive entry"));
    assert!(!fixture.project.path().join(".infra/tools/tools.lock.json").exists());
}

#[test]
fn test_unowned_transaction_directory_is_preserved_and_reported_as_conflict() {
    let fixture = Fixture::new("stable-one");
    fixture.init();
    let stage = fixture.project.path().join(".infra/transactions/tools-update");
    fs::create_dir_all(&stage).expect("unowned directory");
    fs::write(stage.join("important.txt"), "user data").expect("unowned data");
    let result = fixture.run(&["check"]);
    assert_eq!(result.status.code(), Some(2));
    assert_eq!(
        fs::read_to_string(stage.join("important.txt")).expect("preserved data"),
        "user data"
    );
}

#[test]
fn test_explicit_pinned_source_fallback_builds_real_cargo_and_is_reusable_offline() {
    let fixture = Fixture::new("source-bundle");
    let revision = "1234567890123456789012345678901234567890";
    let source = r#"fn main(){println!("{{\"name\":\"cpp-infra-style\",\"version\":\"0.1.0\",\"revision\":\"{}\",\"protocol\":1}}",env!("CPP_INFRA_BUILD_REVISION"));}"#;
    let cargo=b"[package]\nname=\"source-fixture\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[[bin]]\nname=\"cpp-infra-style\"\npath=\"src/main.rs\"\n";
    let lock = b"version = 4\n[[package]]\nname = \"source-fixture\"\nversion = \"0.1.0\"\n";
    let path = fixture.root.path().join("source.tar.gz");
    let gzip = flate2::write::GzEncoder::new(
        fs::File::create(&path).expect("source archive"),
        flate2::Compression::default(),
    );
    let mut archive = tar::Builder::new(gzip);
    for (name, bytes) in [
        ("Cargo.toml", cargo.as_slice()),
        ("Cargo.lock", lock.as_slice()),
        ("src/main.rs", source.as_bytes()),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        archive.append_data(&mut header, name, bytes).expect("source entry");
    }
    archive.into_inner().expect("tar finish").finish().expect("gzip finish");
    let mut catalog = fixture.catalog_value();
    catalog["tools"][0]["assets"] = json!({});
    catalog["tools"][0]["source"] = json!({"url":format!("file://{}",path.display()),"sha256":digest(&fs::read(path).expect("source bytes")),"cargo_lock_sha256":digest(lock)});
    assert_eq!(catalog["tools"][0]["revision"], revision);
    fixture.write_catalog(&catalog);
    fixture.init();
    assert_eq!(
        fixture.run(&["prewarm", "--tool", "cpp-infra-style"]).status.code(),
        Some(2)
    );
    let result = fixture.run(&["--from-source", "prewarm", "--tool", "cpp-infra-style"]);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert!(
        fixture
            .run(&["--offline", "--from-source", "prewarm", "--tool", "cpp-infra-style"])
            .status
            .success()
    );
}

#[cfg(unix)]
#[test]
fn test_cancel_while_waiting_for_project_lock_returns_130() {
    use std::io::BufRead;
    use std::io::BufReader;
    use std::time::Duration;
    use std::time::Instant;
    let fixture = Fixture::new("stable-one");
    fixture.init();
    let path = fixture.project.path().join("build/.cpp-infra-project.lock");
    let mut holder=Command::new("python3").args(["-c","import fcntl,sys,time; f=open(sys.argv[1],'a+'); fcntl.flock(f,fcntl.LOCK_EX); print('locked',flush=True); time.sleep(30)"]).arg(path).stdout(std::process::Stdio::piped()).spawn().expect("lock holder");
    let mut line = String::new();
    BufReader::new(holder.stdout.take().expect("stdout"))
        .read_line(&mut line)
        .expect("lock readiness");
    assert_eq!(line.trim(), "locked");
    let mut child = Command::new(env!("CARGO_BIN_EXE_cpp-infra-tools"))
        .arg("--locked")
        .arg("--project")
        .arg(fixture.project.path())
        .arg("check")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("waiting tools");
    std::thread::sleep(Duration::from_millis(100));
    assert!(
        Command::new("kill")
            .args(["-INT", &child.id().to_string()])
            .status()
            .expect("cancel")
            .success()
    );
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll") {
            break Some(status);
        }
        if start.elapsed() > Duration::from_secs(2) {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let _ = holder.kill();
    let _ = holder.wait();
    assert_eq!(status.and_then(|s| s.code()), Some(130));
}

#[test]
fn test_child_success_is_bound_to_tool_and_current_run_identity() {
    for mutation in [
        "report['tool']['name']='wrong-tool'",
        "report['tool']['version']='9.9.9'",
        "report['tool']['revision']='wrong'",
        "report['run_id']='stale-run'",
        "report['task_id']='other-task'",
        "report['report_root']='/foreign/root'",
    ] {
        let fixture = Fixture::new("identity-bound");
        let script = fs::read_to_string(fixture.root.path().join("cpp-infra-style")).expect("script");
        fixture.replace_binary(
            script
                .replace(
                    "with open(args[args.index(\"--report\")+1], \"w\") as output:",
                    &format!("{mutation}\nwith open(args[args.index(\"--report\")+1], \"w\") as output:"),
                )
                .as_bytes(),
        );
        fixture.init();
        let result = fixture.run(&["run", "--tool", "cpp-infra-style"]);
        assert_eq!(
            result.status.code(),
            Some(2),
            "accepted {mutation}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

#[test]
fn test_required_child_artifacts_must_exist_be_safe_and_match_sha() {
    for artifact in [
        json!({"kind":"result","path":"missing.json","sha256":"0".repeat(64),"required":true}),
        json!({"kind":"result","path":"../escape.json","sha256":"0".repeat(64),"required":true}),
        json!({"kind":"result","path":"result.json","sha256":"0".repeat(64),"required":true}),
    ] {
        let fixture = Fixture::new("artifact-bound");
        let script = fs::read_to_string(fixture.root.path().join("cpp-infra-style")).expect("script");
        let mutation = format!(
            "from pathlib import Path\nPath(context['report_root']).joinpath('result.json').write_text('actual bytes')\nreport['artifacts']=[{}]",
            serde_json::to_string(&artifact)
                .expect("artifact")
                .replace("true", "True")
        );
        fixture.replace_binary(
            script
                .replace(
                    "with open(args[args.index(\"--report\")+1], \"w\") as output:",
                    &format!("{mutation}\nwith open(args[args.index(\"--report\")+1], \"w\") as output:"),
                )
                .as_bytes(),
        );
        fixture.init();
        assert_eq!(
            fixture.run(&["run", "--tool", "cpp-infra-style"]).status.code(),
            Some(2)
        );
    }
}

#[test]
fn test_forged_journal_cannot_remove_unmanaged_user_file() {
    let fixture = Fixture::new("journal-bound");
    fixture.init();
    let victim = fixture.project.path().join(".infra/private-data.json");
    fs::write(&victim, "user data").expect("victim");
    let stage = fixture.project.path().join(".infra/transactions/tools-update");
    fs::create_dir_all(&stage).expect("stage");
    fs::write(stage.join("owner.json"), br#"{"schema":1,"owner":"cpp-infra-tools"}"#).expect("owner");
    fs::write(stage.join("journal.json"),serde_json::to_vec(&json!({"schema":1,"committed":false,"entries":[{"target":".infra/private-data.json","existed":false,"executable":false,"backup":"backup/0"}]})).expect("journal")).expect("journal file");
    for action in ["check", "update"] {
        assert_eq!(fixture.run(&[action]).status.code(), Some(2));
        assert_eq!(fs::read_to_string(&victim).expect("preserved"), "user data");
        assert!(stage.join("journal.json").is_file());
    }
}

#[test]
fn test_delivered_coverage_wrapper_uses_collect() {
    let fixture = Fixture::new("wrapper");
    fixture.init();
    fs::write(
        fixture.project.path().join(".infra/bootstrap.py"),
        "import json,sys\nprint(json.dumps(sys.argv[1:]))\n",
    )
    .expect("capture bootstrap fixture");
    let result = Command::new("bash")
        .arg(fixture.project.path().join(".infra/bin/coverage.sh"))
        .arg("--profile=coverage")
        .output()
        .expect("wrapper");
    assert!(result.status.success());
    let args: Value = serde_json::from_slice(&result.stdout).expect("args");
    assert_eq!(
        args,
        json!([
            "run",
            "--tool",
            "cpp-infra-coverage",
            "--",
            "collect",
            "--profile=coverage"
        ])
    );
}

#[test]
fn test_configuration_symlink_into_build_is_rejected() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new("tracked-config");
        fixture.init();
        let config = fixture.project.path().join(".infra/style/config.toml");
        let bytes = fs::read(&config).expect("config");
        fs::write(fixture.project.path().join("build/policy.toml"), bytes).expect("excluded policy");
        fs::remove_file(&config).expect("replace fixture config");
        symlink(fixture.project.path().join("build/policy.toml"), &config).expect("symlink");
        assert_eq!(fixture.run(&["check"]).status.code(), Some(2));
    }
}

fn policy_package(fixture: &Fixture, label: &str, count: usize) {
    let archive_path = fixture.root.path().join("public.tar.gz");
    let gzip = flate2::write::GzEncoder::new(
        fs::File::create(&archive_path).expect("archive"),
        flate2::Compression::default(),
    );
    let mut archive = tar::Builder::new(gzip);
    let mut contents = Vec::new();
    let mut files = Vec::new();
    for index in 0..count {
        let name = format!("policy/{index:04}.txt");
        let bytes = format!("{label}:{index}").into_bytes();
        files.push(json!({"target":&name,"source":&name,"sha256":digest(&bytes),"executable":false}));
        contents.push((name, bytes));
    }
    contents.push((
        "manifest.json".to_owned(),
        serde_json::to_vec(&json!({"schema":1,"owner":"cpp-infra-style","files":files})).expect("manifest"),
    ));
    for (name, bytes) in contents {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        archive.append_data(&mut header, name, bytes.as_slice()).expect("entry");
    }
    archive.into_inner().expect("tar").finish().expect("gzip");
    let mut catalog = fixture.catalog_value();
    catalog["packages"][0]["sha256"] = json!(digest(&fs::read(archive_path).expect("archive bytes")));
    fixture.write_catalog(&catalog);
}

#[test]
fn test_trusted_crash_recovers_only_on_update_and_preserves_conflicts() {
    use std::time::Duration;
    use std::time::Instant;
    for conflict in ["none", "local", "backup", "journal"] {
        let fixture = Fixture::new("trusted-recovery");
        policy_package(&fixture, "old", 500);
        fixture.init();
        policy_package(&fixture, "new", 500);
        let mut child = Command::new(env!("CARGO_BIN_EXE_cpp-infra-tools"))
            .arg("--locked")
            .arg("--project")
            .arg(fixture.project.path())
            .arg("update")
            .env("CPP_INFRA_CATALOG_URL", &fixture.catalog)
            .env("CPP_INFRA_CACHE_DIR", fixture.cache.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("update process");
        let first = fixture.project.path().join(".infra/style/policy/0000.txt");
        let start = Instant::now();
        loop {
            if fs::read_to_string(&first).is_ok_and(|v| v == "new:0") {
                break;
            }
            assert!(
                child.try_wait().expect("poll").is_none(),
                "update finished before crash window"
            );
            assert!(
                start.elapsed() < Duration::from_secs(20),
                "update did not reach crash window"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        child
            .kill()
            .expect("crash update after journal/receipt and first replacement");
        let _ = child.wait();
        let stage = fixture.project.path().join(".infra/transactions/tools-update");
        let journal: Value =
            serde_json::from_slice(&fs::read(stage.join("journal.json")).expect("durable journal")).expect("JSON");
        let owner: Value = serde_json::from_slice(&fs::read(stage.join("owner.json")).expect("owner")).expect("JSON");
        assert!(!Path::new(owner["receipt"].as_str().expect("receipt")).starts_with(fixture.project.path()));
        assert_eq!(fixture.run(&["check"]).status.code(), Some(2));
        assert_eq!(
            fs::read_to_string(&first).expect("readonly preserves partial state"),
            "new:0"
        );
        if conflict == "none" {
            let infra = fixture.project.path().join(".infra");
            let receipts = Path::new(owner["receipt"].as_str().expect("receipt"))
                .parent()
                .expect("receipt directory");
            let infra_before = tree_state(&infra);
            let receipts_before = tree_state(receipts);
            let preview = fixture.run(&["update", "--dry-run"]);
            assert_eq!(
                preview.status.code(),
                Some(2),
                "pending recovery requires explicit non-dry update"
            );
            assert!(String::from_utf8_lossy(&preview.stderr).contains("unfinished transaction"));
            assert_eq!(
                tree_state(&infra),
                infra_before,
                "dry-run preserves all project transaction state"
            );
            assert_eq!(
                tree_state(receipts),
                receipts_before,
                "dry-run preserves external trust receipts"
            );
        }
        if conflict == "local" {
            fs::write(&first, "user local edit").expect("local edit");
        }
        if conflict == "backup" {
            let entry = journal["entries"]
                .as_array()
                .expect("entries")
                .iter()
                .find(|e| e["target"] == ".infra/style/policy/0000.txt")
                .expect("entry");
            fs::write(
                stage.join(entry["backup"].as_str().expect("backup")),
                "corrupted backup",
            )
            .expect("corrupt backup");
        }
        if conflict == "journal" {
            let mut changed = journal.clone();
            changed["entries"][0]["target"] = json!(".infra/private-data.json");
            fs::write(
                stage.join("journal.json"),
                serde_json::to_vec(&changed).expect("changed journal"),
            )
            .expect("tamper journal");
        }
        let before = fs::read(&first).expect("before recovery");
        let result = fixture.run(&["update"]);
        if conflict == "none" {
            assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
            assert!(fixture.run(&["check"]).status.success());
            assert!(!stage.exists());
            assert_eq!(fs::read_to_string(first).expect("updated"), "new:0");
        } else {
            assert_eq!(
                result.status.code(),
                Some(2),
                "{conflict}: {}",
                String::from_utf8_lossy(&result.stderr)
            );
            assert_eq!(fs::read(&first).expect("conflict preserved"), before);
            assert!(stage.join("journal.json").is_file());
        }
    }
}

#[test]
fn test_forged_previous_manifest_does_not_grant_retirement_ownership() {
    for target in [".infra/private-data.json", ".infra/style/policy/user-private.toml"] {
        let fixture = Fixture::new("prior-authority");
        fixture.init();
        let victim = fixture.project.path().join(target);
        fs::write(&victim, "important user contents").expect("user data");
        let path = fixture.project.path().join(".infra/managed-files.json");
        let mut manifest: Value = serde_json::from_slice(&fs::read(&path).expect("manifest")).expect("JSON");
        manifest["files"].as_array_mut().expect("files").push(json!({"target":target,"source":"cpp-infra-style:policy/user-private.toml","sha256":digest(b"important user contents"),"executable":false}));
        fs::write(path, serde_json::to_vec(&manifest).expect("forged manifest")).expect("manifest file");
        let result = fixture.run(&["update"]);
        assert_eq!(
            result.status.code(),
            Some(2),
            "{target}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            fs::read_to_string(victim).expect("preserved victim"),
            "important user contents"
        );
    }
}

#[test]
fn test_delivered_align_wrapper_runs_style_fix() {
    let fixture = Fixture::new("align");
    fixture.init();
    fs::write(
        fixture.project.path().join(".infra/bootstrap.py"),
        "import json,sys\nprint(json.dumps(sys.argv[1:]))\n",
    )
    .expect("capture");
    let output = Command::new("bash")
        .arg(fixture.project.path().join(".infra/bin/align-ci.sh"))
        .arg("--dry-run")
        .output()
        .expect("align wrapper");
    assert!(output.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).expect("args"),
        json!(["run", "--tool", "cpp-infra-style", "--", "fix", "--dry-run"])
    );
}

#[test]
fn test_required_directory_artifact_is_accepted_with_canonical_tree_hash() {
    let fixture = Fixture::new("tree-artifact");
    let script = fs::read_to_string(fixture.root.path().join("cpp-infra-style")).expect("script");
    let mutation = "from pathlib import Path\nimport hashlib,struct\np=Path(context['report_root'])/'site'\np.mkdir()\nb=b'<html>real artifact</html>'\np.joinpath('index.html').write_bytes(b)\nh=hashlib.sha256()\nn=b'index.html'\nh.update(struct.pack('<Q',len(n)));h.update(n);h.update(struct.pack('<Q',len(b)));h.update(b)\nreport['artifacts']=[dict(kind='html',path='site',sha256=h.hexdigest(),required=True)]";
    fixture.replace_binary(
        script
            .replace(
                "with open(args[args.index(\"--report\")+1], \"w\") as output:",
                &format!("{mutation}\nwith open(args[args.index(\"--report\")+1], \"w\") as output:"),
            )
            .as_bytes(),
    );
    fixture.init();
    let result = fixture.run(&["run", "--tool", "cpp-infra-style"]);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
}

#[test]
fn test_readonly_entrypoints_share_the_existing_project_lock() {
    use std::fs::OpenOptions;
    use std::time::Duration;
    use std::time::Instant;

    use fs2::FileExt;
    let fixture = Fixture::new("shared-project-lock");
    fixture.init();
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .open(fixture.project.path().join("build/.cpp-infra-project.lock"))
        .expect("project lock");
    FileExt::lock_shared(&lock).expect("outer reader lock");
    let mut child = Command::new(env!("CARGO_BIN_EXE_cpp-infra-tools"))
        .args(["--locked", "--project"])
        .arg(fixture.project.path())
        .arg("check")
        .env("CPP_INFRA_CATALOG_URL", &fixture.catalog)
        .env("CPP_INFRA_CACHE_DIR", fixture.cache.path())
        .spawn()
        .expect("nested reader");
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().expect("poll reader") {
            assert!(status.success());
            break;
        }
        if start.elapsed() > Duration::from_secs(4) {
            child.kill().expect("stop blocked reader");
            child.wait().expect("reap blocked reader");
            panic!("read-only manager tried to upgrade the shared project lock");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
