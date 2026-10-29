# cpp-infra-tools

[![CI](https://github.com/qubit-ltd/cpp-infra-tools/actions/workflows/ci.yml/badge.svg)](https://github.com/qubit-ltd/cpp-infra-tools/actions/workflows/ci.yml) [![Crate](https://img.shields.io/badge/crate-qubit--cpp--infra--tools-blue)](Cargo.toml) [![Rust](https://img.shields.io/badge/rust-1.94%2B-orange)](rust-toolchain.toml) [![License](https://img.shields.io/badge/license-Apache--2.0-blue)](LICENSE) [![API](https://img.shields.io/badge/API-source-blue)](src/lib.rs) [![中文](https://img.shields.io/badge/文档-简体中文-blue)](README.zh_CN.md)

C++ teams can run the current infrastructure tools without copying every tool's scripts into every project. This Rust manager follows the eight `cpp-infra-*` repositories' `main` branches, fixes the resolved Git revisions for each invocation, builds them in a shared cache, and hands the selected tool a verifiable execution context. Project policy files change only through explicit `init` or `update` commands.

## Installation

Build the manager from this checkout with Rust 1.94 or newer:

```sh
cargo install --path . --locked
cpp-infra-tools --version --json
```

The version response identifies protocol 1. A source build without `CPP_INFRA_BUILD_REVISION` reports `revision: null`; main-cache builds embed the exact Git SHA. Git, Cargo, and Python 3.10+ are required by the main bootstrap. CMake, compilers, Conan, and analysis tools remain environment-provided dependencies. A C++ project can commit the generated root `update-infra.sh`; its first run needs Git and Cargo, while later runs use Python 3.10+.

## Quick start

Consider a CMake library whose developers need the latest shared style and CI rules while retaining its own CMake files and configuration. From its project directory:

```sh
./update-infra.sh
./update-infra.sh --check
.infra/bin/style-check.sh
```

When `.infra/bootstrap.py` is absent, the root script fetches `cpp-infra-tools` from GitHub `main` into a temporary directory and runs `init` with that checkout's Cargo toolchain. Initialization resolves all eight repositories' published `main` commits and installs the bootstrap, owned policies, and missing initial configuration. Existing project configuration is preserved. If a repository has no published `main` commit, initialization fails clearly; it does not manufacture a release or silently use a different branch. Edit the seeded project/tool configuration to describe the actual CMake project before running its checks. Once initialized, the same root script uses the installed bootstrap to update managed files and follow current `main` revisions. `--dry-run`, `--status`, and `--check` require an initialized project; a first run with options fails without creating `.infra`.

`--check` validates the local managed snapshot without consulting a release channel. The style wrapper resolves the selected tool's latest `main`, builds that immutable revision once, and executes it. A normal run writes only caches and `build/` outputs. Obtain newer public policies explicitly:

```sh
./update-infra.sh --dry-run
./update-infra.sh
cpp-infra-tools --project . --report build/style-manager.json run --tool cpp-infra-style -- check
```

A locally edited managed policy is a conflict, with its path reported; update preserves project-owned configuration and does not stage or commit Git changes. The report names the executed revisions, command logs, source/config digests, and child report.

`.infra/bin/align-ci.sh` executes `style fix`; pass `--dry-run` to preview formatting. `.infra/bin/ci-check.sh` runs the CI gates. Explicit style fixes and dependency `sync` / `lock update` record before/after digests, and the manager permits only the command’s selected input paths to change. Read-only tasks retain their original input identity.

## One revision group across CI jobs

Resolve once in a planning job, transfer this portable JSON file as an artifact, then set its absolute path in subsequent jobs:

```sh
cpp-infra-tools --project . resolve --output build/ci/revisions.json
export CPP_INFRA_REVISIONS_FILE="$PWD/build/ci/revisions.json"
python3 .infra/bootstrap.py run --tool cpp-infra-ci -- check
```

The portable file contains repository URLs and Git revisions, never another runner's executable paths. Each runner builds for its own Rust host and records executable SHA-256 values in `build/infra-runs/<run-id>/tools/<call-id>/runtime-snapshot.json`. A bundle ID depends only on the sorted tool name/revision group. `latest` is an alias in behavior for the default `run` mode.

CI execution prepares all eight tools, including the manager, for doctor tasks, Pages quality prerequisites, and matrix profiles. CI applies `--only` when scheduling its tasks. Direct analyze and verify calls use an explicit `--profile NAME` or `--profile=NAME` when preparing the profile's Conan dependency.

## Cache, offline use, and diagnostics

```sh
cpp-infra-tools --project . prewarm --tool cpp-infra-style
cpp-infra-tools --project . --offline run --tool cpp-infra-style -- check
cpp-infra-tools --project . doctor --task verify --profile debug
cpp-infra-tools --project . status
```

Offline main execution explicitly reuses the latest locally resolved group for the requested tool closure and marks its report `offline: true, stale: true`. Missing source, dependency, or build caches fail; offline does not imply a reproducible external compiler environment. Doctor queries real tool versions against `.infra/tools/defaults.toml`; it does not install system packages.

`CPP_INFRA_CACHE_DIR` overrides the user cache (`XDG_CACHE_HOME`/`~/.cache/qubit/cpp-infra` on Unix, `LOCALAPPDATA/qubit/cpp-infra` on Windows). `CPP_INFRA_REPOSITORY_BASE` overrides the repository base for a mirror or local Git fixture. `CPP_INFRA_BIN_DIR` is supplied to children with only the invocation's prepared tool binaries; there is no PATH fallback for missing bundle tools.

## Explicit locked bundles

The advanced `--locked` mode retains stable-catalog distribution, verified raw/tar.gz/zip assets, safe extraction, version/revision/protocol validation, and explicit `--from-source` fallback. It requires a real catalog and project `tools.lock.json`:

```sh
CPP_INFRA_CATALOG_URL=file:///absolute/catalog.json cpp-infra-tools --project . --locked init
cpp-infra-tools --project . --locked prewarm --tool cpp-infra-style
cpp-infra-tools --project . --locked --offline run --tool cpp-infra-style -- check
```

`CPP_INFRA_CHANNEL_URL` can identify a stable channel instead. These modes are not prerequisites for default main execution. This source checkout does not publish or claim a stable bundle. Source fallback requires a catalog-pinned source archive and Cargo.lock digest; it never builds branch HEAD implicitly.

Reports outside `build/` may name only validated, non-input JSON files. Existing business JSON and protected source/configuration files are never overwritten. The validated `source_excluded_paths` file list is propagated to children so parent and child source digests remain identical. Logs, contexts, and artifacts use a unique `tools/<call-id>/` directory under the public report root.

## Protocol and boundaries

The authoritative [JSON schemas](schemas/) cover reports, contexts, ownership manifests, catalogs, portable revisions, and runtime snapshots. Configuration uses `schema = 1`; the manager strictly validates project configuration, path ownership, root overlap, and tool configuration schema discriminators. Each task tool owns validation of its complete task configuration.

Updates use a project lock, an immutable journal, and independent 0600 trust receipts outside the project. Read-only entrypoints reject unfinished transactions; an explicit update validates every current/backup/new digest before recovery. Committed snapshot receipts prove old ownership; a cold cache reconstructs it from immutable manifests and bootstrap assets in the configured trusted Git repositories, proving that each old main revision belongs to the current trusted main history. Unknown records and locally changed files are preserved. Shared cache downloads/builds are serialized. Child processes have timeouts and cancellation propagation. Exit codes are 0 success, 1 gate/build failure, 2 configuration/protocol error, 3 I/O/network/spawn failure, and 130 cancellation.

Known `--help` and `--version` requests execute as read-only introspection and do not claim a successful task report. Normal task execution requires a fresh child report matching the selected tool, invocation, inputs, and required artifact digests.

`update --dry-run` refuses unfinished transactions and preserves their files and trust receipts. Run an explicit update to validate and recover them before requesting another preview.

The suite exercises real local Git fetches, Cargo builds, CMake version queries, HTTP downloads, cache contention, and update recovery. Task executables in unit fixtures are small protocol implementations; full eight-tool C++ integration is a separate acceptance step. Linux is exercised here; this checkout does not claim macOS or Windows execution has been validated. SHA-256 detects content changes and is not an independent release signature.

## Testing

```sh
cargo fmt --check
CARGO_BUILD_JOBS=2 cargo test --locked
CARGO_BUILD_JOBS=2 cargo clippy --all-targets -- -D warnings
```

Only this crate and its own fixtures are tested. The Rust crate's `.infra` is distinct from the [C++ delivery templates](assets/cpp-project/).

## License

Licensed under [Apache-2.0](LICENSE).

## Contributing

Keep tests focused on observable CLI behavior and protocol boundaries. Supply a failing regression before changing behavior, and keep both README versions aligned.

## Author

Haixing Hu <starfish.hu@gmail.com>, maintained in the [Qubit project](https://github.com/qubit-ltd/cpp-infra-tools).
