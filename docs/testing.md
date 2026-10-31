# 测试拓扑与测试运行优化

本文件记录 Githux 项目的 Cargo 测试目标、模块装配拓扑、测试用例分布与防重复执行约束。

## 架构背景与测试拓扑

Githux 为单 Rust package（edition 2024）。在早期开发阶段，为支持独立组件的 preview（例如 `examples/account.rs`、`examples/reader.rs` 等）并在 `cargo test` 中运行对应的单元测试，各子模块曾各自建立了集成测试入口（`tests/*.rs`）。

### 目标与用例归属

1. **主二进制库/应用（`src/main.rs`）**：
   - 包含了完整的应用产品模块树：`app`、`auth`、`content`、`data`、`diagnostics`、`domain`、`github`、`network`、`platform`、`storage`、`ui`、`workspace`。
   - 在 `cfg(test)` 下装配 `bin_support/test_repository_modules.rs`（开发 fixture），使 `ui::repository` 的全套单元测试与集成测试均在主 binary 单元测试套件中编译和执行。
   - 主二进制目标共运行 799 个测试用例，覆盖全部通用领域、网络、存储、认证、UI 基础逻辑与 Shell Composition。

2. **领域与专用组件集成测试（`tests/*.rs`）**：
   - `tests/home.rs`：测试 Home / Feed 模块装配。
   - `tests/profile.rs`：包含 Profile Overview 专属设计夹具断言测试 `design_fixture_renders_real_profile_content`。
   - `tests/reader.rs`：独立 Reader 视图装配。
   - `tests/reader_images.rs`：包含 README 图片管线集成验收测试 `view_tests`（共 14 项端到端流水线场景及 3 项夹具自测用例）。
   - `tests/repository.rs`：独立 Repository 页面装配测试。
   - `tests/repository_overview.rs`：独立 Repository Overview 顶部组件测试。

3. **已消除的重复目标**：
   - `tests/account.rs`：历史遗留的重复集成测试入口。在 PR #38 将真实账户服务与工作区接回 Shell 后，主二进制已完整挂载全部账户与工作区模块（共 799 项测试）。`tests/account.rs` 中没有任何独占的 `#[test]` 或 `#[gpui::test]`，其 799 项测试与 `src/main.rs` 完全逐字同源同名且 100% 重复。移除 `tests/account.rs` 彻底消除了这 799 次重复执行（节约 ~31.3 秒测试运行时间），且未丢失任何断言场景。

## 测试执行与维护规则

- **新增测试用例**：
  - 业务模块单元测试应当与实现放置在同一模块旁（`mod tests`）。
  - 若主二进制已覆盖该模块，无需在 `tests/` 下建立完全同构的镜像 target，避免产生双倍编译与双倍测试执行。
  - 仅当测试涉及独立的外部预览装配、特殊的跨模块集成验收（例如 `tests/profile.rs` 的 fixture 测试或 `tests/reader_images.rs` 的 view_tests）时，才使用单独的集成测试目标。
- **验证命令**：
  - `cargo test --bin githux`：运行主二进制所有核心逻辑与页面测试。
  - `cargo test`：运行全套测试套件。
