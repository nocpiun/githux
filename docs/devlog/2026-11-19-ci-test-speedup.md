# CI 全量测试去重与流程减脂（B1–B3）

- 日期：2026-11-19
- 任务 ID：`ci-test-speedup`
- 分支：`chore/ci-test-speedup`（基线 `fd74eecf45eeab63e3932b057c35a6aaf6496a15`）
- PR：[#98](https://github.com/nocpiun/githux/pull/98)（Milestone #1 First Release）
- 状态：待最终审查与合并（已完成代码实现、本地等价性核验及首轮 PR CI 实测）

## 改动与决定

同步维护主题文档：[共同代码与验证约定](../code-standards.md)、[测试拓扑与测试运行优化](../testing.md)。

1. **迁移全部 39 项独有用例入主二进制 `#[cfg(test)]`，移除 8 个重复 `tests/*.rs`（B1）**
   - 迁移前在相同基线 `fd74eecf` 上通过 `scripts/verify.mjs` 完整采集 9 目标 `--list` 与 `--list --ignored` 清单，按精确模块别名映射（`*_modules::` → `ui::`、`reader_images_modules::fixture::` → `fixture::`、`view_tests::` → `reader_images_view_tests::`）逐条核对完整测试身份与 `#[ignore]` 属性，确认独有用例共 39 项：
     - `tests/scroll_viewports.rs`（1 项）→ 迁至 `src/ui/scroll_viewports_tests.rs`，仅在 `src/main.rs` 下以 `#[cfg(test)]` 挂载（避免影响挂载 `src/ui/mod.rs` 的 `examples/*.rs`）。
     - `tests/profile.rs`（1 项）→ 迁至 `src/ui/profile_fixture_tests.rs`，在 `src/main.rs` 下以 `#[cfg(test)]` 挂载 `bin_support/profile_fixtures.rs` 与 `ui::profile` 别名。
     - `tests/reader_images.rs`（37 项，含 5 项既有 `#[ignore]`）→ 在 `src/main.rs` 下以 `#[cfg(test)]` 挂载 `bin_support/reader_images_fixture.rs`（3 项夹具自测）、`bin_support/reader_images_performance_fixture.rs` 与 `bin_support/reader_images_view_tests.rs`（34 项，含 5 项既有 `#[ignore]`）及 `ui::reader` 别名。
     - `tests/home.rs`、`tests/notifications.rs`、`tests/reader.rs`、`tests/repository.rs`、`tests/repository_overview.rs`（0 项独有用例）连同上述 3 个已迁出的文件全部删除，共删除 8 个重复 `tests/*.rs` 目标。
   - 保留 `src/bin_support/*` 与全部 13 个 `examples/*.rs` 完整可用，清理 `src/bin_support/*` 中指向已删除 `tests/*.rs` 的过期注释；仅对测试装配中的两个预览夹具模块 `profile_fixtures` 与 `fixture` 局部添加 `#[allow(dead_code)]` 并附原因（遵循 L006）。生产构建 `cargo build --locked` 不包含任何 `#[cfg(test)]` 夹具。

2. **精简 `.github/workflows/ci.yml` 的 `code-check` 与 Linux `build`（B2）**
   - 删除 `code-check` 中与 `cargo clippy --locked --all-targets -- -D warnings` 重复的 `Cargo check` 步骤；保持 job ID `code-check` 不变，显示名调整为 `Code check (fmt, clippy, test)`，不触碰 `ci-scope.mjs` 契约。
   - 将 `Test` 步骤由 `cargo test --locked` 改为 `cargo test --locked --bins --tests`，保留未来独立集成测试自动纳入能力，避免 `cargo test` 重复对 13 个 `examples/*.rs` 做完整 codegen 与链接（13 个示例仍由 `clippy --all-targets` 全量检查）。
   - 在 `code-check` job 设置 `CARGO_PROFILE_TEST_DEBUG: line-tables-only`，保留 panic 堆栈文件/行号同时缩小测试产物与 `Swatinem/rust-cache` 读写体积。
   - 在 `code-check` 与 Linux `build` 的 `apt-get install` 中移除 `Cargo.lock` Linux 依赖树未使用的 `clang` 与 `cmake`，已在 PR #98 的 Linux `code-check` 与 `Build (x86_64-unknown-linux-gnu)` 实测通过，apt 下载包体积由 `16.8 MB` 降至 `3.26 MB`（减少约 81%）。未额外引入 `lld` 包：慢速 Ubuntu apt 镜像已是已观测耗时瓶颈，且单目标 + `line-tables-only` 的全冷编+链接仅 2m 41s，未增加额外 apt 依赖。

## 测试清单等价性与平台差异核验

同平台（Windows）、同基线（`fd74eecf`）逐条完整测试身份与 ignore 属性比对结果：

| 维度 | 改动前（9 目标） | 改动后（1 目标） | Diff（丢失 / 新增 / ignore 变更） |
| --- | ---: | ---: | --- |
| Windows 总执行次数 | 6,564（含 22 次 ignore） | 1,514（含 8 次 ignore） | — |
| Windows 独立测试场景 | 1,514（含 8 项 ignore） | 1,514（含 8 项 ignore） | **0 / 0 / 0** |
| Linux 实测（减 9 项 Windows 专属 cfg 用例） | 6,535（9 目标，22 次 ignore） / 1,505 独立 | 1,505（1,497 passed + 8 ignored，0 failed） | **0 / 0 / 0**（run `44484467793` 实测确认） |

Windows 相比 Linux 多出的 **9 项独立测试**（改动前在 9 目标中合计展开为 **29 次执行**，`6,564 - 29 = 6,535`、`1,514 - 9 = 1,505`）均由源码中显式 `#[cfg(target_os = "windows")]` 控制：
- `auth::windows::tests::windows_credential_roundtrip`（1 项，`src/auth/mod.rs:27`；改动前在 `main`/`notifications`/`reader_images`/`repository`/`scroll_viewports` 共 5 次）
- `platform::caption::*`（6 项，`src/platform/mod.rs:5`；改动前在 `main`/`repository`/`scroll_viewports` 共 18 次）
- `ui::home::tests::row_regressions::row_acceptance_bundled_native_fonts`（1 项，`src/ui/home/row_tests.rs:485`；改动前在 `main`/`home`/`scroll_viewports` 共 3 次）
- `ui::repository::tests::tab_boxes_keep_their_geometry_when_the_active_tab_changes_with_bundled_fonts`（1 项，`src/ui/repository/tests.rs:2482`；改动前在 `main`/`repository`/`scroll_viewports` 共 3 次）

8 项既有 `#[ignore]` 用例保持原状（未新增跳过，未改为执行）：
- `content::images::tests::public_svg_probe`
- `data::reader_images::tests::performance_tests::memory_probe::performance_nine_originals_memory_probe`
- `data::reader_images::tests::performance_tests::performance_original_raster_and_svg_stage_measurement`
- `reader_images_view_tests::performance::performance_original_host_resizes_columns_and_recovers_dpi_from_cache`
- `reader_images_view_tests::performance::performance_original_host_rows_flow`
- `reader_images_view_tests::performance::performance_original_host_rows_repository`
- `reader_images_view_tests::performance::performance_original_host_table_flow`
- `reader_images_view_tests::performance::performance_original_host_table_repository`

## 验证与耗时

### 本地 Windows 实测（`scripts/verify.mjs`，`CARGO_BUILD_JOBS=1`）

- 基线清单（改动前 9 目标）：
  - `cargo test --locked -- --list`（run `45c10794`）：exit 0，运行 762.8s（含首次工作树构建），槽位等待 0.2s
  - `cargo test --locked -- --list --ignored`（run `fa1d497f`）：exit 0，运行 3.9s，槽位等待 271.8s
- 改动后验证：
  - `cargo fmt --all -- --check`（run `ad41f247`）：exit 0，运行 14.5s（fmt 免锁）
  - `cargo test --locked --bins --tests -- --list`（run `bb01c5db`）：exit 0，运行 49.6s（增量重编单测试目标），槽位等待 0.2s
  - `cargo test --locked --bins --tests -- --list --ignored`（run `950cd3e1`）：exit 0，运行 1.0s，槽位等待 0.2s
  - `cargo test --locked --bins --tests`（run `566db997`）：exit 0，**1,506 passed; 0 failed; 8 ignored**（用例执行 32.75s，总运行 34.0s，槽位等待 0.2s）
  - `cargo clippy --locked --all-targets -- -D warnings`（run `b23a2379`）：exit 0，运行 249.4s，槽位等待 0.2s（覆盖主应用、测试装配与全部 13 个 examples）
  - `cargo build --locked`（run `b2989206`）：exit 0，运行 72.4s，槽位等待 0.2s（生产构建不含测试装配）
  - `node --test .github/scripts/ci-scope.test.mjs`：6/6 passed
  - `node --test .github/scripts/pr-policy.test.mjs`：8/8 passed
  - `git diff --check`：exit 0

### Linux CI 对比（`ubuntu-24.04`）

| 阶段 | 改动前基线（run `44452030561`，热缓存 / 慢 apt） | 改动前基线（run `44448952523`，冷测试编译 / 快 apt） | 改动后实测（PR #98 run `44484467793`，**完全冷缓存** / 极慢 apt） |
| --- | --- | --- | --- |
| `Install Linux dependencies` | 5m 15s（16.8 MB @ 61.1 kB/s） | 12s | 5m 46s（**3.26 MB** @ 11.0 kB/s，下载量 -81%） |
| `Install pinned Rust` + `Cache` | 8s + 25s | 10s + 20s | 8s + 5s（`No cache found.`） |
| `Format check` | 15s | 21s | 11s |
| `Cargo check` | 58s | 1m 15s | 已移除（由 `Clippy` 覆盖） |
| `Clippy` | 52s | 1m 11s | 1m 42s（冷缓存） |
| `Test` 编译与链接 | 4m 08s（9 测试目标 + 13 examples，热缓存） | 包含于下方 19m 26s | **2m 41s**（1 测试目标，`line-tables-only`，冷缓存） |
| `Test` 用例执行 | 8m 18s（6,535 次，9 目标） | 包含于下方 19m 26s | **2m 28s**（148.13s，1,497 passed + 8 ignored = 1,505，-70%） |
| `Test` 步骤合计 | **12m 26s** | **19m 26s** | **5m 10s**（冷缓存即比旧热缓存快 58%、比旧冷编快 73%） |
| Rust 检查与测试合计（`fmt`+`check`+`clippy`+`test`） | **14m 31s** | **22m 13s** | **7m 03s**（完全冷缓存） |
| `code-check` Job 总耗时 | **20m 29s** | **23m 04s** | **13m 26s**（含 5m 46s apt 限速与冷缓存重建） |

缓存与冷启动说明：
- PR #98 首轮 run `44484467793` 因新增 `CARGO_PROFILE_TEST_DEBUG=line-tables-only` 改变 `Swatinem/rust-cache` 环境哈希而触发完全冷启动（`No cache found.`），结束后已写入新缓存（约 1.04 GB zstd）。
- 同一轮 CI 中 `Build (x86_64-unknown-linux-gnu)` 同样验证了移除 `clang`/`cmake`（`3.26 MB` @ 15.2 kB/s，耗时 4m 03s；`Build executable` 1m 11s），四平台构建与 `Agent script checks`、`CI`、`Check` 全部通过。
- 热缓存场景下的最终实测耗时随最终提交的 PR #98 CI 运行记录于 PR 评论，不再为追录自身提交 SHA 额外产生文档提交。

## 未覆盖与延后项边界

- 本次严格限定在任务包 B（B1–B3），未实施 Home viewport 重型测试合并、第三方渲染 crate 优化等级调整、拆分 `lib.rs`、apt 缓存或容器化方案。
- Ubuntu 官方源偶发限速（本轮实测 11–15 kB/s）导致 `Install Linux dependencies` 耗时存在明显外部波动；本次仅通过移除 `clang`/`cmake` 将下载量压缩 81%（16.8 MB → 3.26 MB），未引入 apt 缓存或容器镜像。
