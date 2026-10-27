#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Build the manager at fixed main revisions, then delegate to its checked CLI."""
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import platform
import re
import subprocess
import sys
import tarfile
import tempfile
import urllib.request
import zipfile


def checked(data, expected):
    if hashlib.sha256(data).hexdigest() != expected:
        raise ValueError("SHA-256 mismatch")
    return data


def valid_identity(identity, expected):
    return isinstance(identity, dict) and isinstance(expected, dict) and all(type(identity.get(key)) is str and identity.get(key) == expected.get(key) for key in ("name", "version", "revision")) and type(identity.get("protocol")) is int and type(expected.get("protocol")) is int and identity["protocol"] == expected["protocol"]


def acquire_lock(handle):
    if os.name == "nt":
        import msvcrt
        handle.seek(0)
        if not handle.read(1):
            handle.write(b"0")
            handle.flush()
        handle.seek(0)
        msvcrt.locking(handle.fileno(), msvcrt.LK_LOCK, 1)
    else:
        import fcntl
        fcntl.flock(handle, fcntl.LOCK_EX)


def locked_main():
    project = Path(__file__).resolve().parent.parent
    lock = json.loads((project / ".infra/tools/tools.lock.json").read_text())
    if lock.get("schema") != 1 or lock.get("protocol_major") != 1:
        raise ValueError("unsupported snapshot protocol")
    tool = next(t for t in lock["tools"] if t["name"] == "cpp-infra-tools")
    host = {"Linux": "linux", "Darwin": "macos", "Windows": "windows"}[platform.system()] + "-" + {"AMD64": "x86_64", "aarch64": "aarch64", "arm64": "aarch64", "x86_64": "x86_64"}[platform.machine()]
    asset = tool["assets"].get(host)
    if asset is None:
        raise ValueError("no platform asset for bootstrap")
    if Path(asset["binary"]).name != asset["binary"] or "\\" in asset["binary"]:
        raise ValueError("unsafe binary name")
    base = os.environ.get("CPP_INFRA_CACHE_DIR")
    if not base:
        base = str(Path(os.environ["LOCALAPPDATA"]) / "qubit/cpp-infra") if os.name == "nt" else str(Path(os.environ.get("XDG_CACHE_HOME", str(Path.home() / ".cache"))) / "qubit/cpp-infra")
    directory = Path(base).resolve() / tool["name"] / tool["revision"] / host / asset["sha256"]
    directory.mkdir(parents=True, exist_ok=True)
    binary = directory / asset["binary"]
    with (directory / ".lock").open("a+b") as handle:
        acquire_lock(handle)
        if not binary.is_file():
            if "--offline" in sys.argv:
                raise OSError("offline: bootstrap cache missing")
            source = asset["url"]
            if not (source.startswith("https://") or source.startswith("file://") or source.startswith("http://127.0.0.1:")):
                raise ValueError("insecure asset URL")
            with urllib.request.urlopen(source, timeout=60) as response:
                data = checked(response.read(), asset["sha256"])
            if asset.get("format", "tar.gz") != "raw":
                entries = []
                if asset.get("format", "tar.gz") == "tar.gz":
                    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as archive:
                        for item in archive.getmembers():
                            path = PurePosixPath(item.name)
                            if path.is_absolute() or ".." in path.parts or "\\" in item.name or ":" in item.name or not (item.isdir() or item.isfile()):
                                raise ValueError("unsafe archive entry")
                            if item.isfile() and path.name == asset["binary"]:
                                entries.append(archive.extractfile(item).read())
                elif asset["format"] == "zip":
                    with zipfile.ZipFile(io.BytesIO(data)) as archive:
                        for item in archive.infolist():
                            path = PurePosixPath(item.filename)
                            kind = (item.external_attr >> 16) & 0o170000
                            if path.is_absolute() or ".." in path.parts or "\\" in item.filename or ":" in item.filename or kind not in (0, 0o100000, 0o040000):
                                raise ValueError("unsafe archive entry")
                            if not item.is_dir() and path.name == asset["binary"]:
                                entries.append(archive.read(item))
                else:
                    raise ValueError("unsupported archive format")
                if len(entries) != 1:
                    raise ValueError("archive must contain one bootstrap executable")
                data = entries[0]
            checked(data, asset["executable_sha256"])
            with tempfile.NamedTemporaryFile(dir=directory, delete=False) as stage:
                stage.write(data)
                stage.flush()
                os.fsync(stage.fileno())
                staged = Path(stage.name)
            staged.chmod(0o755)
            version = json.loads(subprocess.check_output([str(staged), "--version", "--json"], timeout=15))
            if not valid_identity(version, tool):
                staged.unlink()
                raise ValueError("bootstrap identity mismatch")
            staged.replace(binary)
        checked(binary.read_bytes(), asset["executable_sha256"])
        identity = json.loads(subprocess.check_output([str(binary), "--version", "--json"], timeout=15))
        if not valid_identity(identity, tool):
            raise ValueError("cached bootstrap identity mismatch")
    args = sys.argv[1:]
    if not args:
        args = ["update"]
    elif args[0] == "--check":
        args = ["check"] + args[1:]
    elif args[0] == "--status":
        args = ["status"] + args[1:]
    elif args[0] in ("--dry-run", "--bundle"):
        args = ["update"] + args
    os.execv(str(binary), [str(binary), "--project", str(project)] + args)


