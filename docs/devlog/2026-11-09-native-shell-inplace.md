# Native Shell 原位替换：集成基线与页面契约

- 日期：2026-11-09。任务：native-shell（`migration/native-shell`），本地集成作者。
- 状态：公共契约与可编译的纯 Rust 默认应用落地；5 个页面任务尚未交付，真实页面待 cherry-pick。

## 改动与决定

- 根 package 原位替换为纯 Rust NSH：`src/main.rs` → `src/app::run` 使用
  `nsh_platform_windows::run_app(AppSource::Rust(Root))`，移除 Ely/GPUI/QuickJS。
- `src/nsh/runtime/**` 迁入 `src/app/runtime/**`；`sessions` 移入 `src/workspace/mod.rs`
  （纯 Rust，`Sessions: Deref<Target=Workspace>`）；`src/nsh/foundation/**` 迁入 `src/ui/foundation/**`；
  字体改由 `src/platform/fonts.rs` 的 `install(Root)` 注册。
- 新增 NSH `src/ui/mod.rs` Shell：标题栏/侧栏/标签条/palette/page host，前台 tick drain runtime、
  `binding_revision` 变化重建页面、poll 所有挂载页面、关闭 tab retire。
- 定义 `src/app/pages/mod.rs` 的 `PageView`/`PageNavigation`/`NavigationSink` 与 5 个页面工厂契约；
  契约桩先行，真实实现由页面任务交付。
- 重写共享 UI 为 NSH：`work_item_row`、`avatar`、`badge`、`compact_status`、`tooltip`；
  `geometry` 去 GPUI 依赖。
- 删除 `native/nsh`、旧 `src/nsh`、GPUI `src/app/accounts`、`src/platform/caption*`、`wasdk_bindings`
  与 GPUI 页面测试/示例（由各任务按拥有范围迁移）。

## 验证

- `cargo check`：成功（默认 lib+bin）。
- `cargo tree -e normal`：783 个包，无 `gpui`/`ely`/`quickjs`/`rquickjs`/`nsh-js`。
- 全量 `cargo build` 与真实窗口运行：受共享编译 mutex 与页面任务并发影响，见 implementation-result 的
  最新状态；`cargo check` 通过是独立可复现证据。
- 未运行：业务回归 `cargo test --lib`、真实窗口截图、页面组合验证（依赖页面任务）。

## 后续

- 5 个页面任务 ready 后 cherry-pick；替换 `src/app/pages/*`、`src/ui/repository`、`src/ui/accounts` 契约桩。
- `nsh-reader-inplace` 交付 `src/ui/reader/**`。
- 迁移页面测试/示例；移除 runtime `#![allow(dead_code)]`；最终真实窗口验收。
