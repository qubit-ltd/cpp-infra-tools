// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
#![cfg(unix)]

use std::env;
use std::ffi::OsString;
use std::fs;
use std::fs::OpenOptions;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;
use std::process::Child;
use std::process::Command;
use std::process::Output;
use std::process::Stdio;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use fs2::FileExt;
use serde_json::Value;
use serde_json::json;
use sha2::Digest;
use sha2::Sha256;
use tempfile::TempDir;

const TIMEOUT: Duration = Duration::from_secs(30);

fn write_json(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec_pretty(value).expect("fixture JSON")).expect("write fixture JSON");
}

fn command(project: &Path, locked: bool, report: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cpp-infra-tools"));
    command.current_dir(project).arg("--project").arg(project);
    if locked {
        command.arg("--locked");
    }
    command
        .arg("--report")
        .arg(project.join("build").join(report))
        .arg("check");
    command
}

fn fixture(locked: bool) -> TempDir {
    let project = TempDir::new().expect("isolated project");
    let root = project.path();
    for relative in [".infra/style/policy", ".infra/tools", "src", "build"] {
        fs::create_dir_all(root.join(relative)).expect("fixture directory");
    }
    fs::write(
        root.join("CMakeLists.txt"),
        "cmake_minimum_required(VERSION 3.25)\nproject(lock_fixture)\n",
    )
    .expect("CMake project");
    fs::write(
        root.join(".infra/project.toml"),
        "schema=1\nkind='library'\nsource_roots=['src']\n",
    )
    .expect("project configuration");
    fs::write(root.join(".infra/style/config.toml"), "schema=1\n").expect("tool configuration");

    let padding = b"fixed managed policy\n";
    fs::write(root.join(".infra/style/policy/padding.txt"), padding).expect("managed padding");
    fs::write(root.join("src/input.txt"), "project input\n").expect("project input");
    write_json(
        &root.join(".infra/managed-files.json"),
        &json!({"schema":1,"owner":"cpp-infra-tools","files":[{"target":".infra/style/policy/padding.txt","source":"publication-lock-fixture","sha256":hex::encode(Sha256::digest(padding)),"executable":false}]}),
    );
    let revision = "1111111111111111111111111111111111111111";
    if locked {
        write_json(
            &root.join(".infra/tools/tools.lock.json"),
            &json!({"schema":1,"bundle_id":"publication-lock-fixture","protocol_major":1,"tools":[{"name":"cpp-infra-tools","version":"0.1.0","revision":revision,"protocol":1}],"catalog_url":"file:///publication-lock-fixture.json","catalog_sha256":"0".repeat(64)}),
        );
    } else {
        let pairs = vec![("cpp-infra-tools", revision)];
        let bundle = format!(
            "main-{}",
            hex::encode(Sha256::digest(
                serde_json::to_vec_pretty(&pairs).expect("canonical revision pairs")
            ))
        );
        let repository = root.join("upstream.git");
        write_json(
            &root.join(".infra/tools/main-snapshot.json"),
            &json!({"schema":1,"bundle_id":bundle,"tools":[],"revisions":{"schema":1,"tools":[{"name":"cpp-infra-tools","repository":repository,"revision":revision}]},"offline":false,"stale":false,"host":"fixture","rustc":"fixture"}),
        );
    }
    project
}

struct RunningCheck {
    child: Option<Child>,
    release: PathBuf,
}

impl RunningCheck {
    fn wait(mut self) -> Output {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if self
                .child
                .as_mut()
                .expect("running manager")
                .try_wait()
                .expect("manager status")
                .is_some()
            {
                return self
                    .child
                    .take()
                    .expect("completed manager")
                    .wait_with_output()
                    .expect("manager output");
            }
            assert!(Instant::now() < deadline, "manager did not complete within {TIMEOUT:?}");
            thread::sleep(Duration::from_millis(1));
        }
    }
}

