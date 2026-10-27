// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
use std::fs;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Output;
use std::process::Stdio;

use serde_json::Value;
use serde_json::json;
use sha2::Digest;
use sha2::Sha256;
use tempfile::TempDir;
const NAMES: &[&str] = &[
    "tools",
    "ci",
    "verify",
    "style",
    "analyze",
    "dependency",
    "coverage",
    "pages",
];
fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
struct Fixture {
    sources: TempDir,
    project: TempDir,
    cache: TempDir,
}
impl Fixture {
    fn new() -> Self {
        let fixture = Self {
            sources: TempDir::new().expect("sources"),
            project: TempDir::new().expect("project"),
            cache: TempDir::new().expect("cache"),
        };
        fs::write(
            fixture.project.path().join("CMakeLists.txt"),
            "cmake_minimum_required(VERSION 3.25)\nproject(main_fixture)\n",
        )
        .expect("CMake");
        fs::create_dir(fixture.project.path().join("src")).expect("src");
        for role in NAMES {
            fixture.publish(role, "initial");
        }
        fixture
    }
    fn repository(&self, role: &str) -> PathBuf {
        self.sources.path().join(format!("cpp-infra-{role}.git"))
    }
    fn publish(&self, role: &str, content: &str) {
        let repo = self.repository(role);
        fs::create_dir_all(&repo).expect("repo dir");
        if !repo.join(".git").is_dir() {
            assert!(
                Command::new("git")
                    .args(["init", "--quiet", "--initial-branch=main"])
                    .arg(&repo)
                    .status()
                    .expect("git init")
                    .success()
            );
        }
        let package = format!("qubit-cpp-infra-{role}");
        let name = format!("cpp-infra-{role}");
        let cargo = format!(
            "[package]\nname=\"{package}\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[[bin]]\nname=\"{name}\"\npath=\"src/main.rs\"\n"
        );
        let lock = format!("version = 4\n[[package]]\nname = \"{package}\"\nversion = \"0.1.0\"\n");
        let mut source = format!(
            r##"fn main() {{
let args:Vec<String>=std::env::args().skip(1).collect();
if args.iter().any(|arg|arg=="--version") {{println!("{{{{\"name\":\"{name}\",\"version\":\"0.1.0\",\"revision\":\"{{}}\",\"protocol\":1}}}}",env!("CPP_INFRA_BUILD_REVISION"));return;}}
let script=r#"import json,os,sys
args=sys.argv[1:]
c=json.load(open(args[args.index('--run-context')+1]))
c.update(tool={{'name':'{name}','version':'0.1.0','revision':os.environ['FIXTURE_REVISION'],'protocol':1}},status='passed',source_revision=None,started_at='2026-01-01T00:00:00Z',duration_ms=0,external_versions={{}},commands=[],diagnostics=[],artifacts=[])
if os.environ.get('CPP_INFRA_FIXTURE_EXIT'): c['status']='failed'
json.dump(c,open(args[args.index('--report')+1],'w'))
if os.environ.get('CPP_INFRA_FIXTURE_EXIT'): sys.exit(int(os.environ['CPP_INFRA_FIXTURE_EXIT']))
print('executed-{content}')
"#;
let status=std::process::Command::new("python3").arg("-c").arg(script).args(args).env("FIXTURE_REVISION",env!("CPP_INFRA_BUILD_REVISION")).status().unwrap();std::process::exit(status.code().unwrap_or(1));
}}
"##
        );
        source = source.replace(
            "let script=r#",
            "if args.iter().any(|arg|arg==\"--help\"||arg==\"-h\"){println!(\"fixture help\");return;}\nlet script=r#",
        );
        if role == "tools" {
            source=source.replace("let script=r#",&format!("if !args.iter().any(|a|a==\"--run-context\"){{println!(\"manager-{content}\");return;}}\nlet script=r#"));
        }
        let mutation = r####"if os.environ.get('CPP_INFRA_FIXTURE_MUTATE'):
 from pathlib import Path
 import hashlib,struct
 root=Path(args[args.index('--project')+1])
 target=root/os.environ['CPP_INFRA_FIXTURE_MUTATE']
 target.parent.mkdir(parents=True,exist_ok=True)
 target.write_text('// formatted\nint value=1;\n')
 def digest(config):
  entries=[]
  for base,dirs,files in os.walk(root,followlinks=False):
   relative=Path(base).relative_to(root)
   if relative==Path('.'): dirs[:]=[d for d in dirs if d not in ('.git','build','target')]
   if relative==Path('.infra'): dirs[:]=[d for d in dirs if d not in ('.tmp','transactions','.transactions')]
   for name in files:
    path=Path(base)/name
    if path.is_symlink(): continue
    rel=path.relative_to(root).as_posix()
    cfg=rel.startswith('.infra/') or rel.startswith('conan/') or rel in ('CMakeLists.txt','CMakePresets.json','conanfile.py','conanfile.txt')
    if config and not cfg: continue
    if not config and (rel.startswith('.infra/') or rel in c.get('source_excluded_paths',[])): continue
    entries.append((rel,path.read_bytes()))
  result=hashlib.sha256()
  for rel,contents in sorted(entries):
   encoded=rel.encode();result.update(struct.pack('<Q',len(encoded)));result.update(encoded);result.update(struct.pack('<Q',len(contents)));result.update(contents)
  return result.hexdigest()
 c['source_digest']=digest(False)
 c['config_digest']=digest(True)
