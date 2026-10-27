// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
use std::fs;
use std::io::BufRead;
use std::io::BufReader;
use std::io::Cursor;
use std::io::Write;
use std::net::TcpListener;
use std::net::TcpStream;
use std::process::Command;
use std::process::Output;
use std::thread;

use serde_json::Value;
use serde_json::json;
use sha2::Digest;
use sha2::Sha256;
use tempfile::TempDir;
use zip::write::SimpleFileOptions;

const REVISION: &str = "1234567890123456789012345678901234567890";
const BINARY: &str = "cpp-infra-style";

#[test]
fn test_locked_invalid_utf8_configuration_is_a_configuration_error() {
    let fixture = Distribution::new();
    fixture.init();
    let config = fixture.project.path().join(".infra/style/config.toml");
    fs::create_dir_all(config.parent().expect("config directory")).expect("config directory");
    fs::write(&config, "schema=1\n").expect("valid tool configuration");
    assert!(fixture.run(&["check"]).status.success());
    for relative in [".infra/project.toml", ".infra/style/config.toml"] {
        let path = fixture.project.path().join(relative);
        let valid = fs::read(&path).expect("valid configuration bytes");
        fs::write(&path, [0xff]).expect("invalid UTF-8 configuration");
        let output = fixture.run(&["check"]);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{relative}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .to_ascii_lowercase()
                .contains("utf-8")
        );
        assert_eq!(fs::read(&path).expect("preserved invalid configuration"), [0xff]);
        fs::write(path, valid).expect("restore valid configuration");
    }
    assert!(fixture.run(&["check"]).status.success());
}

fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn read_request_headers(stream: &mut TcpStream) {
    let mut reader = BufReader::new(stream);
    loop {
        let mut line = String::new();
        assert!(
            reader.read_line(&mut line).expect("HTTP request line") > 0,
            "truncated HTTP headers"
        );
        if line == "\r\n" {
            break;
        }
    }
}

fn executable() -> Vec<u8> {
    format!(
        "#!/usr/bin/env python3\nimport json\nprint(json.dumps(dict(name='cpp-infra-style',version='0.1.0',revision='{REVISION}',protocol=1)))\n"
    )
    .into_bytes()
}

fn tar_bytes(entries: &[(&str, &[u8], tar::EntryType)]) -> Vec<u8> {
    let gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut archive = tar::Builder::new(gzip);
    for (name, bytes, kind) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o755);
        header.set_entry_type(*kind);
        if kind.is_symlink() || kind.is_hard_link() {
            header.set_link_name("outside").expect("link target");
        }
        header.set_cksum();
        archive.append_data(&mut header, name, *bytes).expect("tar entry");
    }
    archive.into_inner().expect("tar finish").finish().expect("gzip finish")
}

fn zip_bytes(entries: &[(&str, &[u8])], directory: bool) -> Vec<u8> {
    let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
    if directory {
        archive
            .add_directory("release/", SimpleFileOptions::default())
            .expect("zip directory");
    }
    for (name, bytes) in entries {
        archive
            .start_file(*name, SimpleFileOptions::default())
            .expect("zip file");
        archive.write_all(bytes).expect("zip bytes");
    }
    archive.finish().expect("zip finish").into_inner()
}

struct Distribution {
    root: TempDir,
    project: TempDir,
    cache: TempDir,
    catalog: Value,
}

impl Distribution {
    fn new() -> Self {
        let root = TempDir::new().expect("distribution");
        let project = TempDir::new().expect("project");
        let cache = TempDir::new().expect("cache");
        fs::write(
            project.path().join("CMakeLists.txt"),
            "cmake_minimum_required(VERSION 3.25)\nproject(fixture)\n",
        )
        .expect("CMake project");
        let host = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
        let bytes = executable();
        let asset_path = root.path().join("asset");
        fs::write(&asset_path, &bytes).expect("release asset");
        let catalog = json!({"schema":1,"bundle_id":"distribution-fixture","protocol_major":1,"tools":[{"name":BINARY,"version":"0.1.0","revision":REVISION,"protocol":1,"assets":{host:{"url":format!("file://{}",asset_path.display()),"sha256":digest(&bytes),"executable_sha256":digest(&bytes),"binary":BINARY,"format":"raw"}}}],"packages":[],"external_tools":{}});
        Self {
            root,
            project,
            cache,
            catalog,
        }
    }

