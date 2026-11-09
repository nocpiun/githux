# NSH Reader 原位替换

- 日期：2026-11-09；任务 ID：nsh-reader-inplace；执行者：本任务 worker（分支 `migration/nsh-reader-inplace`，worktree `D:\githux-nsh-reader-inplace`）。PR/Issue：本任务为本地交付，无 PR。
- 目标与状态：**实现中**。把阶段一的 NSH Reader 从 `src/nsh/reader` 原位搬回 `src/ui/reader`，删除旧 GPUI Reader，公开 API 唯一前缀改为 `crate::ui::reader`；保留旧 AST/HTML/link/image/scroll 测试映射。Reader 组件本身已通过探针编译、clippy、fmt 与 139+123 项测试；与其它页面/集成作者真实契约的组合尚未发生（本地集成边界）。

## 改动与决定

- 结构：删除 `src/nsh/reader/**`、旧 GPUI `src/ui/reader/{source,images_tests,layout_tests,motion_tests,tests,markdown/{document,inline}}.rs`；`src/ui/reader` 现为纯 NSH 组件（`mod.rs`/`model.rs`/`view.rs`/`images.rs`/`links.rs`/`fixtures.rs` + 保留的 `markdown/{ast,html,doctext,html_tests}.rs`）。
- 公开 API 前缀 `crate::ui::reader`，签名保持稳定：`ReaderHandle`/`ReaderDocument`/`ResourceContext`/`ImageBridge`/`ReaderLayout`/`ReaderAction`/`DocumentKind`。`ReaderAction` 增加了 `LayoutChanged { height }` 与 `AuthorizationRetired`（阶段一已有语义的落实）；`ResourceContext` 字段公开并补 `owner()/name()/commit()/path()`。
- 图片桥：`ImageBridge` 增加 `Released` 状态（预算释放后同代次不重复请求，隐藏/恢复后重新请求）、`is_retired()`/`limits()`；撤权/迟到结果/预算释放语义见 `docs/nsh-reader.md`。
- 长文：bounded 布局块数超过 `WINDOW_AFTER=512` 时用 NSH `virtual_list` 挂载，Flow 与普通文档保持全量；虚拟化下 Ctrl+A 覆盖范围与全量不同，已记录为剩余缺口（不静默降级）。
- 状态文案：状态面板文案改为英文（`This file is empty.` / `This file cannot be shown as text.` / `This file is too large to show.`），与阶段一中文文案的差异见 `docs/nsh-reader.md`，待负责人确认。

## 验证

全部经 `node scripts/verify.mjs -- cargo …`（探针 package `.agents/task/probe`，被 Git 忽略，汇编真实 `src/ui/reader` + 真实 `content`/`domain`）：

- `cargo test --test reader`：**139 passed**（链接/文档种类/状态/GFM/HTML/跨块选择 scope/Flow/长文/链接报告/替换/撤权/图片占位）。
- `cargo test --test reader_images`：**645 passed**（含既有业务回归 + 新增桥回归 + 真实 `RepositoryImageService` 经桥加载）。其中 `network::transport::bounded_tests::chunked_and_no_length_stop_before_eof_…` 在并行全量下曾 flaky，单跑通过，属既有计时测试，非本轮改动。
- `cargo test --test reader_compatibility`：**123 passed**（headless NSH 兼容证据）。
- `cargo clippy --tests --bins --examples`：**通过，0 warning**。
- `cargo fmt --check`：**通过**。
- 探针实测（未提交 `--test measure`）：2 MiB/28488 块 `fixtures::large_markdown()` 首帧 `~188ms`、顶部挂载 33 行、`scroll_to_end` `~124ms` 直达文末。headless 探针数据，不冒充真实窗口验收。
- 未运行/未覆盖：真实 Windows 窗口的鼠标拖选、Ctrl+C、IME、锚点点击后滚动；与集成作者真实 `app::runtime`/`workspace` 契约的组合验证；认证图片 CacheStore 命中测试（由集成层保留其测试）。

## 后续

- 剩余缺口与集成注意事项见 `docs/nsh-reader.md`「已知缺口」：虚拟化下的跨屏全选、状态文案语言、原生交互验收、math 呈现差异。
- 集成作者需替换的测试注册/装配项记录在 `.agents/task/integration-notes.md`，本任务未改动根 `Cargo.toml`/`src/lib.rs`/`src/ui/mod.rs` 之外的任何非归属文件。
- 组合验证需待 home/profile/repository 页面任务与集成作者就绪后由集成作者统一执行；本任务不 busy-poll。
