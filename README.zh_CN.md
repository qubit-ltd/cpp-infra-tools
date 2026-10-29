# cpp-infra-tools

[![CI](https://github.com/qubit-ltd/cpp-infra-tools/actions/workflows/ci.yml/badge.svg)](https://github.com/qubit-ltd/cpp-infra-tools/actions/workflows/ci.yml) [![Crate](https://img.shields.io/badge/crate-qubit--cpp--infra--tools-blue)](Cargo.toml) [![Rust](https://img.shields.io/badge/rust-1.94%2B-orange)](rust-toolchain.toml) [![License](https://img.shields.io/badge/license-Apache--2.0-blue)](LICENSE) [![API](https://img.shields.io/badge/API-source-blue)](src/lib.rs) [![English](https://img.shields.io/badge/docs-English-blue)](README.md)

C++ 团队可以通过这个 Rust 管理工具使用最新基础设施，无须在每个业务仓库复制整套脚本。默认跟踪八个 `cpp-infra-*` 仓库的 `main`，每次运行先确定一组 Git SHA，再在共享缓存中构建并调用指定工具。项目里的公共策略只有执行 `init` 或 `update` 才会变化。

## 安装

使用 Rust 1.94 或更新版本，在本仓库安装：

```sh
cargo install --path . --locked
cpp-infra-tools --version --json
```

版本响应包含协议版本 1。未设置 `CPP_INFRA_BUILD_REVISION` 的源码构建返回 `revision: null`；main 缓存中的工具嵌入真实 Git SHA。main 引导流程依赖 Git、Cargo 和 Python 3.10+。CMake、编译器、Conan 和分析工具由运行环境提供。C++ 项目可以提交生成到根目录的 `update-infra.sh`：首次运行需要 Git 和 Cargo，后续运行需要 Python 3.10+。

## 快速开始

假设一个 CMake 类库希望采用最新公共风格和 CI 规则，同时保留自己的 CMake 文件与配置。在该项目中执行：

```sh
./update-infra.sh
./update-infra.sh --check
.infra/bin/style-check.sh
```

如果项目还没有 `.infra/bootstrap.py`，根目录脚本会把 GitHub `main` 上的 `cpp-infra-tools` 检出到临时目录，并使用该检出版本的 Cargo 工具链执行 `init`。初始化会解析八个仓库已发布到 `main` 的提交，安装引导入口、受管理策略，并补齐缺少的初始配置；已有项目配置会保留。任何仓库尚无 main 提交都会明确失败，不会捏造发布版本或切换到其他分支。初始配置需要根据实际 CMake 项目填写，然后再执行检查。完成初始化后，同一个根目录脚本会通过已安装的引导程序更新受管文件并跟进最新 `main`。首次运行附带参数时会直接报错，不会创建 `.infra`；`--dry-run`、`--status` 和 `--check` 仅适用于已初始化项目。

`--check` 验证本地受管快照，不查询发布渠道。风格入口运行时解析对应工具的最新 main，在缓存中构建确定的提交后执行。普通运行只写缓存和 `build/` 产物。需要更新项目公共策略时，显式执行：

```sh
./update-infra.sh --dry-run
./update-infra.sh
cpp-infra-tools --project . --report build/style-manager.json run --tool cpp-infra-style -- check
```

如果受管策略被本地修改，更新会报告冲突路径。更新器保留项目所有配置，也不会替用户暂存或提交 Git 变更。管理器报告记录实际工具版本、命令日志、源码与配置摘要，以及子工具报告。

`.infra/bin/align-ci.sh` 执行 `style fix`，传入 `--dry-run` 可预览格式化；`.infra/bin/ci-check.sh` 执行 CI 门禁。显式风格修正及 dependency 的 `sync`、`lock update` 会记录前后摘要，管理器只允许该命令选中的输入文件发生变化。只读任务仍严格保持初始输入身份。

## CI 各任务复用同一组版本

在规划任务中解析一次，将生成的 JSON 作为制品传给后续任务，再设置其绝对路径：

```sh
cpp-infra-tools --project . resolve --output build/ci/revisions.json
export CPP_INFRA_REVISIONS_FILE="$PWD/build/ci/revisions.json"
python3 .infra/bootstrap.py run --tool cpp-infra-ci -- check
```

便携版本清单只含仓库地址和 Git SHA，不记录其他 runner 的可执行文件路径。各 runner 为本机 Rust host 构建工具，并在 `build/infra-runs/<run-id>/tools/<call-id>/runtime-snapshot.json` 中记录可执行文件 SHA-256。bundle ID 只由排序后的工具名称与提交组决定。默认 `run` 与 `latest` 的行为相同。

CI 运行会准备包含管理器在内的全部八个工具，供 doctor、Pages 质量前置任务及 matrix 各 profile 使用；CI 调度任务时应用 `--only`。直接调用 analyze 或 verify 时，显式 `--profile NAME` 和 `--profile=NAME` 都会决定应准备哪个 profile 的 Conan 依赖。

## 缓存、离线与环境检查

```sh
cpp-infra-tools --project . prewarm --tool cpp-infra-style
cpp-infra-tools --project . --offline run --tool cpp-infra-style -- check
cpp-infra-tools --project . doctor --task verify --profile debug
cpp-infra-tools --project . status
```

离线运行会明确复用此前为同一工具依赖闭包解析的版本组，报告标记 `offline: true, stale: true`。缺少源码、依赖或构建缓存时失败；离线并不代表外部编译环境完全可复现。doctor 实际查询工具版本，并按 `.infra/tools/defaults.toml` 约束检查，不自动安装系统软件。

`CPP_INFRA_CACHE_DIR` 指定共享缓存。Unix 默认使用 `XDG_CACHE_HOME` 或 `~/.cache/qubit/cpp-infra`，Windows 使用 `LOCALAPPDATA/qubit/cpp-infra`。`CPP_INFRA_REPOSITORY_BASE` 可以指向仓库镜像或本地 Git fixture。管理器通过 `CPP_INFRA_BIN_DIR` 向子工具提供本次版本组的程序；缺少组内工具时不会从 PATH 混用其他版本。

## 显式锁定的工具集

高级 `--locked` 模式保留稳定 catalog 分发：校验 raw/tar.gz/zip 制品、安全解包、核对版本与 revision/protocol，并允许显式 `--from-source`。它需要真实 catalog 和项目的 `tools.lock.json`：

```sh
CPP_INFRA_CATALOG_URL=file:///absolute/catalog.json cpp-infra-tools --project . --locked init
cpp-infra-tools --project . --locked prewarm --tool cpp-infra-style
cpp-infra-tools --project . --locked --offline run --tool cpp-infra-style -- check
```

也可用 `CPP_INFRA_CHANNEL_URL` 指定稳定渠道。默认 main 模式不依赖这些发布元数据。本源码仓库不宣称已有稳定工具集发布。源码回退只允许 catalog 指定的源码包和 Cargo.lock 摘要，不会隐式构建分支 HEAD。

报告放在 `build/` 以外时，只能写入经过校验的非输入 JSON 文件；已有业务 JSON、源码和配置不会被覆盖。校验后的 `source_excluded_paths` 文件清单会传给子工具，保证父子工具使用相同源码摘要。日志、上下文和工件放在公共报告根目录下唯一的 `tools/<call-id>/` 子目录中。

## 协议与能力边界

权威 [JSON Schema](schemas/) 覆盖报告、运行上下文、所有权清单、catalog、便携版本组和运行快照。配置采用 `schema = 1`。管理器严格校验项目配置、路径归属、目录重叠及各工具配置的 schema 标识；每个任务工具负责自己的完整配置模式校验。

更新使用项目锁、不可变事务日志和项目外独立的 0600 信任凭据。只读入口遇到未完成事务会明确拒绝；显式 update 先整体验证当前文件、备份与预期新内容，再恢复。上次成功快照凭据证明旧文件归属；缓存清空后，通过可信 Git 仓库的固定旧提交清单与 bootstrap 资产重建归属，并证明旧 main 提交属于当前可信 main 的历史。未知记录及本地改动均保留。共享缓存的下载与构建持锁串行执行。子进程有超时，并传播取消信号。退出码分别是：0 成功、1 门禁或构建失败、2 配置或协议错误、3 文件/网络/进程基础设施故障、130 取消。

已知命令的 `--help` 和 `--version` 请求按只读查询执行，不宣称任务报告通过。普通任务必须生成全新的子报告，并匹配所选工具、本次调用、输入摘要与必需工件摘要。

`update --dry-run` 遇到未完成事务时会拒绝执行，保留事务文件和信任凭据。先执行显式 update 校验并恢复事务，再请求更新预览。

测试会真实执行本地 Git 获取、Cargo 构建、CMake 版本查询、HTTP 下载、缓存竞争和事务恢复；任务工具 fixture 使用小型协议程序。八个正式工具共同检查 C++ 项目的验收另行执行。当前已运行 Linux 测试，尚不宣称验证过 macOS 或 Windows。SHA-256 用于检测内容变化，不等同于独立发布签名。

## 测试

```sh
cargo fmt --check
CARGO_BUILD_JOBS=2 cargo test --locked
CARGO_BUILD_JOBS=2 cargo clippy --all-targets -- -D warnings
```

仅验证当前 crate 与自带 fixture。Rust 仓库自身的 `.infra` 与 [C++ 交付模板](assets/cpp-project/) 分开维护。

## 许可证

采用 [Apache-2.0](LICENSE) 许可证。

## 贡献

围绕可观察的 CLI 行为和协议边界编写测试；修改行为前先补能复现问题的失败测试。同步维护两种语言的 README。

## 作者

胡海星（Haixing Hu，starfish.hu@gmail.com），在 [Qubit 项目](https://github.com/qubit-ltd/cpp-infra-tools) 中维护。
