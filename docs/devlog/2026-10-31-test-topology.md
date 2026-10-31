# 测试拓扑优化与重复测试削减（test-topology）

- 日期：2026-10-31；任务 ID：test-topology；执行者：基层 Agent。
- 分支：`refactor/test-topology`；基础提交：`1721eec20805611089d0e611fa2799ed2d77c45e`（`feature/shell-pages`）；PR 待建，关联 Milestone #1。
- 目标与状态：实现审批的优化 4（测试拓扑与重复测试消除），消除无谓的重复共享测试编译/运行，保留全部实际业务行为与页面/夹具集成覆盖；**实现与本地全量验证完成**。

## 改动与决定

### 触发条件与拓扑映射分析
1. **基线现状**：
   - 在基线分支 `1721eec` 上，`Cargo.toml` 包含 1 个 binary (`githux`)、9 个 examples，以及 7 个 integration test targets (`account`, `home`, `profile`, `reader`, `reader_images`, `repository`, `repository_overview`)。
   - 基线全量 `cargo test` 触发 8 个测试可执行文件的编译与运行。
   - 统计基线各 target 的测试用例数：
     - `githux` (bin): 799 tests
     - `tests/account.rs`: 799 tests
     - `tests/home.rs`: 57 tests
     - `tests/profile.rs`: 197 tests
     - `tests/reader.rs`: 168 tests
     - `tests/reader_images.rs`: 485 tests
     - `tests/repository.rs`: 559 tests
     - `tests/repository_overview.rs`: 20 tests
     - **总执行次数**：3,084 次；**原始用例名数**：1,595；**经模块前缀归一化后的独立测试场景数**：1,102 个。
2. **重复发现**：
   - 比较 `githux` 与 `tests/account.rs`：两个 target 内部包含的 799 个测试完全一一对应（对称差为 0）。
   - 查阅历史提交（PR #28 `feat: add a real account preview example` vs PR #38 `feat: wire real account services and per-account workspaces into the shell`）：早期账户模块未挂载入主 Shell，需单独的 `tests/account.rs` 运行测试；而在 PR #38 中，真实账户服务、工作区和 UI 已经完全合入主应用 `src/main.rs`。此时 `tests/account.rs` 本身不声明任何新测试用例，仅通过 `#[path]` 将整个主应用模块树又镜像组装了一遍，导致 799 个测试在每次 `cargo test` 时完整运行两遍，且编译时产生了完全冗余的 `account-*.exe` 构建产物。
3. **保留其他集成目标的原因**：
   - `tests/profile.rs` 包含设计夹具集成断言用例 `design_fixture_renders_real_profile_content`。
   - `tests/reader_images.rs` 包含专属的 README 图片流水线视口集成验收测试 `view_tests`（共 14 项端到端用例）与夹具自测用例。
   - `tests/home.rs`、`tests/repository.rs`、`tests/repository_overview.rs`、`tests/reader.rs` 为对应组件的独立组装验证，且并发分支（如 `home-feed-focus`）正在密集修改 Home 相关代码。
   - 为了严格遵守任务约定的并发边界（不修改 `src/domain/home*`、`src/github/home*`、`src/ui/home*` 等并发分支所有文件），避免影响集成，采用最小侵入且收益最确凿的优化：彻底移除镜像无新增断言的 `tests/account.rs`，并清理 `examples/account.rs` 与 `src/bin_support/account_*.rs` 中对其的历史引用注释。

### 结果与等价性保证
- 移除了冗余的 `tests/account.rs`。
- 自动化等价性核验表明：1,102 个唯一规范化测试场景在修改后依然 100% 存在并被执行，0 个测试被遗漏或丢失。
- 所有的 799 个核心领域、认证、存储、网络、账户以及 UI 逻辑测试依然在 `githux` (bin) 单元测试套件中正常且唯一地运行。

## 验证与性能实测数据

环境：Windows 11 x86_64, rustc / cargo 固定工具链 1.98.1。

### 1. 测试用例数与执行时间对比（Sequential 测量）
| 指标 | 优化前 (Baseline) | 优化后 (Post-refactor) | 变化 |
| --- | --- | --- | --- |
| 测试 target 数量 | 8 | 7 | -1 (`account` removed) |
| 总测试执行次数 | 3,084 | 2,285 | -799 (-25.9%) |
| 独立测试场景数 | 1,102 | 1,102 | 100% 保持，无丢失 |
| 顺序运行总耗时 | 99.11s | 66.99s | -32.12s (提速 ~32.4%) |

各 Target 耗时明细（Post-refactor）：
- `githux` (bin): 31.35s (799 tests)
- `tests/home.rs`: 1.06s (57 tests)
- `tests/profile.rs`: 7.81s (197 tests)
- `tests/reader.rs`: 7.37s (168 tests)
- `tests/reader_images.rs`: 8.83s (485 tests)
- `tests/repository.rs`: 9.68s (559 tests)
- `tests/repository_overview.rs`: 0.88s (20 tests)

### 2. 编译与格式完整性验证
通过串行锁 `.agents/task/cargo-serialized.ps1` 执行：
- `cargo fmt --all -- --check`：通过（exit 0）。
- `cargo check --locked --all-targets`：通过（exit 0）。
- `cargo clippy --locked --all-targets -- -D warnings`：通过（exit 0）。
- `cargo test --locked`：全量测试套件通过（2,285 passed, 0 failed, exit 0）。
- `cargo build --locked`：主二进制正常编译构建成功（exit 0）。
- 验证全部 9 个 examples（`account`, `avatar`, `fonts`, `home`, `profile`, `reader`, `reader_images`, `repository`, `repository_overview`）：`cargo check --example <name>` 全部编译通过（exit 0）。

## 后续与协调事项
- 本分支直接针对 `feature/shell-pages`。
- 本任务未修改任何并发分支 `home-feed-focus` 或 `verification-workflow` 所属的文件。
- PR 待提交，Milestone 关联 First Release (#1)，按规则保持 `pending`，等待负责人验收，不进行自行合并。