    fn asset(&mut self) -> &mut Value {
        self.catalog["tools"][0]["assets"]
            .as_object_mut()
            .expect("platform assets")
            .values_mut()
            .next()
            .expect("asset")
    }

    fn set_archive(&mut self, bytes: &[u8], format: Option<&str>) {
        fs::write(self.root.path().join("asset"), bytes).expect("archive asset");
        self.asset()["sha256"] = json!(digest(bytes));
        if let Some(format) = format {
            self.asset()["format"] = json!(format);
        } else {
            self.asset().as_object_mut().expect("asset object").remove("format");
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        let catalog = self.root.path().join("catalog.json");
        fs::write(&catalog, serde_json::to_vec(&self.catalog).expect("catalog JSON")).expect("catalog file");
        Command::new(env!("CARGO_BIN_EXE_cpp-infra-tools"))
            .arg("--locked")
            .arg("--project")
            .arg(self.project.path())
            .env("CPP_INFRA_CATALOG_URL", format!("file://{}", catalog.display()))
            .env("CPP_INFRA_CACHE_DIR", self.cache.path())
            .args(args)
            .output()
            .expect("public CLI")
    }

    fn init(&self) {
        let output = self.run(&["init"]);
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    }

    fn catalog_reference(&self, name: &str, catalog: &Value) -> Value {
        let path = self.root.path().join(name);
        let bytes = serde_json::to_vec(catalog).expect("channel catalog JSON");
        fs::write(&path, &bytes).expect("channel catalog file");
        json!({"url":format!("file://{}",path.display()),"sha256":digest(&bytes)})
    }

    fn channel(&self) -> Value {
        json!({"schema":1,"bundle_id":self.catalog["bundle_id"],"catalog":self.catalog_reference("stable-catalog.json", &self.catalog)})
    }

    fn run_channel(&self, channel: &Value, args: &[&str]) -> Output {
        let path = self.root.path().join("stable.json");
        fs::write(&path, serde_json::to_vec(channel).expect("channel JSON")).expect("stable channel file");
        Command::new(env!("CARGO_BIN_EXE_cpp-infra-tools"))
            .arg("--locked")
            .arg("--project")
            .arg(self.project.path())
            .env_remove("CPP_INFRA_CATALOG_URL")
            .env("CPP_INFRA_CHANNEL_URL", format!("file://{}", path.display()))
            .env("CPP_INFRA_CACHE_DIR", self.cache.path())
            .args(args)
            .output()
            .expect("public stable channel CLI")
    }

    fn lock(&self) -> Value {
        serde_json::from_slice(
            &fs::read(self.project.path().join(".infra/tools/tools.lock.json")).expect("installed lock"),
        )
        .expect("installed lock JSON")
    }

    fn channel_init_failure(&self, channel: &Value, message: &str) {
        let output = self.run_channel(channel, &["init"]);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(message),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!self.project.path().join(".infra/tools/tools.lock.json").exists());
    }

    fn prewarm(&self) -> Output {
        self.run(&["prewarm", "--tool", BINARY])
    }

    fn pinned_source(&mut self, main: &[u8], lock_digest: &str) {
        let cargo = b"[package]\nname=\"distribution-source\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[[bin]]\nname=\"cpp-infra-style\"\npath=\"src/main.rs\"\n";
        let lock = b"version = 4\n[[package]]\nname = \"distribution-source\"\nversion = \"0.1.0\"\n";
        let bytes = tar_bytes(&[
            ("Cargo.toml", cargo, tar::EntryType::Regular),
            ("Cargo.lock", lock, tar::EntryType::Regular),
            ("src/main.rs", main, tar::EntryType::Regular),
        ]);
        let source = self.root.path().join("source.tar.gz");
        fs::write(&source, &bytes).expect("pinned source archive");
        self.catalog["tools"][0]["assets"] = json!({});
        self.catalog["tools"][0]["source"] = json!({"url":format!("file://{}",source.display()),"sha256":digest(&bytes),"cargo_lock_sha256":lock_digest});
    }

