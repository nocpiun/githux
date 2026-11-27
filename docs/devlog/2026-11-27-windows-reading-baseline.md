# 2026-11-27：Windows 阅读路径基线测量（windows-reading-baseline）

## 交付内容

- 新增 `src/app/pages/reading_baseline_tests.rs`（挂载于 `src/app/pages/tests.rs`，标记 `#[ignore]` 以免影响常规 CI 门槛）：使用正式 `Shell`、`Accounts`、`AccountRuntime`、`CacheStore`、临时目录与确定性 `BaselineTransport`，执行 N=5 轮冷缓存启动、真实关闭释放后暖重开（150ms 延迟）、12.5 KB 含 4 张 1200×800 PNG 的长 README 滚动、Issues 第 2 页分页、Issue #42 详情、PR #5 Files changed 懒加载、5 次暖标签切换、9 标签打开/8 标签关闭内存采样与磁盘缓存统计。
- 新增 `examples/reading_baseline_native.rs`：使用真实 `gpui_platform::application()` 与隐藏不聚焦窗口（`show: false, focus: false`），在隔离临时目录下独立测量原生 Win32/Direct2D/DirectWrite 与 `Shell::new` 启动耗时及原生工作集/私有提交内存（N=5 次独立进程）。
- 新增共享基线文档 [docs/windows-reading-baseline.md](../windows-reading-baseline.md)，汇总环境、fixture 规模、冷/暖对比、内存/磁盘/请求计数与未覆盖边界。
- 按范围仅更新 [docs/roadmap.md](../roadmap.md)：修正 #115（PR Conversation 离线缓存）与 #116（正式 Shell 挂载 `SettingsView`）的过时状态描述，并在 R6 引用本次基线文档。

## 验证证据

- `node scripts/verify.mjs -- cargo test --locked --bin githux app::pages::tests::reading_baseline_tests -- --ignored --nocapture --test-threads=1`：在 `main@e175fed` 上 run `5d77c24c-7c05-4cdd-aad0-e0a0a618ab1e` 通过（running 93,004ms, slot wait 85,881ms），5 轮冷/暖与真实 `CacheStore` 关闭重开均成功（已按审查反馈等待页面 `active_refresh_state().is_refreshing() == false` 并提供完整 25 条 Issue 评论与 20 条 PR 评论）。
- `node scripts/verify.mjs -- cargo build --locked --example reading_baseline_native`：run `3dfd8361-cac1-4455-a0f5-f2f47ba14cdb` 通过（running 31,813ms, slot wait 57,189ms），随后连续 5 次独立运行 `./target/debug/examples/reading_baseline_native.exe` 均输出 `NATIVE_PROBE_REPORT` 并正常退出。

## 未覆盖与后续

- 独立 GPU 显存、真实 PAT/公网 GitHub、可见窗口人工滚动/多 DPI/IME 与 `release` 构建表现仍待 integrated main 综合验收时补充。