class BuildFailure(RuntimeError):
    """The pinned manager source failed to compile."""


def run_bounded(arguments, cwd=None, environment=None, timeout=90):
    child = subprocess.Popen(arguments, cwd=cwd, env=environment, stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=os.name != "nt")
    try:
        stdout, stderr = child.communicate(timeout=timeout)
    except (KeyboardInterrupt, subprocess.TimeoutExpired):
        if os.name == "nt":
            subprocess.run(["taskkill", "/PID", str(child.pid), "/T", "/F"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        else:
            import signal
            os.killpg(child.pid, signal.SIGKILL)
        child.kill()
        child.wait()
        raise
    return subprocess.CompletedProcess(arguments, child.returncode, stdout, stderr)


def cargo_build(arguments, cwd, environment):
    import time
    environment = dict(environment)
    for key in ("GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_COMMON_DIR", "GIT_OBJECT_DIRECTORY", "GIT_ALTERNATE_OBJECT_DIRECTORIES"):
        environment.pop(key, None)
    for attempt in range(4):
        result = run_bounded(arguments, cwd=cwd, environment=environment, timeout=1800)
        sys.stderr.buffer.write(result.stdout + result.stderr)
        if result.returncode == 0:
            return
        error = result.stderr.decode(errors="replace").lower()
        transient = not any(text in error for text in ("404", "not found", "checksum", "sha-256")) and any(text in error for text in ("timed out", "connection reset", "could not resolve host", "connection refused", "temporary failure", "http 502", "http 503"))
        if not transient:
            raise BuildFailure("pinned manager Cargo build failed")
        if attempt == 3:
            raise OSError("dependency download failed after four attempts")
        time.sleep(2 ** (attempt + 1))


def git(args, cwd=None):
    import time
    for attempt in range(4):
        environment = dict(os.environ, GIT_TERMINAL_PROMPT="0")
        for key in ("GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_COMMON_DIR", "GIT_OBJECT_DIRECTORY", "GIT_ALTERNATE_OBJECT_DIRECTORIES"):
            environment.pop(key, None)
        result = run_bounded(["git"] + args, cwd=cwd, timeout=90, environment=environment)
        if result.returncode == 0:
            return result.stdout.decode().strip()
        error = result.stderr.decode(errors="replace")
        if attempt == 3 or not any(text in error.lower() for text in ("timed out", "could not resolve host", "connection reset", "http 502", "http 503")):
            raise OSError("git " + args[0] + ": " + error)
        time.sleep(2 ** (attempt + 1))


def package_version(manifest):
    """Read the literal package.version supported by the Rust manager (Python 3.10)."""
    text = Path(manifest).read_text()
    package = re.search(r"(?ms)^\s*\[package\]\s*(?:#[^\n]*)?$\n(.*?)(?=^\s*\[|\Z)", text)
    if package is None:
        raise ValueError("manager Cargo.toml has no package section")
    version = re.search(r"(?m)^\s*version\s*=\s*([\"'])([0-9A-Za-z.+-]+)\1\s*(?:#.*)?$", package.group(1))
    if version is None:
        raise ValueError("manager Cargo.toml needs a literal package.version")
    return version.group(2)


def main():
    if "--locked" in sys.argv[1:]:
        return locked_main()
    project = Path(__file__).resolve().parent.parent
    args = sys.argv[1:]
    if not args:
        args = ["update"]
    elif args[0] == "--check":
        args = ["check"] + args[1:]
    elif args[0] == "--status":
        args = ["status"] + args[1:]
    elif args[0] in ("--dry-run", "--bundle"):
        args = ["update"] + args
    base = os.environ.get("CPP_INFRA_CACHE_DIR")
    if not base:
        base = str(Path(os.environ["LOCALAPPDATA"]) / "qubit/cpp-infra") if os.name == "nt" else str(Path(os.environ.get("XDG_CACHE_HOME", str(Path.home() / ".cache"))) / "qubit/cpp-infra")
    cache = Path(base).resolve() / "main"
    cache.mkdir(parents=True, exist_ok=True)
    repository = os.environ.get("CPP_INFRA_REPOSITORY_BASE", "https://github.com/qubit-ltd").rstrip("/") + "/cpp-infra-tools.git"
    latest = cache / ("bootstrap-latest-" + hashlib.sha256(repository.encode()).hexdigest() + ".json")
    offline = "--offline" in args
    if os.environ.get("CPP_INFRA_REVISIONS_FILE"):
        refs = json.loads(Path(os.environ["CPP_INFRA_REVISIONS_FILE"]).read_text())
        if refs.get("schema") != 1:
            raise ValueError("unsupported portable revisions schema")
        manager = next(tool for tool in refs["tools"] if tool["name"] == "cpp-infra-tools")
        repository, revision = manager["repository"], manager["revision"]
    elif args and args[0] == "check":
        manager = json.loads((project / ".infra/bootstrap-source.json").read_text())
        repository, revision = manager["repository"], manager["revision"]
    elif offline:
        if not latest.is_file():
            raise OSError("offline: no cached manager main resolution")
        manager = json.loads(latest.read_text())
        revision = manager["revision"]
    else:
        output = git(["ls-remote", repository, "refs/heads/main"])
        if not output:
            raise ValueError("cpp-infra-tools main has no published revision")
        revision = output.split()[0]
        temporary = latest.with_suffix(".tmp-" + str(os.getpid()))
        temporary.write_text(json.dumps({"schema": 1, "repository": repository, "revision": revision}))
        temporary.replace(latest)
    if len(revision) != 40 or any(c not in "0123456789abcdefABCDEF" for c in revision):
        raise ValueError("invalid manager Git revision")
    source = cache / "sources/cpp-infra-tools" / revision
    lock_dir = cache / "locks"
    lock_dir.mkdir(parents=True, exist_ok=True)
    with (lock_dir / ("source-cpp-infra-tools-" + revision + ".lock")).open("a+b") as handle:
        acquire_lock(handle)
        source.mkdir(parents=True, exist_ok=True)
        valid = (source / ".git").is_dir() and git(["rev-parse", "HEAD"], str(source)) == revision
        if not valid:
            if offline:
                raise OSError("offline: manager source cache missing")
            if not (source / ".git").is_dir():
                git(["init", "--quiet"], str(source))
                git(["remote", "add", "origin", repository], str(source))
            git(["fetch", "--depth=1", "origin", revision], str(source))
            git(["checkout", "--quiet", "--detach", revision], str(source))
        if git(["status", "--porcelain", "--untracked-files=all"], str(source)):
            raise ValueError("cached manager checkout has local changes")
    rustc = subprocess.check_output(["rustc", "--version", "--verbose"], cwd=source, timeout=15).decode()
    host = next(line[6:] for line in rustc.splitlines() if line.startswith("host: "))
    directory = cache / "tools/cpp-infra-tools" / revision / host / hashlib.sha256(rustc.encode()).hexdigest()
    directory.mkdir(parents=True, exist_ok=True)
    binary = directory / ("cpp-infra-tools.exe" if os.name == "nt" else "cpp-infra-tools")
    marker = directory / "build.json"
    with (directory / ".lock").open("a+b") as handle:
        acquire_lock(handle)
        cargo_lock = hashlib.sha256((source / "Cargo.lock").read_bytes()).hexdigest()
        ready = False
        if binary.is_file() and marker.is_file():
            try:
                state = json.loads(marker.read_text())
                ready = isinstance(state, dict) and state.get("schema") == 1 and state.get("revision") == revision and state.get("rustc") == rustc and state.get("host") == host and state.get("cargo_lock_sha256") == cargo_lock and state.get("executable_sha256") == hashlib.sha256(binary.read_bytes()).hexdigest()
            except (ValueError, KeyError):
                ready = False
        if not ready:
            environment = dict(os.environ, CPP_INFRA_BUILD_REVISION=revision, CARGO_BUILD_JOBS="2")
            if offline:
                environment["CARGO_NET_OFFLINE"] = "true"
            cargo_build(["cargo", "build", "--release", "--locked", "--bin", "cpp-infra-tools", "--target-dir", str(directory / "target")], cwd=source, environment=environment)
            built = directory / "target/release" / binary.name
            data = built.read_bytes()
            with tempfile.NamedTemporaryFile(dir=directory, delete=False) as stage:
                stage.write(data)
                staged = Path(stage.name)
            staged.chmod(0o755)
            staged.replace(binary)
            marker.write_text(json.dumps({"schema":1,"revision":revision,"host":host,"rustc":rustc,"cargo_lock_sha256":cargo_lock,"executable_sha256":hashlib.sha256(data).hexdigest()}))
        identity = json.loads(subprocess.check_output([str(binary), "--version", "--json"], timeout=15))
        expected = dict(name="cpp-infra-tools", version=package_version(source / "Cargo.toml"), revision=revision, protocol=1)
        if not valid_identity(identity, expected):
            raise ValueError("manager main executable identity mismatch")
    os.execv(str(binary), [str(binary), "--project", str(project)] + args)


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        sys.exit(130)
    except (ValueError, KeyError, StopIteration) as error:
        print("bootstrap configuration: " + str(error), file=sys.stderr)
        sys.exit(2)
    except BuildFailure as error:
        print("bootstrap build: " + str(error), file=sys.stderr)
        sys.exit(1)
    except (OSError, subprocess.SubprocessError) as error:
        print("bootstrap infrastructure: " + str(error), file=sys.stderr)
        sys.exit(3)