"####;
        source = source.replace("c.update(tool=", &(mutation.to_owned() + "\nc.update(tool="));
        let policy = format!("schema = 1\n# {content}\n");
        let config = "schema = 1\n";
        let manifest = if *role == *"tools" {
            json!({"schema":1,"owner":name,"files":[]})
        } else {
            json!({"schema":1,"owner":name,"files":[{"source":"policy.toml","target":format!(".infra/{role}/policy/{}",if role=="style"&&content.contains("retire"){"renamed.toml"}else{"default.toml"}),"sha256":hash(policy.as_bytes()),"executable":false}],"project_files":[{"source":"config.toml","target":format!(".infra/{role}/config.toml"),"sha256":hash(config.as_bytes()),"executable":false}]})
        };
        let mut entries = vec![
            ("Cargo.toml", cargo.into_bytes()),
            ("Cargo.lock", lock.into_bytes()),
            ("src/main.rs", source.into_bytes()),
            ("conf/manifest.json", serde_json::to_vec(&manifest).expect("manifest")),
            ("conf/policy.toml", policy.into_bytes()),
            ("conf/config.toml", config.as_bytes().to_vec()),
        ];
        if role == "tools" {
            let root = Path::new(env!("CARGO_MANIFEST_DIR"));
            let mut definition: Value = serde_json::from_slice(
                &fs::read(root.join("assets/cpp-project/manifest.json")).expect("asset definition"),
            )
            .expect("asset JSON");
            for key in ["files", "templates"] {
                for entry in definition[key].as_array_mut().expect("asset entries") {
                    let name = entry["source"].as_str().expect("asset source").to_owned();
                    let mut bytes = fs::read(root.join(&name)).expect("declared manager source");
                    if name == "assets/cpp-project/bootstrap.py" {
                        bytes.extend(format!("\n# main fixture {content}\n").as_bytes());
                    }
                    entry["sha256"] = json!(hash(&bytes));
                    entries.push((Box::leak(name.into_boxed_str()), bytes));
                }
            }
            entries.push((
                "assets/cpp-project/manifest.json",
                serde_json::to_vec(&definition).expect("asset definition bytes"),
            ));
        }
        let previous = Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["rev-parse", "--verify", "refs/heads/main"])
            .output()
            .expect("previous ref");
        let mut input = Vec::new();
        for (index, (_, bytes)) in entries.iter().enumerate() {
            write!(&mut input, "blob\nmark :{}\ndata {}\n", index + 1, bytes.len()).expect("blob header");
            input.extend(bytes);
            input.push(b'\n');
        }
        write!(
            &mut input,
            "commit refs/heads/main\ncommitter Fixture <fixture@example.test> 1700000000 +0000\ndata {}\n{}\n",
            content.len(),
            content
        )
        .expect("commit stream");
        if previous.status.success() {
            writeln!(&mut input, "from {}", String::from_utf8_lossy(&previous.stdout).trim()).expect("parent");
        }
        input.extend(b"deleteall\n");
        for (index, (name, _)) in entries.iter().enumerate() {
            writeln!(&mut input, "M 100644 :{} {}", index + 1, name).expect("tree entry");
        }
        input.push(b'\n');
        let mut child = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["fast-import", "--quiet"])
            .stdin(Stdio::piped())
            .spawn()
            .expect("git fixture import");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(&input)
            .expect("import stream");
        assert!(child.wait().expect("import result").success());
    }
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cpp-infra-tools"));
        command
            .arg("--project")
            .arg(self.project.path())
            .env("CPP_INFRA_REPOSITORY_BASE", self.sources.path())
            .env("CPP_INFRA_CACHE_DIR", self.cache.path())
            .env("CARGO_BUILD_JOBS", "2")
            .args(args);
        command
    }
    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().expect("tools")
    }
    fn init(&self) {
        let result = self.run(&["init"]);
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    }
}
#[test]
fn test_bare_relative_report_completes_in_the_project_directory() {
    let fixture = Fixture::new();
    fixture.init();
    for _ in 0..2 {
        let output = fixture
            .command(&["--report", "result.json", "check"])
            .current_dir(fixture.project.path())
            .output()
            .expect("check with relative report");
        let report: Value = serde_json::from_slice(
            &fs::read(fixture.project.path().join("result.json")).expect("relative report bytes"),
        )
        .expect("relative report JSON");
        assert_eq!(report["status"], "passed");
        assert_eq!(
            output.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(report["source_excluded_paths"], json!(["result.json"]));
    }
}

#[test]
fn test_invalid_utf8_configuration_is_a_configuration_error() {
    let fixture = Fixture::new();
    fixture.init();
    let analyze = fixture.project.path().join(".infra/analyze/config.toml");
    fs::create_dir_all(analyze.parent().expect("config directory")).expect("config directory");
    fs::write(&analyze, "schema=1\n").expect("valid tool configuration");
    assert!(fixture.run(&["check"]).status.success());
    for (relative, args) in [
        (".infra/project.toml", vec!["check"]),
        (".infra/analyze/config.toml", vec!["check"]),
        (".infra/tools/defaults.toml", vec!["doctor", "--task", "style"]),
    ] {
        let path = fixture.project.path().join(relative);
        let valid = fs::read(&path).expect("valid configuration bytes");
        fs::write(&path, [0xff]).expect("invalid UTF-8 configuration");
        let output = fixture.run(&args);
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
    let defaults = fixture.project.path().join(".infra/tools/defaults.toml");
    fs::remove_file(&defaults).expect("replace generated fixture defaults");
    fs::create_dir(&defaults).expect("directory obstructing configuration read");
    let output = fixture.run(&["doctor", "--task", "style"]);
    assert_eq!(
        output.status.code(),
        Some(3),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(defaults.is_dir());
}
#[test]
fn test_default_init_and_update_follow_each_tools_main_and_preserve_user_config() {
    let fixture = Fixture::new();
    fixture.init();
    assert!(!fixture.project.path().join(".infra/tools/tools.lock.json").exists());
    let policy = fixture.project.path().join(".infra/style/policy/default.toml");
    assert!(fs::read_to_string(&policy).expect("policy").contains("initial"));
    let config = fixture.project.path().join(".infra/style/config.toml");
    fs::write(&config, "schema=1\ncustom=true\n").expect("user config");
    fixture.publish("style", "second");
    let result = fixture.run(&["update"]);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert!(fs::read_to_string(policy).expect("policy").contains("second"));
    assert!(fs::read_to_string(config).expect("config").contains("custom=true"));
}
#[test]
fn test_latest_builds_real_main_then_reuses_cache_and_offline_snapshot() {
    let fixture = Fixture::new();
    fixture.init();
    let result = fixture.run(&["latest", "--tool", "cpp-infra-style", "--", "check"]);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert!(String::from_utf8_lossy(&result.stdout).contains("executed-initial"));
    let report = fixture.project.path().join("build/offline.json");
    let result = fixture.run(&[
        "--offline",
        "--report",
        report.to_str().expect("path"),
        "run",
        "--tool",
        "cpp-infra-style",
        "--",
        "check",
    ]);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let report: Value = serde_json::from_slice(&fs::read(report).expect("report")).expect("json");
    assert_eq!(report["data"]["offline"], true);
    assert_eq!(report["data"]["stale"], true);
    assert!(!report["commands"].as_array().expect("commands").iter().any(|c| {
        c["args"]
            .as_array()
            .is_some_and(|args| args.iter().any(|a| a == "ls-remote" || a == "build"))
    }));
}
#[test]
fn test_run_resolves_new_main_without_modifying_project_managed_snapshot() {
    let fixture = Fixture::new();
    fixture.init();
    let managed = fs::read(fixture.project.path().join(".infra/managed-files.json")).expect("manifest");
    assert!(fixture.run(&["run", "--tool", "cpp-infra-style"]).status.success());
    fixture.publish("style", "new-main");
    let result = fixture.run(&["run", "--tool", "cpp-infra-style"]);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert!(String::from_utf8_lossy(&result.stdout).contains("executed-new-main"));
    assert_eq!(
        fs::read(fixture.project.path().join(".infra/managed-files.json")).expect("manifest"),
        managed
    );
}
#[test]
fn test_portable_revisions_fix_main_across_later_invocations() {
    let fixture = Fixture::new();
    fixture.init();
    let path = fixture.project.path().join("build/revisions.json");
    let result = fixture.run(&["resolve", "--output", path.to_str().expect("path")]);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let portable: Value = serde_json::from_slice(&fs::read(&path).expect("portable")).expect("json");
    assert_eq!(portable["tools"].as_array().expect("tools").len(), 8);
    assert!(
        portable["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .all(|t| t.get("binary").is_none())
    );
    fixture.publish("style", "later-main");
    let result = fixture
        .command(&["run", "--tool", "cpp-infra-style"])
        .env("CPP_INFRA_REVISIONS_FILE", path)
        .output()
        .expect("fixed run");
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert!(String::from_utf8_lossy(&result.stdout).contains("executed-initial"));
}
#[test]
fn test_offline_main_without_resolution_cache_fails_clearly() {
    let fixture = Fixture::new();
    let result = fixture.run(&["--offline", "run", "--tool", "cpp-infra-style"]);
    assert_eq!(result.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&result.stderr).contains("offline"));
}
#[test]
fn test_missing_main_revision_is_configuration_error_without_fake_success() {
    let fixture = Fixture::new();
    let empty = fixture.sources.path().join("cpp-infra-style.git");
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(empty)
            .args(["update-ref", "-d", "refs/heads/main"])
            .status()
            .expect("retire fixture main")
            .success()
    );
    let result = fixture.run(&["run", "--tool", "cpp-infra-style"]);
    assert_eq!(result.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&result.stderr).contains("main"));
}
#[test]
fn test_embedded_bootstrap_is_main_source_based() {
    let source = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/cpp-project/bootstrap.py"))
        .expect("bootstrap");
    assert!(source.contains("CPP_INFRA_REVISIONS_FILE"));
    assert!(source.contains("refs/heads/main"));
    assert!(source.contains("--locked"));
}

#[test]
fn test_update_installs_bootstrap_from_manager_main_revision() {
    let fixture = Fixture::new();
    fixture.init();
    let path = fixture.project.path().join(".infra/bootstrap.py");
    assert!(
        fs::read_to_string(&path)
            .expect("bootstrap")
            .contains("# main fixture initial")
    );
    fixture.publish("tools", "bootstrap-next");
    let result = fixture.run(&["update"]);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert!(
        fs::read_to_string(path)
            .expect("new bootstrap")
            .contains("# main fixture bootstrap-next")
    );
}

#[test]
fn test_manager_accepts_main_runtime_context_and_rejects_tampered_snapshot() {
    let fixture = Fixture::new();
    fixture.init();
    let report = fixture.project.path().join("build/main-run.json");
    let result = fixture.run(&[
        "--report",
        report.to_str().expect("path"),
        "run",
        "--tool",
        "cpp-infra-style",
    ]);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let report: Value = serde_json::from_slice(&fs::read(report).expect("report")).expect("JSON");
    let context = PathBuf::from(report["data"]["run_context"].as_str().expect("context"));
    let result = fixture.run(&["--run-context", context.to_str().expect("path"), "check"]);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    fixture.publish("style", "after-context");
    let result = fixture.run(&[
        "--run-context",
        context.to_str().expect("path"),
        "run",
        "--tool",
        "cpp-infra-style",
    ]);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert!(
        String::from_utf8_lossy(&result.stdout).contains("executed-initial"),
        "a supplied context must preserve its revision group"
    );
    fs::write(
        Path::new(report["data"]["runtime_snapshot"].as_str().expect("snapshot")),
        "{}",
    )
    .expect("tampered snapshot");
    let result = fixture.run(&["--run-context", context.to_str().expect("path"), "check"]);
    assert_eq!(result.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&result.stderr).contains("SHA-256"));
}