impl Drop for RunningCheck {
    fn drop(&mut self) {
        let _ = fs::write(&self.release, "release");
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

struct PublicationGate {
    _directory: TempDir,
    path: OsString,
    ready: PathBuf,
    release: PathBuf,
}

impl PublicationGate {
    fn new() -> Self {
        let directory = TempDir::new().expect("external publication gate");
        let bin = directory.path().join("bin");
        fs::create_dir(&bin).expect("gate executable directory");
        let ready = directory.path().join("ready");
        let release = directory.path().join("release");
        let original_path = env::var_os("PATH").expect("process executable path");
        let git = env::split_paths(&original_path)
            .map(|directory| directory.join("git"))
            .find(|candidate| candidate.is_file())
            .expect("real Git executable")
            .canonicalize()
            .expect("absolute Git executable");
        let quoted =
            |path: &Path| serde_json::to_string(path.to_str().expect("UTF-8 fixture path")).expect("quoted path");
        // Pause only the normal metadata subprocess at publication, then execute
        // real Git with its original arguments, output and exit status.
        let script = format!(
            "#!/usr/bin/env python3\nimport os,sys,time\nfrom pathlib import Path\nif sys.argv[1:] == ['rev-parse','HEAD']:\n    parent=os.getppid()\n    ready=Path({})\n    release=Path({})\n    ready.write_text('ready')\n    deadline=time.monotonic()+30\n    while not release.exists():\n        if os.getppid() != parent: sys.exit(125)\n        if time.monotonic() >= deadline: sys.exit(124)\n        time.sleep(0.001)\n    if os.getppid() != parent: sys.exit(125)\nos.execv({}, [{}, *sys.argv[1:]])\n",
            quoted(&ready),
            quoted(&release),
            quoted(&git),
            quoted(&git),
        );
        let wrapper = bin.join("git");
        fs::write(&wrapper, script).expect("publication gate script");
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).expect("executable gate");
        let path = env::join_paths(std::iter::once(bin).chain(env::split_paths(&original_path)))
            .expect("gate executable path");
        Self {
            _directory: directory,
            path,
            ready,
            release,
        }
    }

    fn wait_until_ready(&self, child: &mut Child) -> bool {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if self.ready.is_file() {
                return true;
            }
            if child.try_wait().expect("manager status at publication gate").is_some() || Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(1));
        }
    }
}

#[test]
fn test_project_lock_protects_the_published_report_identity() {
    for locked in [false, true] {
        let project = fixture(locked);
        let root = project.path();
        let baseline = command(root, locked, "baseline.json").output().expect("baseline check");
        assert!(
            baseline.status.success(),
            "{}",
            String::from_utf8_lossy(&baseline.stderr)
        );
        let baseline: Value =
            serde_json::from_slice(&fs::read(root.join("build/baseline.json")).expect("baseline report"))
                .expect("baseline JSON");
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join("build/.cpp-infra-project.lock"))
            .expect("project lock");
        let gate = PublicationGate::new();
        let child = command(root, locked, "race.json")
            .env("PATH", &gate.path)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("concurrent check");
        let mut child = RunningCheck {
            child: Some(child),
            release: gate.release.clone(),
        };
        let ready = gate.wait_until_ready(child.child.as_mut().expect("running manager"));
        let protected = match FileExt::try_lock_exclusive(&lock) {
            Ok(()) => {
                FileExt::unlock(&lock).expect("release unprotected observer lock");
                false
            }
            Err(error) => error.kind() == io::ErrorKind::WouldBlock,
        };
        let report_path = root.join("build/race.json");
        let config_path = root.join(".infra/style/config.toml");
        let writer = thread::spawn(move || {
            let deadline = Instant::now() + TIMEOUT;
            loop {
                match FileExt::try_lock_exclusive(&lock) {
                    Ok(()) => break,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                        thread::sleep(Duration::from_millis(1));
                    }
                    Err(error) => panic!("writer could not obtain the project lock: {error}"),
                }
            }
            let published = fs::read(&report_path)
                .ok()
                .map(|bytes| serde_json::from_slice::<Value>(&bytes).expect("published JSON"));
            fs::write(config_path, "schema=2\n").expect("writer changes configuration under exclusive lock");
            FileExt::unlock(&lock).expect("release writer lock");
            published
        });
        fs::write(&gate.release, "release").expect("release publication subprocess");
        let result = child.wait();
        let observed = writer.join().expect("configuration writer");
        assert!(
            ready,
            "manager must reach the external metadata call during report publication"
        );
        assert!(
            protected,
            "the project lock must remain held while the report is being published"
        );
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
        let published = observed.expect("the report must be published before an exclusive writer can enter");
        assert_eq!(published["status"], "passed");
        assert_eq!(
            published["config_digest"], baseline["config_digest"],
            "a passed report must identify the configuration actually checked"
        );
        assert_eq!(
            fs::read_to_string(root.join(".infra/style/config.toml")).expect("writer configuration"),
            "schema=2\n"
        );
        let fresh = command(root, locked, "fresh.json")
            .output()
            .expect("fresh check after writer");
        assert_eq!(fresh.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&fresh.stderr).contains("tool configuration must use schema = 1"));
    }
}
