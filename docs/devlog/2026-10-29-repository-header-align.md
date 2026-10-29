# Code 两列 header 底线对齐

任务：`repository-header-align`；基线 `3c9a1ae`，独立分支 `fix/repository-header-align`。

## 问题与实现

`src/ui/repository/view.rs` 的 tree/reader header 原来分别由内容加 `py_2` 决定高度。README 时两侧可能相同；打开源码后 reader 多了 Ely `ControlSize::Sm` 的 Back to README 按钮，右侧边框随之下降。Windows 150% DPI 原生预览已复现。新增实际渲染边界测试在修复前失败，分别观察到两侧底边相差 6 和 8 logical px。

两侧现在共用 `code_header`，统一 flex 行、垂直居中、不可收缩的 border-box 高度与底边框。高度为当前主题的 Sm control height + 0.5rem，可容纳按钮且随 rem/主题 density 缩放；没有绝对定位补偿。左右横向 padding 保留。reader 图标独立固定槽位，路径单行 Ellipsis 并提供完整 tooltip；返回按钮在独立有界槽位中显示。长路径、按钮出现/消失以及 reader 状态不再决定 header 高度。

文件树四边 7px inset、tree_row loading/retry、Reader 正文顶部/文末 padding 和页面底部 language bar 未改。没有新依赖、Ely 升级或 Cargo.lock 变动。

## 验证与证据

本地检查通过：`cargo fmt --all -- --check`、`cargo check --locked --all-targets`、`cargo test --locked --test repository`（490 passed）、`cargo clippy --locked --all-targets -- -D warnings`、`cargo build --locked --example repository`、`git diff --check`。重型 Cargo 均经任务提供的互斥包装，jobs=1，使用本 worktree target。

真实 Windows Ely 窗口（144 DPI / 150%）验证了 README → 展开 src → 打开 main.rs、缩窄窗口、逐层打开 docs/guides/nested/deep-note.md、File failure 场景错误 → 单次 Retry 恢复。修复前 `before-source.png` 可见两侧底线错位；修复后 `after-readme.png`、`after-source.png`、`after-narrow-source.png`、`after-narrow-deep-path.png`、`after-file-error.png`、`after-file-retry.png` 保留在 `.agents/output/`，底线保持一致。预览进程已关闭。

本地原生截图与检查日志不进入 Git；这是 repository example 的 synthetic fixture，并非实网账户验收。其他物理 DPI、跨屏切换、原生极长路径与 loading 瞬间截图尚未覆盖；对应布局压力与状态转换由测试平台验证，不能替代负责人真实窗口验收。

新增 `header_tests.rs` 通过真实 repository/service fixture 渲染并比较两侧 header 的 top/bottom/height；覆盖 README、源码按钮、长路径、1240/820/560 logical px 窗口、16/20px rem 和 1/1.25/1.5/2 测试平台 scale factor，以及真实文件请求 loading → failure → retry → showing → README。长路径是明确标注的布局压力状态，沿用真实 reader，仅替换显示中的 selection，不声称该路径是 fixture 可读取资源。

## 共享文档跟进与教训候选

本批主题文档由组合负责人维护，请在 `docs/repository-browser.md` 的 header 约定中补入共享 border-box 高度及状态切换不改变分隔线的行为。已阅读并应用 LESSONS：L002 要求分开报告测试平台、原生 fixture 与实网证据；L004 要求几何不挂平台分支；L008 的反向验证思想用于确认新增布局测试在修复前确实失败。

交 `workflow-lessons` 唯一写入者评估的有证据候选：相邻列要求边框对齐时，不能用相同上下 padding 推断几何相等；必须纳入条件按钮/图标、字体度量并测实际边界。此次源码按钮在真实 Windows 预览导致右侧 header 增高，且新增边界测试修复前失败。无需机械追加 LESSONS 条目，是否归入主题约定由协调者去重。

## 交付边界

按本任务要求 PR 关联 First Release milestone #1，添加 `pending`，等待负责人原生验收，不合并、不启用 auto-merge。最终 head、CI/review 链接和全程裁决记录保存在 PR 与 `.agents/task/delivery-state.json`，不为审查后补日志再制造新提交。

## 2026-10-29 rebase 到 feature/shell-pages 后的组合验证

负责人要求本 UI 任务 rebase 到 `feature/shell-pages`，PR 合并目标由 `main` 改为该分支（PR #42 为 Shell 实际页面接线）。rebase 由主协调者完成，无人工源码冲突；本地 `git diff --check`、`cargo fmt --all -- --check` 已通过。

组合基线以实时 `origin/feature/shell-pages` 为准。本任务发现并修复一处自己引入的可移植性问题：`header_tests.rs` 原来 `use crate::repository::{...}` 只存在于独立测试装配（`tests/repository.rs` 把 `repository` 挂在 crate 根）；rebase 后产品测试树把 `ui::repository` 真正挂载，该路径在产品 `--bin githux` 测试目标中不存在，CI code-check 报 `E0432`。改为 `use super::{ReaderStatus, ...}`（`tests.rs` 同一路径），测试逻辑不变。

本地检查（均经 `.agents/task/cargo-serialized.ps1`、jobs=1、本 worktree target）：

- `cargo fmt --all -- --check`：通过。
- `cargo check --locked --all-targets`：通过。
- `cargo test --locked --bin githux`：727 项通过（包含 Shell 真实页面挂载测试与 `header_tests`）。
- `cargo test --locked --test repository`：495 项通过。
- `cargo clippy --locked --all-targets -- -D warnings`、`cargo build --locked`：在串行锁上排队执行，结果待补（不预设通过）。

真实 Windows 组合验证：在包含 Shell 实际页面接线的正式 `target/debug/githux.exe`（不是 repository example）中核验 Code 两列 header 对齐；需要真实账户/凭据才能进入 Repository 页的路径不属于本任务合成证据范围，只记录已实际启动和可到达的步骤，截图写入被忽略的 `.agents/output/`。

Shell 本轮继续修复 Profile README Flow 与简介三行换行，可能推进基线；本任务不替 Shell 修改这两个功能。若实时 `origin/feature/shell-pages` 前进，按 `githux-pr-review` 的 `base_changed` 处理，必要 rebase 后重新对新 head 走 CI/review，不沿用旧基线通过结论。
