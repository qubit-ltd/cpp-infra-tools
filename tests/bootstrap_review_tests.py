# SPDX-License-Identifier: Apache-2.0
"""Isolate bootstrap identity/cache boundaries; Git validity alone is mocked."""
import ast
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

BOOTSTRAP = Path(__file__).resolve().parents[1] / "assets/cpp-project/bootstrap.py"

class ReachedExec(Exception):
    pass

class BootstrapReviewTests(unittest.TestCase):
    def fixture(self, identity=None):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name)
        project = root / "project"
        (project / ".infra").mkdir(parents=True)
        path = project / ".infra/bootstrap.py"
        path.write_text(BOOTSTRAP.read_text())
        revision = "1" * 40
        cache = root / "cache"
        source = cache / "main/sources/cpp-infra-tools" / revision
        source.mkdir(parents=True)
        (source / ".git").mkdir()
        (source / "Cargo.lock").write_text("version = 4\n")
        (source / "Cargo.toml").write_text("[package]\nname='qubit-cpp-infra-tools'\nversion='0.1.0'\n")
        refs = root / "refs.json"
        refs.write_text(json.dumps(dict(schema=1, tools=[dict(name="cpp-infra-tools",repository="https://fixture.invalid/tools.git",revision=revision)])))
        rustc = subprocess.check_output(["rustc","--version","--verbose"],cwd=source).decode()
        host = next(line[6:] for line in rustc.splitlines() if line.startswith("host: "))
        sha = lambda value: hashlib.sha256(value).hexdigest()
        directory = cache / "main/tools/cpp-infra-tools" / revision / host / sha(rustc.encode())
        directory.mkdir(parents=True)
        binary = directory / "cpp-infra-tools"
        if identity is None:
            identity = dict(name="cpp-infra-tools",version="0.1.0",revision=revision,protocol=1)
        binary.write_text("#!/usr/bin/env python3\nimport json\nprint(json.dumps(" + repr(identity) + "))\n")
        binary.chmod(0o755)
        marker = directory / "build.json"
        marker.write_text(json.dumps(dict(schema=1,revision=revision,host=host,rustc=rustc,cargo_lock_sha256=sha((source / "Cargo.lock").read_bytes()),executable_sha256=sha(binary.read_bytes()))))
        ns = dict(__name__="fixture_bootstrap",__file__=str(path))
        exec(compile(path.read_text(),str(path),"exec"),ns)
        ns["git"] = lambda args,cwd=None: revision if args[0] == "rev-parse" else ""
        return ns,root,source,binary,marker,dict(CPP_INFRA_CACHE_DIR=str(cache),CPP_INFRA_REVISIONS_FILE=str(refs))

    def invoke(self, ns, environment):
        with mock.patch.dict(os.environ,environment),mock.patch.object(sys,"argv",[ns["__file__"],"--offline","check"]),mock.patch.object(os,"execv",side_effect=ReachedExec):
            ns["main"]()

    def test_manager_rejects_each_missing_or_wrong_identity_field(self):
        base = dict(name="cpp-infra-tools",version="0.1.0",revision="1"*40,protocol=1)
        for key,wrong in (("name","wrong"),("version","9.9.9"),("revision","2"*40),("protocol",2)):
            for missing in (False,True):
                with self.subTest(key=key,missing=missing):
                    identity = dict(base)
                    if missing: identity.pop(key)
                    else: identity[key] = wrong
                    ns,_,_,_,_,environment = self.fixture(identity)
                    with self.assertRaises(ValueError): self.invoke(ns,environment)

    def test_rustc_query_uses_source_working_directory(self):
        ns,_,source,_,_,environment = self.fixture()
        original = subprocess.check_output
        observed = []
        def capture(args,**kwargs):
            if args[0] == "rustc": observed.append(Path(kwargs.get("cwd",Path.cwd())))
            return original(args,**kwargs)
        with mock.patch.object(subprocess,"check_output",side_effect=capture):
            with self.assertRaises(ReachedExec): self.invoke(ns,environment)
        self.assertEqual(observed,[source])

    def test_partial_cache_without_marker_is_rebuilt(self):
        ns,_,_,binary,marker,environment = self.fixture()
        contents = binary.read_bytes()
        marker.unlink()
        called = []
        def build(arguments,cwd,environment):
            called.append(arguments)
            target = Path(arguments[arguments.index("--target-dir")+1]) / "release/cpp-infra-tools"
            target.parent.mkdir(parents=True,exist_ok=True)
            target.write_bytes(contents)
        ns["cargo_build"] = build
        with self.assertRaises(ReachedExec): self.invoke(ns,environment)
        self.assertEqual(len(called),1)
        self.assertTrue(marker.is_file())

    def test_bootstrap_parses_as_python_310(self):
        ast.parse(BOOTSTRAP.read_text(),feature_version=(3,10))

if __name__ == "__main__": unittest.main()