    fn source_failure(&self, code: i32, message: &str) {
        self.init();
        let output = self.run(&["--from-source", "prewarm", "--tool", BINARY]);
        assert_eq!(
            output.status.code(),
            Some(code),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(message),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn expect_failure(&self, code: i32, message: &str) {
        self.init();
        let output = self.prewarm();
        assert_eq!(
            output.status.code(),
            Some(code),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(message),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let offline = self.run(&["--offline", "prewarm", "--tool", BINARY]);
        assert!(
            !offline.status.success(),
            "failed distribution must not become runnable offline"
        );
    }
}

#[test]
fn test_tar_default_format_extracts_nested_executable_and_reuses_offline_cache() {
    let mut fixture = Distribution::new();
    let bytes = executable();
    fixture.set_archive(
        &tar_bytes(&[
            ("release/", b"", tar::EntryType::Directory),
            ("release/cpp-infra-style", &bytes, tar::EntryType::Regular),
        ]),
        None,
    );
    fixture.init();
    let output = fixture.prewarm();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    fs::write(fixture.root.path().join("asset"), b"upstream unavailable").expect("replace upstream asset");
    let output = fixture.run(&["--offline", "run", "--tool", BINARY, "--", "--version", "--json"]);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let identity: Value = serde_json::from_slice(&output.stdout).expect("version JSON");
    assert_eq!(identity["revision"], REVISION);
}

#[test]
fn test_zip_extracts_directory_and_nested_executable() {
    let mut fixture = Distribution::new();
    fixture.set_archive(
        &zip_bytes(&[("release/cpp-infra-style", &executable())], true),
        Some("zip"),
    );
    fixture.init();
    let output = fixture.prewarm();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(
        fixture
            .run(&["--offline", "prewarm", "--tool", BINARY])
            .status
            .success()
    );
}

#[test]
fn test_tar_rejects_duplicate_file_names() {
    let mut fixture = Distribution::new();
    fixture.set_archive(
        &tar_bytes(&[
            (BINARY, &executable(), tar::EntryType::Regular),
            (BINARY, b"second", tar::EntryType::Regular),
        ]),
        Some("tar.gz"),
    );
    fixture.expect_failure(2, "duplicate archive path");
}

#[test]
fn test_tar_rejects_hard_links() {
    let mut fixture = Distribution::new();
    fixture.set_archive(&tar_bytes(&[(BINARY, b"", tar::EntryType::Link)]), Some("tar.gz"));
    fixture.expect_failure(2, "unsupported archive entry");
}

#[test]
fn test_tar_rejects_fifo_entries() {
    let mut fixture = Distribution::new();
    fixture.set_archive(&tar_bytes(&[(BINARY, b"", tar::EntryType::Fifo)]), Some("tar.gz"));
    fixture.expect_failure(2, "unsupported archive entry");
}

#[test]
fn test_zip_rejects_parent_traversal_before_cache_installation() {
    let mut fixture = Distribution::new();
    fixture.set_archive(
        &zip_bytes(&[("../escape", b"unsafe"), (BINARY, &executable())], false),
        Some("zip"),
    );
    fixture.expect_failure(2, "unsafe relative path");
    assert!(!fixture.cache.path().join("escape").exists());
}

#[test]
fn test_zip_rejects_absolute_paths() {
    let mut fixture = Distribution::new();
    fixture.set_archive(&zip_bytes(&[("/absolute", b"unsafe")], false), Some("zip"));
    fixture.expect_failure(2, "unsafe relative path");
}

#[test]
fn test_archive_requires_one_matching_executable() {
    let mut fixture = Distribution::new();
    fixture.set_archive(&zip_bytes(&[("readme.txt", b"no executable")], false), Some("zip"));
    fixture.expect_failure(2, "exactly one matching executable");
}

#[test]
fn test_archive_rejects_ambiguous_executable_basenames() {
    let mut fixture = Distribution::new();
    let bytes = executable();
    fixture.set_archive(
        &zip_bytes(
            &[("one/cpp-infra-style", &bytes), ("two/cpp-infra-style", &bytes)],
            false,
        ),
        Some("zip"),
    );
    fixture.expect_failure(2, "exactly one matching executable");
}

#[test]
fn test_zip_rejects_corrupt_archive() {
    let mut fixture = Distribution::new();
    fixture.set_archive(b"not a zip archive", Some("zip"));
    fixture.expect_failure(2, "zip archive");
}

#[test]
fn test_extracted_executable_digest_is_verified_separately_from_archive_digest() {
    let mut fixture = Distribution::new();
    fixture.set_archive(&zip_bytes(&[(BINARY, b"tampered executable")], false), Some("zip"));
    fixture.expect_failure(2, "SHA-256");
}

#[test]
fn test_distribution_rejects_wrong_version_identity() {
    let mut fixture = Distribution::new();
    let bytes = String::from_utf8(executable())
        .expect("script")
        .replace("0.1.0", "9.9.9")
        .into_bytes();
    fixture.set_archive(&bytes, Some("raw"));
    fixture.asset()["executable_sha256"] = json!(digest(&bytes));
    fixture.expect_failure(2, "version/revision/protocol mismatch");
}

#[test]
fn test_distribution_rejects_failed_version_query() {
    let mut fixture = Distribution::new();
    let bytes = b"#!/usr/bin/env python3\nraise SystemExit(7)\n";
    fixture.set_archive(bytes, Some("raw"));
    fixture.asset()["executable_sha256"] = json!(digest(bytes));
    fixture.expect_failure(2, "version query failed");
}

#[test]
fn test_distribution_rejects_invalid_version_json() {
    let mut fixture = Distribution::new();
    let bytes = b"#!/usr/bin/env python3\nprint('not-json')\n";
    fixture.set_archive(bytes, Some("raw"));
    fixture.asset()["executable_sha256"] = json!(digest(bytes));
    fixture.expect_failure(2, "tool version");
}

#[test]
fn test_release_download_rejects_unencrypted_remote_origin() {
    let mut fixture = Distribution::new();
    fixture.asset()["url"] = json!("http://example.invalid/asset");
    fixture.expect_failure(2, "release sources require HTTPS");
}

#[test]
fn test_release_download_rejects_invalid_url() {
    let mut fixture = Distribution::new();
    fixture.asset()["url"] = json!(":invalid-url");
    fixture.expect_failure(2, "invalid release URL");
}

#[test]
fn test_http_missing_release_fails_without_publishing_cache() {
    let mut fixture = Distribution::new();
    let listener = TcpListener::bind("127.0.0.1:0").expect("HTTP fixture");
    fixture.asset()["url"] = json!(format!("http://{}/missing", listener.local_addr().expect("address")));
    fixture.init();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("HTTP request");
        read_request_headers(&mut stream);
        stream
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .expect("HTTP response");
    });
    let output = fixture.prewarm();
    server.join().expect("HTTP server");
    assert_eq!(output.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&output.stderr).contains("HTTP 404"));
    assert_eq!(
        fixture.run(&["--offline", "prewarm", "--tool", BINARY]).status.code(),
        Some(3)
    );
}