#[test]
fn test_runtime_context_requires_typed_identity_and_safe_snapshot_evidence() {
    let fixture = Fixture::new();
    fixture.init();
    let report_path = fixture.project.path().join("build/context-origin.json");
    let result = fixture.run(&[
        "--report",
        report_path.to_str().expect("report path"),
        "run",
        "--tool",
        "cpp-infra-style",
    ]);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let report: Value = serde_json::from_slice(&fs::read(report_path).expect("origin report")).expect("JSON");
    let original: Value = serde_json::from_slice(
        &fs::read(report["data"]["run_context"].as_str().expect("context path")).expect("context"),
    )
    .expect("JSON");
    let input = fixture.project.path().join("build/invalid-context.json");
    for (field, value, expected) in [
        ("schema", json!(2), "unsupported run context schema"),
        ("project", Value::Null, "missing context project"),
        ("bundle_id", Value::Null, "missing context bundle_id"),
        ("run_id", Value::Null, "missing context run_id"),
        ("task_id", Value::Null, "missing context task_id"),
        ("report_root", Value::Null, "missing context report_root"),
        (
            "report_root",
            json!(fixture.sources.path()),
            "outside project run roots",
        ),
        ("run_id", json!("../foreign"), "unsafe relative path"),
        ("config_digest", json!("0".repeat(64)), "stale run context"),
        ("source_excluded_paths", json!(false), "source_excluded_paths"),
        (
            "runtime_snapshot",
            json!(fixture.sources.path().join("foreign.json")),
            "outside report_root",
        ),
        (
            "runtime_snapshot_digest",
            Value::Null,
            "missing runtime snapshot digest",
        ),
    ] {
        let mut context = original.clone();
        context[field] = value;
        fs::write(&input, serde_json::to_vec(&context).expect("context JSON")).expect("context input");
        let result = fixture.run(&["--run-context", input.to_str().expect("context path"), "check"]);
        assert_eq!(
            result.status.code(),
            Some(2),
            "{field}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(
            String::from_utf8_lossy(&result.stderr).contains(expected),
            "{field}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let snapshot_path = PathBuf::from(original["runtime_snapshot"].as_str().expect("snapshot path"));
    let snapshot: Value = serde_json::from_slice(&fs::read(&snapshot_path).expect("snapshot bytes")).expect("JSON");
    for (field, value, expected) in [
        ("schema", json!(2), "runtime snapshot schema/bundle mismatch"),
        ("bundle_id", json!("foreign"), "runtime snapshot schema/bundle mismatch"),
        ("tools", json!({}), "runtime snapshot tools is not an array"),
    ] {
        let mut invalid = snapshot.clone();
        invalid[field] = value;
        let bytes = serde_json::to_vec(&invalid).expect("snapshot JSON");
        fs::write(&snapshot_path, &bytes).expect("snapshot input");
        let mut context = original.clone();
        context["runtime_snapshot_digest"] = json!(hash(&bytes));
        fs::write(&input, serde_json::to_vec(&context).expect("context JSON")).expect("context input");
        let result = fixture.run(&["--run-context", input.to_str().expect("context path"), "check"]);
        assert_eq!(result.status.code(), Some(2));
        assert!(
            String::from_utf8_lossy(&result.stderr).contains(expected),
            "{field}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let manager = Path::new(env!("CARGO_BIN_EXE_cpp-infra-tools"))
        .canonicalize()
        .expect("manager executable");
    let entry = json!({"name":"cpp-infra-tools","version":"0.1.0","revision":null,"protocol":1,"binary":manager,"executable_sha256":hash(&fs::read(&manager).expect("manager bytes"))});
    for (field, value, expected_code, expected) in [
        ("version", json!("0.1.0"), 0, ""),
        ("version", json!("foreign"), 2, "runtime manager identity mismatch"),
        ("revision", json!("foreign"), 2, "runtime manager identity mismatch"),
        ("protocol", json!(2), 2, "runtime manager identity mismatch"),
        ("binary", Value::Null, 2, "missing manager binary path"),
        ("executable_sha256", Value::Null, 2, "missing manager executable SHA"),
        (
            "binary",
            json!(fixture.project.path().join("CMakeLists.txt")),
            2,
            "SHA-256 mismatch",
        ),
    ] {
        let mut manager = entry.clone();
        manager[field] = value;
        let mut runtime = snapshot.clone();
        runtime["tools"].as_array_mut().expect("tools").push(manager);
        let bytes = serde_json::to_vec(&runtime).expect("runtime JSON");
        fs::write(&snapshot_path, &bytes).expect("runtime snapshot");
        let mut context = original.clone();
        context["runtime_snapshot_digest"] = json!(hash(&bytes));
        fs::write(&input, serde_json::to_vec(&context).expect("context JSON")).expect("context input");
        let result = fixture.run(&["--run-context", input.to_str().expect("context path"), "check"]);
        assert_eq!(
            result.status.code(),
            Some(expected_code),
            "{field}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(String::from_utf8_lossy(&result.stderr).contains(expected));
    }
}

#[test]
fn test_main_prewarm_builds_without_running_tasks_and_validates_cached_source() {
    let fixture = Fixture::new();
    fixture.init();
    let managed = fixture.project.path().join(".infra/managed-files.json");
    let before = fs::read(&managed).expect("managed snapshot");
    let result = fixture.run(&["prewarm", "--tool", "cpp-infra-style"]);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let output: Value = serde_json::from_slice(&result.stdout).expect("prewarm output");
    assert_eq!(output["mode"], "main");
    let snapshot: Value = serde_json::from_slice(
        &fs::read(output["runtime_snapshot"].as_str().expect("snapshot path")).expect("snapshot bytes"),
    )
    .expect("JSON");
    let style = snapshot["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .find(|tool| tool["name"] == "cpp-infra-style")
        .expect("style");
    assert!(Path::new(style["binary"].as_str().expect("style binary")).is_file());
    assert_eq!(fs::read(&managed).expect("unchanged managed snapshot"), before);
    let offline = fixture.run(&["--offline", "prewarm", "--tool", "cpp-infra-style"]);
    assert!(offline.status.success(), "{}", String::from_utf8_lossy(&offline.stderr));
    let output: Value = serde_json::from_slice(&offline.stdout).expect("offline prewarm output");
    assert_eq!(
        (output["offline"].clone(), output["stale"].clone()),
        (json!(true), json!(true))
    );
    let source = fixture
        .cache
        .path()
        .join("main/sources/cpp-infra-style")
        .join(style["revision"].as_str().expect("revision"));
    fs::write(source.join("untracked-change"), "local source edit").expect("cache source edit");
    let rejected = fixture.run(&["--offline", "prewarm", "--tool", "cpp-infra-style"]);
    assert_eq!(rejected.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("cached source checkout has local modifications"));
}

#[test]
fn test_main_doctor_uses_real_versions_and_reports_configuration_boundaries() {
    let fixture = Fixture::new();
    fixture.init();
    let defaults = fixture.project.path().join(".infra/tools/defaults.toml");
    fs::write(
        &defaults,
        "schema=1\n[external_tools.cmake]\nprogram='cmake'\nversion='>=3'\nargs=['--version']\ntasks=['verify']\n",
    )
    .expect("doctor constraints");
    let result = fixture.run(&["doctor", "--task", "verify", "--profile", "debug"]);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let output: Value = serde_json::from_slice(&result.stdout).expect("doctor output");
    assert_eq!(output["checked"], 1);
    let unmatched = fixture.run(&["doctor", "--task", "pages"]);
    assert_eq!(unmatched.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&unmatched.stderr).contains("no external tool constraints"));
    for (configuration, expected) in [
        ("schema=2\nexternal_tools={}\n", "unsupported defaults schema"),
        ("schema=1\nexternal_tools=[]\n", "defaults.toml:"),
        (
            "schema=1\n[external_tools.probe]\nprogram='echo'\nargs=['not-a-version']\nversion='>=1'\n",
            "cannot parse version of echo",
        ),
        (
            "schema=1\n[external_tools.probe]\nprogram='echo'\nargs=['1.2']\nversion='>=1'\n",
            "",
        ),
        (
            "schema=1\n[external_tools.probe]\nprogram='echo'\nargs=['1.2.3']\nversion='invalid'\n",
            "unexpected character",
        ),
    ] {
        fs::write(&defaults, configuration).expect("doctor input");
        let result = fixture.run(&["doctor"]);
        if expected.is_empty() {
            assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
        } else {
            assert_eq!(result.status.code(), Some(2));
            assert!(
                String::from_utf8_lossy(&result.stderr).contains(expected),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
    }
}

#[test]
fn test_locked_cold_cache_authenticates_historical_catalog_and_retires_real_policy() {
    let fixture = Fixture::new();
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let descriptor_path = "assets/cpp-project/manifest.json";
    let descriptor: Value =
        serde_json::from_slice(&fs::read(crate_root.join(descriptor_path)).expect("asset descriptor")).expect("JSON");
    let mut entries = vec![(
        descriptor_path.to_owned(),
        fs::read(crate_root.join(descriptor_path)).expect("descriptor bytes"),
    )];
    for key in ["files", "templates"] {
        for entry in descriptor[key].as_array().expect("descriptor entries") {
            let source = entry["source"].as_str().expect("asset source");
            entries.push((
                source.to_owned(),
                fs::read(crate_root.join(source)).expect("asset bytes"),
            ));
        }
    }
    let repository = fixture.repository("tools");
    let old = Command::new("git")
        .arg("-C")
        .arg(&repository)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("old manager ref");
    assert!(old.status.success());
    let mut stream = Vec::new();
    for (index, (_, bytes)) in entries.iter().enumerate() {
        write!(&mut stream, "blob\nmark :{}\ndata {}\n", index + 1, bytes.len()).expect("asset blob");
        stream.extend(bytes);
        stream.push(b'\n');
    }
    write!(&mut stream, "commit refs/heads/main\ncommitter Fixture <fixture@example.test> 1700000001 +0000\ndata 12\nexact assets\nfrom {}\n", String::from_utf8_lossy(&old.stdout).trim()).expect("asset commit");
    for (index, (path, _)) in entries.iter().enumerate() {
        writeln!(&mut stream, "M 100644 :{} {path}", index + 1).expect("asset tree");
    }
    stream.push(b'\n');
    let mut git = Command::new("git")
        .arg("-C")
        .arg(&repository)
        .args(["fast-import", "--quiet"])
        .stdin(Stdio::piped())
        .spawn()
        .expect("asset import");
    git.stdin
        .take()
        .expect("asset import stdin")
        .write_all(&stream)
        .expect("asset stream");
    assert!(git.wait().expect("asset import completion").success());
    let revision = Command::new("git")
        .arg("-C")
        .arg(&repository)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("manager ref");
    let revision = String::from_utf8(revision.stdout)
        .expect("revision UTF-8")
        .trim()
        .to_owned();
    let distribution = TempDir::new().expect("historical distribution");
    let raw_asset = distribution.path().join("unused-binary");
    fs::write(&raw_asset, "metadata-only asset").expect("asset");
    let asset_hash = hash(&fs::read(&raw_asset).expect("asset bytes"));
    let host = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
    let style_revision = Command::new("git")
        .arg("-C")
        .arg(fixture.repository("style"))
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("style ref");
    let style_revision = String::from_utf8(style_revision.stdout)
        .expect("style revision UTF-8")
        .trim()
        .to_owned();
    let tools = ["tools", "style"].map(|role| json!({"name":format!("cpp-infra-{role}"),"version":"0.1.0","revision":if role=="tools"{&revision}else{&style_revision},"protocol":1,"assets":{host.clone():{"url":format!("file://{}",raw_asset.display()),"sha256":asset_hash,"executable_sha256":asset_hash,"binary":format!("cpp-infra-{role}"),"format":"raw"}}}));
    let mut catalogs = Vec::new();
    for label in ["old", "new"] {
        let contents = format!("schema=1\n# {label}\n");
        let manifest = json!({"schema":1,"owner":"cpp-infra-style","files":[{"source":"policy.toml","target":format!("policy/{label}.toml"),"sha256":hash(contents.as_bytes()),"executable":false}]});
        let package = distribution.path().join(format!("{label}.tar.gz"));
        let gzip = flate2::write::GzEncoder::new(
            fs::File::create(&package).expect("package"),
            flate2::Compression::default(),
        );
        let mut archive = tar::Builder::new(gzip);
        for (name, bytes) in [
            ("manifest.json", serde_json::to_vec(&manifest).expect("manifest JSON")),
            ("policy.toml", contents.into_bytes()),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            archive
                .append_data(&mut header, name, bytes.as_slice())
                .expect("package entry");
        }
        archive.into_inner().expect("tar finish").finish().expect("gzip finish");
        let catalog = json!({"schema":1,"bundle_id":label,"protocol_major":1,"tools":tools,"packages":[{"owner":"cpp-infra-style","url":format!("file://{}",package.display()),"sha256":hash(&fs::read(package).expect("package bytes"))}],"external_tools":{}});
        let path = distribution.path().join(format!("{label}.json"));
        fs::write(&path, serde_json::to_vec(&catalog).expect("catalog JSON")).expect("catalog");
        catalogs.push(path);
    }
    let init = fixture
        .command(&["--locked", "init"])
        .env("CPP_INFRA_CATALOG_URL", format!("file://{}", catalogs[0].display()))
        .output()
        .expect("locked init");
    assert!(init.status.success(), "{}", String::from_utf8_lossy(&init.stderr));
    let policy = fixture.project.path().join(".infra/style/policy/old.toml");
    assert!(policy.is_file());
    let config = fixture.project.path().join(".infra/project.toml");
    let original_config = fs::read(&config).expect("project config");
    let cold_cache = TempDir::new().expect("cold user cache");
    let lock = fixture.project.path().join(".infra/tools/tools.lock.json");
    let original_lock = fs::read(&lock).expect("original lock");
    for (url, expected_code, expected) in [
        (
            "https://github.com/qubit-ltd/cpp-infra-tools/releases/download/../../../../attacker/evil/releases/download/catalog.json",
            2,
            "old catalog is outside",
        ),
        (
            "https://github.com/qubit-ltd/cpp-infra-tools/releases/download/%2e%2e/%2e%2e/%2e%2e/%2e%2e/attacker/evil/releases/download/catalog.json",
            2,
            "old catalog is outside",
        ),
        (
            "https://github.com/qubit-ltd/cpp-infra-tools/releases/download/v1/catalog.json",
            3,
            "offline: unavailable release metadata",
        ),
    ] {
        let mut changed: Value = serde_json::from_slice(&original_lock).expect("original lock JSON");
        changed["catalog_url"] = json!(url);
        let bytes = serde_json::to_vec(&changed).expect("aliased lock JSON");
        fs::write(&lock, &bytes).expect("aliased URL in old lock");
        let rejected = fixture
            .command(&["--locked", "--offline", "update"])
            .env_remove("CPP_INFRA_REPOSITORY_BASE")
            .env("CPP_INFRA_CACHE_DIR", cold_cache.path())
            .env("CPP_INFRA_CATALOG_URL", format!("file://{}", catalogs[1].display()))
            .output()
            .expect("reject escaped release origin");
        assert_eq!(
            rejected.status.code(),
            Some(expected_code),
            "normalized URL must remain in the trusted release path: {}",
            String::from_utf8_lossy(&rejected.stderr)
        );
        assert!(String::from_utf8_lossy(&rejected.stderr).contains(expected));
        assert_eq!(fs::read(&lock).expect("preserved rejected lock"), bytes);
        assert!(policy.is_file());
    }
    fs::write(&lock, original_lock).expect("restore canonical lock");
    #[cfg(unix)]
    {
        let outside = TempDir::new().expect("outside trusted catalog directory");
        let external = outside.path().join("old.json");
        fs::rename(&catalogs[0], &external).expect("move fixture catalog outside trust root");
        std::os::unix::fs::symlink(&external, &catalogs[0]).expect("catalog symlink outside trusted directory");
        let rejected = fixture
            .command(&["--locked", "update"])
            .env("CPP_INFRA_CACHE_DIR", cold_cache.path())
            .env("CPP_INFRA_CATALOG_URL", format!("file://{}", catalogs[1].display()))
            .output()
            .expect("reject canonical file trust escape");
        assert_eq!(
            rejected.status.code(),
            Some(2),
            "trusted catalog directory uses canonical targets: {}",
            String::from_utf8_lossy(&rejected.stderr)
        );
        assert!(String::from_utf8_lossy(&rejected.stderr).contains("old catalog is outside"));
        assert!(policy.is_file());
        fs::remove_file(&catalogs[0]).expect("discard temporary symlink");
        fs::rename(external, &catalogs[0]).expect("restore fixture catalog");
    }
    let update = fixture
        .command(&["--locked", "update"])
        .env("CPP_INFRA_CACHE_DIR", cold_cache.path())
        .env("CPP_INFRA_CATALOG_URL", format!("file://{}", catalogs[1].display()))
        .output()
        .expect("cold locked update");
    assert!(update.status.success(), "{}", String::from_utf8_lossy(&update.stderr));
    assert!(
        !policy.exists(),
        "retirement is authorized by reconstructed historical manifests"
    );
    assert!(fixture.project.path().join(".infra/style/policy/new.toml").is_file());
    assert_eq!(fs::read(config).expect("preserved project config"), original_config);
    assert!(fixture.run(&["--locked", "check"]).status.success());
}

#[test]
fn test_ci_only_filter_prepares_doctor_quality_and_matrix_tool_dependencies() {
    let fixture = Fixture::new();
    fixture.init();
    fs::write(
        fixture.project.path().join(".infra/ci/config.toml"),
        "schema=1\ntasks=['style','analyze','coverage','pages']\n",
    )
    .expect("CI task selection");
    for role in ["analyze", "coverage"] {
        fs::write(
            fixture.project.path().join(format!(".infra/{role}/config.toml")),
            format!("schema=1\nprofile='{role}'\n"),
        )
        .expect("tool profile selection");
    }
    let project = fixture.project.path().join(".infra/project.toml");
    let config = fs::read_to_string(&project).expect("project config");
    fs::write(&project, format!("{config}\n[profiles.analyze]\nconfigure='infra-debug'\nbuild='infra-debug'\nconan_context='analysis-deps'\n[profiles.coverage]\nconfigure='infra-debug'\nbuild='infra-debug'\n")).expect("analysis and coverage profiles");
    let report = fixture.project.path().join("build/selected-ci.json");
    let result = fixture.run(&[
        "--report",
        report.to_str().expect("report path"),
        "run",
        "--tool",
        "cpp-infra-ci",
        "--",
        "check",
        "--only=analyze,coverage",
    ]);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let report: Value = serde_json::from_slice(&fs::read(report).expect("report")).expect("JSON");
    let snapshot: Value = serde_json::from_slice(
        &fs::read(report["data"]["runtime_snapshot"].as_str().expect("snapshot path")).expect("snapshot"),
    )
    .expect("JSON");
    let names = snapshot["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .map(|tool| tool["name"].as_str().expect("tool name"))
        .collect::<Vec<_>>();
    for name in [
        "cpp-infra-ci",
        "cpp-infra-analyze",
        "cpp-infra-coverage",
        "cpp-infra-verify",
        "cpp-infra-dependency",
    ] {
        assert!(names.contains(&name), "missing selected dependency {name}");
    }
    for name in ["cpp-infra-tools", "cpp-infra-style", "cpp-infra-pages"] {
        assert!(
            names.contains(&name),
            "CI expands quality and doctor prerequisites beyond --only"
        );
    }
    assert_eq!(names.len(), 8);
}

#[test]
fn test_project_internal_cache_uses_independent_external_transaction_receipts() {
    let fixture = Fixture::new();
    let user_cache = TempDir::new().expect("external user cache");
    let internal = fixture.project.path().join("build/shared-cache");
    let result = fixture
        .command(&["init"])
        .env("CPP_INFRA_CACHE_DIR", &internal)
        .env("XDG_CACHE_HOME", user_cache.path())
        .output()
        .expect("init with internal build cache");
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert!(!internal.join("transaction-receipts").exists());
    let receipts = user_cache.path().join("qubit/cpp-infra/transaction-receipts");
    assert!(receipts.is_dir());
    assert!(fs::read_dir(receipts).expect("receipts").any(|entry| {
        entry
            .expect("receipt entry")
            .file_name()
            .to_string_lossy()
            .starts_with("project-")
    }));
    assert!(fixture.run(&["check"]).status.success());
}

#[test]
fn test_report_records_a_real_project_git_revision() {
    let fixture = Fixture::new();
    fixture.init();
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(fixture.project.path())
            .args(["init", "--quiet", "--initial-branch=main"])
            .status()
            .expect("project Git init")
            .success()
    );
    let mut child = Command::new("git")
        .arg("-C")
        .arg(fixture.project.path())
        .args(["fast-import", "--quiet"])
        .stdin(Stdio::piped())
        .spawn()
        .expect("project Git commit");
    child
        .stdin
        .take()
        .expect("Git stream")
        .write_all(
            b"commit refs/heads/main\ncommitter Fixture <fixture@example.test> 1700000000 +0000\ndata 7\nproject\n\n",
        )
        .expect("project commit stream");
    assert!(child.wait().expect("Git commit completion").success());
    let expected = Command::new("git")
        .arg("-C")
        .arg(fixture.project.path())
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("project revision");
    let report = fixture.project.path().join("build/git-report.json");
    let checked = fixture.run(&["--report", report.to_str().expect("report path"), "check"]);
    assert!(checked.status.success(), "{}", String::from_utf8_lossy(&checked.stderr));
    let report: Value = serde_json::from_slice(&fs::read(report).expect("report")).expect("JSON");
    assert_eq!(
        report["source_revision"],
        String::from_utf8_lossy(&expected.stdout).trim()
    );
}

#[test]
fn test_offline_default_repository_and_xdg_cache_fail_without_network_or_snapshot() {
    let project = TempDir::new().expect("project");
    let user_cache = TempDir::new().expect("user cache");
    let result = Command::new(env!("CARGO_BIN_EXE_cpp-infra-tools"))
        .arg("--project")
        .arg(project.path())
        .args([
            "--offline",
            "resolve",
            "--tool",
            "cpp-infra-style",
            "--output",
            "build/revisions.json",
        ])
        .env_remove("CPP_INFRA_CACHE_DIR")
        .env_remove("CPP_INFRA_REPOSITORY_BASE")
        .env("XDG_CACHE_HOME", user_cache.path())
        .output()
        .expect("offline first use");
    assert_eq!(result.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&result.stderr).contains("no previously resolved main snapshot"));
    assert!(user_cache.path().join("qubit/cpp-infra").is_dir());
    assert!(!project.path().join("build/revisions.json").exists());
    assert!(!project.path().join(".infra").exists());
}

#[test]
fn test_python_bootstrap_fetches_and_builds_manager_main_for_local_check() {
    let fixture = Fixture::new();
    fixture.init();
    let bootstrap = fixture.project.path().join(".infra/bootstrap.py");
    let result = Command::new("python3")
        .arg(&bootstrap)
        .arg("--check")
        .env("CPP_INFRA_CACHE_DIR", fixture.cache.path())
        .env("CPP_INFRA_REPOSITORY_BASE", fixture.sources.path())
        .output()
        .expect("Python bootstrap");
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert!(String::from_utf8_lossy(&result.stdout).contains("manager-initial"));
    let result = Command::new("python3")
        .arg(bootstrap)
        .arg("--check")
        .env("CPP_INFRA_CACHE_DIR", fixture.cache.path())
        .env("CPP_INFRA_REPOSITORY_BASE", "/unavailable/source")
        .output()
        .expect("cached local check");
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
}

#[test]
fn test_ci_prepares_full_family_including_manager_for_doctor_nodes() {
    let fixture = Fixture::new();
    fixture.init();
    fs::write(
        fixture.project.path().join(".infra/ci/config.toml"),
        "schema=1\ntasks=[\"style\"]\nprimary_profile=\"debug\"\n",
    )
    .expect("CI tasks");
    let report = fixture.project.path().join("build/ci-manager.json");
    let result = fixture.run(&[
        "--report",
        report.to_str().expect("path"),
        "run",
        "--tool",
        "cpp-infra-ci",
        "--",
        "check",
    ]);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let report: Value = serde_json::from_slice(&fs::read(report).expect("report")).expect("JSON");
    let snapshot: Value = serde_json::from_slice(
        &fs::read(Path::new(
            report["data"]["runtime_snapshot"].as_str().expect("snapshot"),
        ))
        .expect("snapshot"),
    )
    .expect("JSON");
    let names: Vec<_> = snapshot["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .map(|t| t["name"].as_str().expect("name"))
        .collect();
    assert_eq!(names.len(), 8, "CI must prepare all supported roles for its own DAG");
    assert!(
        names.contains(&"cpp-infra-tools"),
        "doctor tasks require the manager binary"
    );
}

#[test]
fn test_dependency_is_added_only_for_selected_conan_profile() {
    let fixture = Fixture::new();
    fixture.init();
    let report = fixture.project.path().join("build/verify-manager.json");
    let args = [
        "--report",
        report.to_str().expect("path"),
        "run",
        "--tool",
        "cpp-infra-verify",
        "--",
        "prepare",
        "--profile",
        "debug",
    ];
    let result = fixture.run(&args);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let value: Value = serde_json::from_slice(&fs::read(&report).expect("report")).expect("JSON");
    let snapshot: Value = serde_json::from_slice(
        &fs::read(Path::new(value["data"]["runtime_snapshot"].as_str().expect("snapshot"))).expect("snapshot"),
    )
    .expect("JSON");
    assert_eq!(snapshot["tools"].as_array().expect("tools").len(), 1);
    let project = fixture.project.path().join(".infra/project.toml");
    let text = fs::read_to_string(&project).expect("project config");
    fs::write(project, format!("{text}conan_context = \"fixture-deps\"\n")).expect("selected Conan context");
    let result = fixture.run(&args);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let value: Value = serde_json::from_slice(&fs::read(report).expect("report")).expect("JSON");
    let snapshot: Value = serde_json::from_slice(
        &fs::read(Path::new(value["data"]["runtime_snapshot"].as_str().expect("snapshot"))).expect("snapshot"),
    )
    .expect("JSON");
    assert_eq!(snapshot["tools"].as_array().expect("tools").len(), 2);
}

#[test]
fn test_existing_manager_report_in_project_source_does_not_change_child_identity() {
    let fixture = Fixture::new();
    fixture.init();
    let report = fixture.project.path().join("manager.json");
    assert!(
        fixture
            .run(&["--report", report.to_str().expect("path"), "check"])
            .status
            .success()
    );
    let result = fixture.run(&[
        "--report",
        report.to_str().expect("path"),
        "run",
        "--tool",
        "cpp-infra-style",
    ]);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let report: Value = serde_json::from_slice(&fs::read(report).expect("manager report")).expect("JSON");
    assert_eq!(report["status"], "passed");
    let root = Path::new(report["report_root"].as_str().expect("root"));
    assert_eq!(report["source_excluded_paths"], json!(["manager.json"]));
    let child = report["artifacts"]
        .as_array()
        .expect("artifacts")
        .iter()
        .find(|a| a["kind"] == "tool-report")
        .expect("child artifact");
    assert!(root.join(child["path"].as_str().expect("path")).is_file());
}

#[test]
fn test_parent_and_explicit_child_reports_share_validated_exclusions() {
    let fixture = Fixture::new();
    fixture.init();
    let parent = fixture.project.path().join("manager.json");
    let child = fixture.project.path().join("style.json");
    for _ in 0..2 {
        let result = fixture.run(&[
            "--report",
            parent.to_str().expect("parent"),
            "run",
            "--tool",
            "cpp-infra-style",
            "--",
            "--report",
            child.to_str().expect("child"),
            "check",
        ]);
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
        let parent: Value = serde_json::from_slice(&fs::read(&parent).expect("parent report")).expect("JSON");
        let child: Value = serde_json::from_slice(&fs::read(&child).expect("child report")).expect("JSON");
        assert_eq!(parent["source_excluded_paths"], json!(["manager.json", "style.json"]));
        assert_eq!(parent["source_digest"], child["source_digest"]);
        assert_eq!(parent["config_digest"], child["config_digest"]);
    }
}

#[test]
fn test_report_output_cannot_overwrite_business_json_or_protected_source() {
    let fixture = Fixture::new();
    fixture.init();
    for name in ["business.json", "src/report.json", "CMakePresets.json"] {
        let path = fixture.project.path().join(name);
        let original = if path.exists() {
            fs::read(&path).expect("existing input")
        } else {
            b"{\"business\":true}".to_vec()
        };
        fs::write(&path, &original).expect("business input");
        let result = fixture.run(&["--report", path.to_str().expect("path"), "check"]);
        assert_eq!(result.status.code(), Some(2));
        assert_eq!(fs::read(path).expect("preserved input"), original);
    }
}

#[test]
fn test_nested_build_sources_are_hashed_and_root_build_outputs_are_excluded() {
    let fixture = Fixture::new();
    fixture.init();
    let nested = fixture.project.path().join("src/build");
    fs::create_dir_all(&nested).expect("nested build source");
    fs::write(nested.join("source.cpp"), "int value=1;\n").expect("source");
    let report = fixture.project.path().join("build/digest.json");
    let read = || {
        let result = fixture.run(&["--report", report.to_str().expect("path"), "check"]);
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
        serde_json::from_slice::<Value>(&fs::read(&report).expect("report")).expect("JSON")
    };
    let first = read();
    fs::write(fixture.project.path().join("build/cache.txt"), "temporary").expect("build output");
    assert_eq!(read()["source_digest"], first["source_digest"]);
    fs::write(nested.join("source.cpp"), "int value=2;\n").expect("changed source");
    assert_ne!(read()["source_digest"], first["source_digest"]);
}

#[cfg(unix)]
#[test]
fn test_transient_cargo_download_failure_is_retried_without_changing_revision() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    fixture.init();
    let bin = fixture.sources.path().join("fake-bin");
    fs::create_dir(&bin).expect("fake bin");
    let cargo = bin.join("cargo");
    let script = format!(
        "#!/usr/bin/env python3\nimport os,sys\nfrom pathlib import Path\np=Path(os.environ['CPP_INFRA_TEST_CARGO_COUNTER'])\nif not p.exists():\n p.write_text('first')\n print('connection reset while downloading dependency',file=sys.stderr)\n sys.exit(101)\nos.execv({:?}, [{:?}]+sys.argv[1:])\n",
        env!("CARGO"),
        env!("CARGO")
    );
    fs::write(&cargo, script).expect("cargo wrapper");
    fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755)).expect("executable");
    let mut paths = vec![bin];
    paths.extend(std::env::split_paths(&std::env::var_os("PATH").expect("PATH")));
    let result = fixture
        .command(&["run", "--tool", "cpp-infra-style"])
        .env("PATH", std::env::join_paths(&paths).expect("PATH join"))
        .env(
            "CPP_INFRA_TEST_CARGO_COUNTER",
            fixture.sources.path().join("cargo-attempt"),
        )
        .output()
        .expect("retried run");
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert!(String::from_utf8_lossy(&result.stdout).contains("executed-initial"));
    let result = Command::new("python3")
        .arg(fixture.project.path().join(".infra/bootstrap.py"))
        .arg("--check")
        .env("CPP_INFRA_CACHE_DIR", fixture.cache.path())
        .env("CPP_INFRA_REPOSITORY_BASE", fixture.sources.path())
        .env("PATH", std::env::join_paths(&paths).expect("PATH join"))
        .env(
            "CPP_INFRA_TEST_CARGO_COUNTER",
            fixture.sources.path().join("manager-cargo-attempt"),
        )
        .output()
        .expect("retried manager bootstrap");
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
}

#[test]
fn test_main_cache_override_may_be_relative_to_invoking_directory() {
    let fixture = Fixture::new();
    fixture.init();
    let result = fixture
        .command(&["run", "--tool", "cpp-infra-style"])
        .current_dir(fixture.cache.path())
        .env("CPP_INFRA_CACHE_DIR", "relative-cache")
        .output()
        .expect("relative cache run");
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert!(
        fixture
            .cache
            .path()
            .join("relative-cache/main/tools/cpp-infra-style")
            .is_dir()
    );
}

#[test]
fn test_report_exclusion_rejects_malformed_family_shaped_business_json() {
    let fixture = Fixture::new();
    fixture.init();
    let path = fixture.project.path().join("business.json");
    let bytes=br#"{"schema":1,"tool":{"name":"cpp-infra-style"},"status":"passed","bundle_id":null,"run_id":null,"task_id":null,"project":null,"report_root":null,"source_revision":null,"source_digest":null,"config_digest":null,"started_at":null,"duration_ms":null,"external_versions":null,"commands":null,"diagnostics":null,"artifacts":null}"#;
    fs::write(&path, bytes).expect("malformed family-shaped business data");
    let result = fixture.run(&["--report", path.to_str().expect("path"), "check"]);
    assert_eq!(result.status.code(), Some(2));
    assert_eq!(fs::read(path).expect("preserved business data"), bytes);
}

#[test]
fn test_resolve_cannot_overwrite_inputs_or_unknown_json() {
    let fixture = Fixture::new();
    fixture.init();
    for name in [
        "CMakeLists.txt",
        ".infra/project.toml",
        "src/refs.json",
        "business.json",
    ] {
        let path = fixture.project.path().join(name);
        let bytes = if path.is_file() {
            fs::read(&path).expect("input")
        } else {
            b"{\"business\":true}".to_vec()
        };
        fs::write(&path, &bytes).expect("input fixture");
        let result = fixture.run(&["resolve", "--output", name]);
        assert_eq!(result.status.code(), Some(2), "{name}");
        assert_eq!(fs::read(path).expect("preserved input"), bytes);
    }
}

#[test]
fn test_equals_profile_selects_same_conan_tool_closure() {
    let fixture = Fixture::new();
    fixture.init();
    let path = fixture.project.path().join(".infra/project.toml");
    let text = fs::read_to_string(&path).expect("project");
    fs::write(
        path,
        format!(
            "{text}\n[profiles.release]\nconfigure='infra-debug'\nbuild='infra-debug'\nconan_context='release-deps'\n"
        ),
    )
    .expect("release profile");
    let report = fixture.project.path().join("build/equals.json");
    let result = fixture.run(&[
        "--report",
        report.to_str().expect("report"),
        "run",
        "--tool",
        "cpp-infra-verify",
        "--",
        "prepare",
        "--profile=release",
    ]);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let report: Value = serde_json::from_slice(&fs::read(report).expect("report")).expect("JSON");
    let snapshot: Value = serde_json::from_slice(
        &fs::read(report["data"]["runtime_snapshot"].as_str().expect("snapshot path")).expect("snapshot"),
    )
    .expect("JSON");
    assert!(
        snapshot["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .any(|tool| tool["name"] == "cpp-infra-dependency")
    );
}

#[test]
fn test_analyze_explicit_profile_overrides_config_for_conan_tool_preparation() {
    let fixture = Fixture::new();
    fixture.init();
    fs::write(
        fixture.project.path().join(".infra/analyze/config.toml"),
        "schema=1\nprofile='analysis'\n",
    )
    .expect("configured analysis profile");
    let project = fixture.project.path().join(".infra/project.toml");
    let text = fs::read_to_string(&project).expect("project config");
    fs::write(&project, format!("{text}\n[profiles.analysis]\nconfigure='infra-debug'\nbuild='infra-debug'\n[profiles.custom]\nconfigure='infra-debug'\nbuild='infra-debug'\nconan_context='custom-deps'\n")).expect("custom Conan profile");
    let report = fixture.project.path().join("build/analyze-profile.json");
    for (arguments, needs_dependency) in [
        (vec!["check"], false),
        (vec!["check", "--profile", "custom"], true),
        (vec!["check", "--profile=custom"], true),
    ] {
        let mut args = vec![
            "--report",
            report.to_str().expect("report path"),
            "run",
            "--tool",
            "cpp-infra-analyze",
            "--",
        ];
        args.extend(arguments);
        let result = fixture.run(&args);
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
        let report: Value = serde_json::from_slice(&fs::read(&report).expect("report")).expect("JSON");
        let context: Value = serde_json::from_slice(
            &fs::read(report["data"]["run_context"].as_str().expect("run context")).expect("context bytes"),
        )
        .expect("context JSON");
        assert_eq!(context["profile"], if needs_dependency { "custom" } else { "analysis" });
        let snapshot: Value = serde_json::from_slice(
            &fs::read(report["data"]["runtime_snapshot"].as_str().expect("snapshot path")).expect("snapshot bytes"),
        )
        .expect("JSON");
        let names = snapshot["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .map(|tool| tool["name"].as_str().expect("tool name"))
            .collect::<Vec<_>>();
        assert!(names.contains(&"cpp-infra-analyze"));
        assert!(names.contains(&"cpp-infra-verify"));
        assert_eq!(
            names.contains(&"cpp-infra-dependency"),
            needs_dependency,
            "explicit profile must determine the Conan dependency"
        );
    }
}

#[test]
fn test_partial_binary_without_marker_is_rebuilt() {
    let fixture = Fixture::new();
    fixture.init();
    let report = fixture.project.path().join("build/cache.json");
    let args = [
        "--report",
        report.to_str().expect("report"),
        "run",
        "--tool",
        "cpp-infra-style",
    ];
    assert!(fixture.run(&args).status.success());
    let value: Value = serde_json::from_slice(&fs::read(&report).expect("report")).expect("JSON");
    let snapshot: Value = serde_json::from_slice(
        &fs::read(value["data"]["runtime_snapshot"].as_str().expect("snapshot")).expect("snapshot file"),
    )
    .expect("JSON");
    let binary = Path::new(snapshot["tools"][0]["binary"].as_str().expect("binary"));
    let marker = binary.parent().expect("cache dir").join("build.json");
    fs::remove_file(&marker).expect("simulate interrupted marker publication");
    let result = fixture.run(&args);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert!(marker.is_file());
}

#[test]
fn test_failed_main_child_report_is_archived() {
    let fixture = Fixture::new();
    fixture.publish("style", "failure-protocol");
    fixture.init();
    let report = fixture.project.path().join("build/failure.json");
    let result = fixture
        .command(&[
            "--report",
            report.to_str().expect("report"),
            "run",
            "--tool",
            "cpp-infra-style",
        ])
        .env("CPP_INFRA_FIXTURE_EXIT", "1")
        .output()
        .expect("run");
    assert_eq!(result.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&fs::read(report).expect("report")).expect("JSON");
    assert!(
        report["artifacts"]
            .as_array()
            .expect("artifacts")
            .iter()
            .any(|artifact| artifact["kind"] == "tool-report")
    );
}

#[test]
fn test_exclusion_raw_spelling_must_be_canonical() {
    let fixture = Fixture::new();
    fixture.init();
    let report = fixture.project.path().join("build/context-source.json");
    assert!(
        fixture
            .run(&[
                "--report",
                report.to_str().expect("report"),
                "run",
                "--tool",
                "cpp-infra-style"
            ])
            .status
            .success()
    );
    let parent: Value = serde_json::from_slice(&fs::read(report).expect("identity")).expect("JSON");
    let identity: Value = serde_json::from_slice(
        &fs::read(parent["data"]["run_context"].as_str().expect("context")).expect("context file"),
    )
    .expect("context JSON");
    for raw in [
        "business.json/",
        "folder//report.json",
        "./business.json",
        "folder/../business.json",
    ] {
        let mut context = identity.clone();
        context["profile"] = Value::Null;
        context["source_excluded_paths"] = json!([raw]);
        let path = fixture.project.path().join("build/exclusion-context.json");
        fs::write(&path, serde_json::to_vec(&context).expect("context")).expect("context file");
        let result = fixture.run(&["--run-context", path.to_str().expect("path"), "check"]);
        assert_eq!(result.status.code(), Some(2), "accepted {raw}");
    }
}

#[cfg(unix)]
#[test]
fn test_rustc_fingerprint_is_queried_from_tool_source() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    fixture.init();
    let actual = Command::new("rustup")
        .args(["which", "rustc"])
        .output()
        .expect("real rustc");
    assert!(actual.status.success());
    let actual = String::from_utf8(actual.stdout).expect("rustc path");
    let bin = fixture.sources.path().join("cwd-bin");
    fs::create_dir(&bin).expect("bin");
    let wrapper = bin.join("rustc");
    let script = format!(
        "#!/usr/bin/env python3\nimport os,sys\nfrom pathlib import Path\nif Path.cwd()==Path({:?}):\n print('wrong project toolchain cwd',file=sys.stderr)\n sys.exit(3)\nos.execv({:?},[{:?}]+sys.argv[1:])\n",
        fixture.project.path().to_string_lossy(),
        actual.trim(),
        actual.trim()
    );
    fs::write(&wrapper, script).expect("wrapper");
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).expect("mode");
    let mut paths = vec![bin];
    paths.extend(std::env::split_paths(&std::env::var_os("PATH").expect("PATH")));
    let result = fixture
        .command(&["run", "--tool", "cpp-infra-style"])
        .env("PATH", std::env::join_paths(paths).expect("PATH join"))
        .output()
        .expect("source cwd build");
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
}

#[test]
fn test_explicit_style_fix_accepts_only_scoped_mutations_and_reports_before_after() {
    for (command, target, expected) in [
        ("fix", "src/main.cpp", 0),
        ("check", "src/main.cpp", 2),
        ("fix", ".infra/style/config.toml", 2),
    ] {
        let fixture = Fixture::new();
        fixture.init();
        fs::write(
            fixture.project.path().join(".infra/style/config.toml"),
            "schema=1\nextensions=[\"cpp\"]\ninclude=[\"src/**\"]\nexclude=[]\n",
        )
        .expect("style scope");
        fs::write(fixture.project.path().join("src/main.cpp"), "int value=0;\n").expect("source");
        let report = fixture.project.path().join("build/mutation.json");
        let result = fixture
            .command(&[
                "--report",
                report.to_str().expect("report"),
                "run",
                "--tool",
                "cpp-infra-style",
                "--",
                command,
            ])
            .env("CPP_INFRA_FIXTURE_MUTATE", target)
            .output()
            .expect("mutating run");
        assert_eq!(
            result.status.code(),
            Some(expected),
            "{command}/{target}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        if expected == 0 {
            let value: Value = serde_json::from_slice(&fs::read(report).expect("report")).expect("JSON");
            assert_ne!(
                value["data"]["inputs_before"]["source_digest"],
                value["data"]["inputs_after"]["source_digest"]
            );
            assert_eq!(value["source_digest"], value["data"]["inputs_after"]["source_digest"]);
        }
    }
}

#[test]
fn test_cold_cache_update_reconstructs_old_authority_before_retirement() {
    let fixture = Fixture::new();
    fixture.publish("style", "preexisting-main-history");
    fixture.init();
    let old = fixture.project.path().join(".infra/style/policy/default.toml");
    assert!(old.is_file());
    for step in 0..4 {
        fixture.publish("style", &format!("main-history-{step}"));
    }
    fixture.publish("style", "retire-old-policy");
    fs::remove_dir_all(fixture.cache.path()).expect("clear disposable tool cache including local receipt");
    fs::create_dir_all(fixture.cache.path()).expect("empty cache");
    let result = fixture.run(&["update"]);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert!(!old.exists());
    assert!(
        fixture
            .project
            .path()
            .join(".infra/style/policy/renamed.toml")
            .is_file()
    );
    assert!(fixture.run(&["check"]).status.success());
}

#[test]
fn test_cold_cache_does_not_trust_forged_old_policy_record() {
    let fixture = Fixture::new();
    fixture.init();
    let target = ".infra/style/policy/user-private.toml";
    let victim = fixture.project.path().join(target);
    fs::write(&victim, "private contents").expect("victim");
    let manifest_path = fixture.project.path().join(".infra/managed-files.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&manifest_path).expect("manifest")).expect("JSON");
    manifest["files"].as_array_mut().expect("files").push(json!({"target":target,"source":"cpp-infra-style:policy/private.toml","sha256":hash(b"private contents"),"executable":false}));
    fs::write(&manifest_path, serde_json::to_vec(&manifest).expect("forgery")).expect("manifest file");
    fs::remove_dir_all(fixture.cache.path()).expect("clear disposable cache");
    fs::create_dir_all(fixture.cache.path()).expect("empty cache");
    let result = fixture.run(&["update"]);
    assert_eq!(result.status.code(), Some(2));
    assert_eq!(
        fs::read_to_string(victim).expect("preserved user data"),
        "private contents"
    );
}

#[test]
fn test_static_delivery_manifest_matches_repository_bytes() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let definition: Value =
        serde_json::from_slice(&fs::read(root.join("assets/cpp-project/manifest.json")).expect("asset definition"))
            .expect("JSON");
    for key in ["files", "templates"] {
        for entry in definition[key].as_array().expect("asset entries") {
            let source = entry["source"].as_str().expect("source");
            assert_eq!(
                entry["sha256"],
                hash(&fs::read(root.join(source)).expect("declared asset")),
                "{source}"
            );
        }
    }
}

#[test]
fn test_cache_git_commands_ignore_callers_git_dir() {
    let fixture = Fixture::new();
    let foreign = fixture.repository("style").join(".git");
    let before = fs::read(foreign.join("config")).expect("foreign config");
    let result = fixture
        .command(&["run", "--tool", "cpp-infra-style"])
        .env("GIT_DIR", &foreign)
        .output()
        .expect("manager with caller Git env");
    assert_eq!(
        fs::read(foreign.join("config")).expect("foreign config preserved"),
        before,
        "source-cache setup changed caller Git configuration"
    );
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
}

#[test]
fn test_dependency_mutation_scope_and_dry_run() {
    for (command, target, expected) in [
        (vec!["sync"], "conanfile.txt", 0),
        (
            vec!["lock", "update", "--context=release"],
            ".infra/dependency/locks/release.lock",
            0,
        ),
        (vec!["sync", "--dry-run"], "conanfile.txt", 2),
        (vec!["sync"], ".infra/dependency/policy/default.toml", 2),
    ] {
        let fixture = Fixture::new();
        fixture.init();
        fs::write(fixture.project.path().join("conanfile.txt"), "[requires]\n").expect("recipe");
        fs::write(
            fixture.project.path().join(".infra/dependency/config.toml"),
            "schema=1\nrecipe='conanfile.txt'\n[contexts.release]\nprofile='debug'\nlockfile='.infra/dependency/locks/release.lock'\n",
        )
        .expect("dependency scope");
        let report = fixture.project.path().join("build/dep-mutate.json");
        let mut args = vec![
            "--report",
            report.to_str().expect("report"),
            "run",
            "--tool",
            "cpp-infra-dependency",
            "--",
        ];
        args.extend(command);
        let result = fixture
            .command(&args)
            .env("CPP_INFRA_FIXTURE_MUTATE", target)
            .output()
            .expect("dependency mutation");
        assert_eq!(
            result.status.code(),
            Some(expected),
            "{target}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        if expected == 0 {
            let value: Value = serde_json::from_slice(&fs::read(report).expect("report")).expect("JSON");
            assert_ne!(
                value["data"]["inputs_before"]["config_digest"],
                value["data"]["inputs_after"]["config_digest"]
            );
        }
    }
}

#[test]
fn test_cold_cache_rejects_off_main_ownership_revision() {
    let fixture = Fixture::new();
    fixture.init();
    let repository = fixture.repository("style");
    let read_head = || {
        String::from_utf8(
            Command::new("git")
                .arg("-C")
                .arg(&repository)
                .args(["rev-parse", "refs/heads/main"])
                .output()
                .expect("head")
                .stdout,
        )
        .expect("SHA")
        .trim()
        .to_owned()
    };
    let original = read_head();
    fixture.publish("style", "retire-unpublished-policy");
    let unpublished = read_head();
    for args in [
        vec!["update-ref", "refs/heads/feature", unpublished.as_str()],
        vec!["update-ref", "refs/heads/main", original.as_str()],
    ] {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&repository)
                .args(args)
                .status()
                .expect("fixture branch")
                .success()
        );
    }
    let snapshot_path = fixture.project.path().join(".infra/tools/main-snapshot.json");
    let mut snapshot: Value = serde_json::from_slice(&fs::read(&snapshot_path).expect("snapshot")).expect("JSON");
    for tool in snapshot["revisions"]["tools"].as_array_mut().expect("refs") {
        if tool["name"] == "cpp-infra-style" {
            tool["revision"] = json!(&unpublished);
        }
    }
    for tool in snapshot["tools"].as_array_mut().expect("tools") {
        if tool["name"] == "cpp-infra-style" {
            tool["revision"] = json!(&unpublished);
        }
    }
    let mut pairs: Vec<(String, String)> = snapshot["revisions"]["tools"]
        .as_array()
        .expect("refs")
        .iter()
        .map(|v| {
            (
                v["name"].as_str().expect("name").to_owned(),
                v["revision"].as_str().expect("revision").to_owned(),
            )
        })
        .collect();
    pairs.sort();
    snapshot["bundle_id"] = json!(format!(
        "main-{}",
        hash(&serde_json::to_vec_pretty(&pairs).expect("pairs"))
    ));
    fs::write(
        snapshot_path,
        serde_json::to_vec_pretty(&snapshot).expect("snapshot bytes"),
    )
    .expect("snapshot file");
    let target = ".infra/style/policy/renamed.toml";
    let contents = b"schema = 1\n# retire-unpublished-policy\n";
    let victim = fixture.project.path().join(target);
    fs::write(&victim, contents).expect("unowned important user file");
    fs::remove_file(fixture.project.path().join(".infra/style/policy/default.toml"))
        .expect("model spoofed old snapshot fixture");
    let manifest_path = fixture.project.path().join(".infra/managed-files.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&manifest_path).expect("manifest")).expect("JSON");
    for entry in manifest["files"].as_array_mut().expect("files") {
        if entry["target"] == ".infra/style/policy/default.toml" {
            entry["target"] = json!(target);
            entry["sha256"] = json!(hash(contents));
            entry["source"] = json!(format!("cpp-infra-style@{unpublished}:policy.toml"));
        }
    }
    manifest["files"]
        .as_array_mut()
        .expect("files")
        .sort_by_key(|e| e["target"].as_str().expect("target").to_owned());
    fs::write(
        manifest_path,
        serde_json::to_vec_pretty(&manifest).expect("manifest bytes"),
    )
    .expect("manifest file");
    fs::remove_dir_all(fixture.cache.path()).expect("clear cache");
    fs::create_dir_all(fixture.cache.path()).expect("empty cache");
    let result = fixture.run(&["update"]);
    assert_eq!(
        result.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(fs::read(victim).expect("preserved user file"), contents);
}

#[test]
fn test_explicit_help_and_version_are_not_quality_tasks() {
    let fixture = Fixture::new();
    fixture.init();
    for args in [vec!["check", "--help"], vec!["--version", "--json"]] {
        let report = fixture.project.path().join("build/introspection.json");
        let mut command = vec![
            "--report",
            report.to_str().expect("report"),
            "run",
            "--tool",
            "cpp-infra-style",
            "--",
        ];
        command.extend(args);
        let result = fixture.run(&command);
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
        let report: Value = serde_json::from_slice(&fs::read(report).expect("report")).expect("JSON");
        assert_eq!(report["data"]["operation"], "introspection");
        assert!(
            !report["artifacts"]
                .as_array()
                .expect("artifacts")
                .iter()
                .any(|a| a["kind"] == "tool-report")
        );
    }
    assert_eq!(
        fixture
            .run(&["run", "--tool", "cpp-infra-style", "--", "check", "--help", "--unknown"])
            .status
            .code(),
        Some(2)
    );
}
