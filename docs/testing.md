# 测试拓扑与测试运行优化

更新：2026-11-19。本文件记录 Githux 项目的 Cargo 测试目标、模块装配拓扑、测试用例分布与防重复执行约束。

## 架构背景与测试拓扑

Githux 为单 Rust package（edition 2024）。在早期开发阶段，为支持独立组件的 preview（例如 `examples/account.rs`、`examples/reader.rs` 等）并在 `cargo test` 中运行对应的单元测试，各子模块曾各自建立了集成测试入口（`tests/*.rs`）。随着各页面陆续正式接入主二进制 `src/main.rs`，这些通过 `#[path]` 重新挂载同一组 `src/` 模块的 `tests/*.rs` 演变为高倍率重复编译与重复执行。

### 目标与用例归属

1. **主二进制应用（`src/main.rs`）**：
   - 包含完整的应用产品模块树：`app`、`auth`、`content`、`data`、`diagnostics`、`domain`、`github`、`network`、`platform`、`storage`、`ui`、`workspace`。
   - 仅在 `#[cfg(test)]` 下挂载测试专用开发夹具与验收测试，确保 `cargo build --locked` 生产构建不夹带任何测试夹具：
     - `bin_support/test_repository_modules.rs`（`ui::repository` 共享夹具）
     - `bin_support/profile_fixtures.rs` 与 `ui/profile_fixture_tests.rs`（Profile 设计夹具几何测试）
     - `bin_support/reader_images_fixture.rs`、`bin_support/reader_images_performance_fixture.rs`、`bin_support/reader_images_view_tests.rs`（README 图片夹具自测与端到端视图验收测试）
     - `ui/scroll_viewports_tests.rs`（生产 Shell 与页面视口几何集成测试）
   - 主二进制测试目标覆盖全部独立测试场景（以 `cargo test --locked --bins --tests` 实测结果为准；Linux 与 Windows 因 `#[cfg(target_os = "windows")]` 用例存在平台差异，见 [2026-11-19 开发日志](devlog/2026-11-19-ci-test-speedup.md)）。

2. **独立预览示例（`examples/*.rs`）**：
   - 13 个交互预览继续通过 `src/bin_support/*_modules.rs` 与对应夹具编译运行，由 `cargo clippy --locked --all-targets -- -D warnings` 统一守护无警告可编译；常规测试执行使用 `--bins --tests`，避免对 13 个示例重复做完整 codegen 与链接。

3. **已消除的重复目标**：
   - 2026-10-31：移除与 `src/main.rs` 完全重复的 `tests/account.rs`。
   - 2026-11-19：将仅存于旧集成测试中的 39 项独有用例（`tests/scroll_viewports.rs` 1 项、`tests/profile.rs` 1 项、`tests/reader_images.rs` 37 项含 5 项既有 `#[ignore]`）完整迁入 `src/main.rs` 的 `#[cfg(test)]` 装配，并移除全部 8 个重复的 `tests/*.rs`（`home`、`notifications`、`profile`、`reader`、`reader_images`、`repository`、`repository_overview`、`scroll_viewports`）。独立测试场景零丢失，测试执行次数从 9 目标重复执行（Windows 6,564 / Linux 6,535）收敛为单目标执行（Windows 1,514 / Linux 1,505）。

## 测试执行与维护规则

- **新增测试用例**：
  - 业务模块单元测试与实现放在同一模块旁（`mod tests`），或按既有模式仅在 `src/main.rs` 的 `#[cfg(test)]` 下挂载。
  - 不要在 `tests/` 下通过 `#[path]` 重新挂载 `src/` 模块树建立镜像 target，避免成倍重复编译与重复执行。
  - 未来如新增不重复挂载 `src/` 树的轻量独立集成测试，`cargo test --locked --bins --tests` 会自动将其纳入。
- **验证命令**：
  - `cargo clippy --locked --all-targets -- -D warnings`：检查主应用、测试装配与全部 13 个 examples。
  - `cargo build --locked`：验证生产二进制构建且不引入 `#[cfg(test)]` 夹具。
  - `cargo test --locked --bins --tests`：运行全部二进制与集成测试目标（CI 同时设置 `CARGO_PROFILE_TEST_DEBUG=line-tables-only`）。