#[test]
fn test_http_transient_status_retries_then_publishes_verified_offline_cache() {
    let mut fixture = Distribution::new();
    let listener = TcpListener::bind("127.0.0.1:0").expect("HTTP fixture");
    fixture.asset()["url"] = json!(format!("http://{}/asset", listener.local_addr().expect("address")));
    fixture.init();
    let server = thread::spawn(move || {
        for attempt in 0..2 {
            let (mut stream, _) = listener.accept().expect("HTTP request");
            read_request_headers(&mut stream);
            if attempt == 0 {
                stream
                    .write_all(b"HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .expect("transient HTTP response");
            } else {
                let bytes = executable();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    bytes.len()
                )
                .expect("success headers");
                stream.write_all(&bytes).expect("asset response");
            }
        }
    });
    let output = fixture.prewarm();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    server.join().expect("two HTTP attempts");
    let output = fixture.run(&["--offline", "run", "--tool", BINARY, "--", "--version", "--json"]);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let identity: Value = serde_json::from_slice(&output.stdout).expect("version JSON");
    assert_eq!(identity["name"], BINARY);
}

#[test]
fn test_init_rejects_distribution_without_platform_asset_or_pinned_source() {
    let mut fixture = Distribution::new();
    fixture.catalog["tools"][0]["assets"] = json!({});
    let output = fixture.run(&["init"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("has no asset for cpp-infra-style"));
    assert!(!fixture.project.path().join(".infra/tools/tools.lock.json").exists());
}

#[test]
fn test_source_fallback_verifies_cargo_lock_digest_before_building() {
    let mut fixture = Distribution::new();
    fixture.pinned_source(b"fn main() {}", &digest(b"different lock"));
    fixture.source_failure(2, "SHA-256");
}

#[test]
fn test_source_fallback_propagates_real_cargo_build_failure() {
    let mut fixture = Distribution::new();
    let lock = b"version = 4\n[[package]]\nname = \"distribution-source\"\nversion = \"0.1.0\"\n";
    fixture.pinned_source(
        b"compile_error!(\"intentional invalid source fixture\"); fn main() {}",
        &digest(lock),
    );
    fixture.source_failure(1, "pinned source build failed");
}

#[test]
fn test_offline_source_fallback_reports_missing_build_cache() {
    let mut fixture = Distribution::new();
    fixture.pinned_source(b"fn main() {}", &digest(b"unused in offline mode"));
    fixture.init();
    let output = fixture.run(&["--offline", "--from-source", "prewarm", "--tool", BINARY]);
    assert_eq!(output.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&output.stderr).contains("pinned source build cache missing"));
}

#[test]
fn test_zip_rejects_symbolic_link_entries() {
    let mut fixture = Distribution::new();
    let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
    archive
        .add_symlink(BINARY, "../outside", SimpleFileOptions::default())
        .expect("zip symbolic link");
    let bytes = archive.finish().expect("zip finish").into_inner();
    fixture.set_archive(&bytes, Some("zip"));
    fixture.expect_failure(2, "unsupported zip entry type");
}

#[test]
fn test_offline_cached_executable_is_rehashed_before_execution() {
    let fixture = Distribution::new();
    fixture.init();
    let output = fixture.prewarm();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let report = fixture.project.path().join("build/prewarm.json");
    let output = fixture.run(&[
        "--report",
        report.to_str().expect("report path"),
        "prewarm",
        "--tool",
        BINARY,
    ]);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let report: Value = serde_json::from_slice(&fs::read(report).expect("prewarm report")).expect("report JSON");
    let binary = report["commands"][0]["program"]
        .as_str()
        .expect("version command executable path");
    assert!(
        std::path::Path::new(binary).starts_with(fixture.cache.path()),
        "fixture cache must own the executable"
    );
    fs::write(binary, b"tampered cached binary").expect("corrupt cached executable");
    let output = fixture.run(&["--offline", "prewarm", "--tool", BINARY]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("SHA-256"));
}

#[test]
fn test_source_fallback_checks_declared_executable_digest_after_real_build() {
    let mut fixture = Distribution::new();
    let lock = b"version = 4\n[[package]]\nname = \"distribution-source\"\nversion = \"0.1.0\"\n";
    fixture.pinned_source(b"fn main() {}", &digest(lock));
    fixture.catalog["tools"][0]["source"]["executable_sha256"] = json!(digest(b"different executable"));
    fixture.source_failure(2, "SHA-256");
}

#[test]
fn test_stable_channel_default_catalog_initializes_verified_lock_and_runs_offline() {
    let fixture = Distribution::new();
    let channel = fixture.channel();
    let output = fixture.run_channel(&channel, &["init"]);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let lock = fixture.lock();
    assert_eq!(lock["bundle_id"], channel["bundle_id"]);
    assert_eq!(lock["catalog_url"], channel["catalog"]["url"]);
    assert_eq!(lock["catalog_sha256"], channel["catalog"]["sha256"]);
    assert_eq!(lock["tools"][0]["revision"], REVISION);
    let output = fixture.run_channel(&channel, &["prewarm", "--tool", BINARY]);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let output = fixture.run_channel(
        &channel,
        &["--offline", "run", "--tool", BINARY, "--", "--version", "--json"],
    );
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let version: Value = serde_json::from_slice(&output.stdout).expect("offline tool version");
    assert_eq!(version["revision"], REVISION);
}

#[test]
fn test_stable_channel_update_selects_explicit_bundle_catalog() {
    let fixture = Distribution::new();
    let mut channel = fixture.channel();
    let mut alternate = fixture.catalog.clone();
    alternate["bundle_id"] = json!("previous-stable-bundle");
    let reference = fixture.catalog_reference("previous-catalog.json", &alternate);
    channel["bundles"] = json!({"previous-stable-bundle":reference});
    let output = fixture.run_channel(&channel, &["init"]);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let output = fixture.run_channel(&channel, &["update", "--bundle", "previous-stable-bundle"]);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let lock = fixture.lock();
    assert_eq!(lock["bundle_id"], "previous-stable-bundle");
    assert_eq!(lock["catalog_url"], reference["url"]);
    assert_eq!(lock["catalog_sha256"], reference["sha256"]);
    assert!(fixture.run_channel(&channel, &["--offline", "check"]).status.success());
}

#[test]
fn test_stable_channel_explicit_default_bundle_uses_primary_catalog() {
    let fixture = Distribution::new();
    let channel = fixture.channel();
    let output = fixture.run_channel(&channel, &["init"]);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let output = fixture.run_channel(&channel, &["update", "--bundle", "distribution-fixture"]);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(fixture.lock()["catalog_url"], channel["catalog"]["url"]);
}

#[test]
fn test_stable_channel_rejects_unlisted_explicit_bundle_without_changing_lock() {
    let fixture = Distribution::new();
    let channel = fixture.channel();
    fixture.init();
    let before = fs::read(fixture.project.path().join(".infra/tools/tools.lock.json")).expect("initial lock bytes");
    let output = fixture.run_channel(&channel, &["update", "--bundle", "unlisted-bundle"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("bundle unlisted-bundle is not listed in the stable channel")
    );
    assert_eq!(
        fs::read(fixture.project.path().join(".infra/tools/tools.lock.json")).expect("preserved lock bytes"),
        before
    );
}

#[test]
fn test_stable_channel_rejects_unsupported_schema_before_initialization() {
    let fixture = Distribution::new();
    let mut channel = fixture.channel();
    channel["schema"] = json!(2);
    fixture.channel_init_failure(&channel, "unsupported stable channel schema");
}

#[test]
fn test_stable_channel_rejects_unknown_fields() {
    let fixture = Distribution::new();
    let mut channel = fixture.channel();
    channel["unexpected"] = json!(true);
    fixture.channel_init_failure(&channel, "unknown field `unexpected`");
}

#[test]
fn test_stable_channel_rejects_catalog_bundle_identity_mismatch() {
    let fixture = Distribution::new();
    let mut channel = fixture.channel();
    channel["bundle_id"] = json!("different-bundle-identity");
    fixture.channel_init_failure(&channel, "channel/catalog bundle mismatch");
}

#[test]
fn test_stable_channel_verifies_referenced_catalog_digest_before_initialization() {
    let fixture = Distribution::new();
    let mut channel = fixture.channel();
    channel["catalog"]["sha256"] = json!(digest(b"different catalog bytes"));
    fixture.channel_init_failure(&channel, "SHA-256");
}

#[test]
fn test_stable_channel_rejects_referenced_catalog_schema() {
    let fixture = Distribution::new();
    let mut channel = fixture.channel();
    let mut catalog = fixture.catalog.clone();
    catalog["schema"] = json!(2);
    channel["catalog"] = fixture.catalog_reference("invalid-schema-catalog.json", &catalog);
    fixture.channel_init_failure(&channel, "unsupported catalog schema/protocol");
}

#[test]
fn test_stable_channel_explicit_bundle_reference_must_match_selected_identity() {
    let fixture = Distribution::new();
    let mut channel = fixture.channel();
    channel["bundles"] = json!({"wrongly-indexed-bundle":channel["catalog"]});
    fixture.init();
    let before = fixture.lock();
    let output = fixture.run_channel(&channel, &["update", "--bundle", "wrongly-indexed-bundle"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("channel/catalog bundle mismatch"));
    assert_eq!(fixture.lock(), before);
}

#[test]
fn test_raw_duplicate_zip_names_are_rejected_in_normal_sfx_and_zip64_archives() {
    fn zip64_directory(mut bytes: Vec<u8>) -> Vec<u8> {
        let end = bytes.len() - 22;
        let count = u64::from(u16::from_le_bytes(
            bytes[end + 10..end + 12].try_into().expect("entry count"),
        ));
        let size = u64::from(u32::from_le_bytes(
            bytes[end + 12..end + 16].try_into().expect("directory size"),
        ));
        let offset = u64::from(u32::from_le_bytes(
            bytes[end + 16..end + 20].try_into().expect("directory offset"),
        ));
        bytes.truncate(end);
        bytes.extend_from_slice(&0x06064b50u32.to_le_bytes());
        bytes.extend_from_slice(&44u64.to_le_bytes());
        bytes.extend_from_slice(&45u16.to_le_bytes());
        bytes.extend_from_slice(&45u16.to_le_bytes());
        bytes.extend_from_slice(&[0; 8]);
        for value in [count, count, size, offset] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(&0x07064b50u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&(end as u64).to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&0x06054b50u32.to_le_bytes());
        bytes.extend_from_slice(&[0; 4]);
        bytes.extend_from_slice(&[0xff; 12]);
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes
    }
    for (zip64, sfx) in [(false, false), (false, true), (true, false)] {
        for duplicate in [false, true] {
            let mut fixture = Distribution::new();
            let contents = executable();
            let mut entries = vec![(BINARY, contents.as_slice())];
            if duplicate {
                entries.push(("cpp-infra-stylx", contents.as_slice()));
            }
            let mut bytes = zip_bytes(&entries, false);
            if duplicate {
                let original = b"cpp-infra-stylx";
                let offsets = bytes
                    .windows(original.len())
                    .enumerate()
                    .filter_map(|(index, value)| (value == original).then_some(index))
                    .collect::<Vec<_>>();
                assert_eq!(offsets.len(), 2, "one local and one central raw filename");
                for offset in offsets {
                    bytes[offset..offset + original.len()].copy_from_slice(BINARY.as_bytes());
                }
            }
            if zip64 {
                bytes = zip64_directory(bytes);
            }
            if sfx {
                bytes.splice(..0, b"fixture self-extractor prefix\n".iter().copied());
            }
            fixture.set_archive(&bytes, Some("zip"));
            if duplicate {
                fixture.expect_failure(2, "duplicate archive path");
            } else {
                fixture.init();
                let result = fixture.prewarm();
                assert!(
                    result.status.success(),
                    "zip64={zip64},sfx={sfx}: {}",
                    String::from_utf8_lossy(&result.stderr)
                );
                let result = fixture.run(&["--offline", "run", "--tool", BINARY, "--", "--version", "--json"]);
                assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
                let identity: Value = serde_json::from_slice(&result.stdout).expect("version JSON");
                assert_eq!(identity["revision"], REVISION);
            }
        }
    }
}
