# 可复用只读阅读器（content-reader）

- 日期：2026-10-25；任务 ID：`content-reader`；分支 `feature/content-reader`，base `42291f7`（main）。
- 目标：先用确定性内容 fixture 交付真实 Ely Markdown 与源码阅读窗口，验证长文、选择复制、中文与输入行为；后续只需把已准备内容接入。状态：**实现与预览完成，未接入主界面**。

## 改动与决定

- 新增 `src/ui/reader/**`：`mod.rs`（`ReaderView`、`DocumentKind`、`ResourceContext`、`ReaderAction`、状态面板）、`source.rs`（只读源码视图）、`links.rs`（版本化链接解析）、`fixtures.rs`（验收内容）、`tests.rs`（测试）。
- 新增 `examples/reader.rs`：`cargo run --example reader` 打开独立预览，用 `#[path]` 复用真实 `content`/`domain`/`platform::fonts`/`ui::reader` 模块，不复制后端逻辑、不新增第二默认 bin、不改 `app` 入口或 `ui/mod.rs`。
- 新增 `tests/reader.rs`：只接线模块树，让阅读器的 `#[cfg(test)]` 测试进入 `cargo test`（example 目标的测试不会被执行）。
- 未新增任何依赖：复用 Ely 的 `documents::{MarkdownDocument, MarkdownMode}`、`forms::code_highlights`、主题字体与 gpui 的 `uniform_list`。`Cargo.toml` / `Cargo.lock` 未改（本任务虽独占，但无需变更）。
- **Ely `CodeEditor` 的核对结论**：`read_only()` 确实拒绝 `insert`/`paste`/`cut`/IME 写入，也有行号和纵向虚拟化；但只读态顶部固定绘制带「Make editable」按钮的 `ReadOnlyBanner`，且没有横向滚动。本轮要求「不得接受键入、粘贴、删除」，也要求长行可读，因此选择复用其公开高亮与列表能力、另写无输入路径的 `SourceView`，而不是打上游补丁或升级，见 [阅读器文档](../content-reader.md#只读源码)。
- **Ely `MarkdownDocument` 的边界**：长文本若经构造函数传入，异步读取 splice 进已按 0 项测量的列表后不绘制任何块。测试平台以 130 KiB 起稳定复现，且直接用 `MarkdownDocument`（不经本模块）同样复现，所以判定为上游行为而非本模块回归。改为 `new(String::new(), ..)` + `reset(text, cx)` 后，2 MiB 内均能只绘制可见块。选择记录在代码注释与主题文档，不改 Ely revision、不做本地 vendor。

## 验证

实际执行（`D:\githux-content-reader`，每个重命令 `CARGO_BUILD_JOBS=1`）：

```sh
cargo fmt --all -- --check
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

全部通过。`cargo test --locked` 结果为 189 个既有用例 + `tests/reader.rs` 的 16 个阅读器用例全部成功。

阅读器用例覆盖：

- 链接：相对路径保留 owner/name/**commit**/目录；`..` 攀升并在根停止；查询串丢弃；`%XX` 解码；`#fragment` 报告；`http(s)` 为外部；`javascript:`/`data:`/`file:`/`mailto:`/`ms-settings:`/`vbscript:` 与 `//host`、空地址被拒；`DocumentKind::for_path` 判定。
- 只读：点击源码后 `simulate_input("hello 世界 🌍")` 与 `backspace delete enter tab` 前后全文一致。
- 选择复制：拖选产生选区，`Ctrl+C` 让剪贴板内容成为源码子串。
- 大内容：约 2 万行源码只绘制可见行（`reader-source-row-20000` 不存在）且可滚动；接近 2 MiB 的 Markdown 只绘制可见块；`Empty`/`Unsupported`/`TooLarge` 均绘制状态面板。
- 横向：最宽行宽度大于视口，`Shift`+水平 wheel 让偏移 x 变为负值。
- 链接动作：Markdown 中按下链接后 `last_action()` 是 `Repository`（携带固定 commit）或 `Refused(Scheme)`，阅读器自身不打开。

**未覆盖（明确列为待验收）**：真实 Windows 窗口的鼠标拖选与 `Ctrl+C`、`Shift`+滚轮横向滚动、源码视图里的中文 IME 组合态、emoji/中文字形回退与多 DPI；测试平台不经过 Windows 原生输入分发。锚点滚动、图片注入与网络图片、Markdown 与 github.com 的完全等价性都不在本轮范围。

## 2026-10-26 审查修复

PR [#27](https://github.com/nocpiun/githux/pull/27) 已同步 main `84b029e`，保留独立预览范围。对 `28fdc9b` 的 [Codex 审查](https://github.com/nocpiun/githux/pull/27#pullrequestreview-5646893714) 提出两项成立的源码阅读问题，均按原任务修复，无待裁决事项：

- 高亮逐段从头查找语法区间产生平方级开销。改为顺序遍历不重叠语法区间、填补空白并只按选区的两个边缘切分。新增单行约 13 万语法区间的回归用例（宽松 5 秒预算）和中文/空白选区样式用例。
- 按字节长度选 8 行无法证明覆盖视觉最宽行。保留其作为初始探测，每个可见行依据实际排版结果上报宽度并扩展横向范围。新增 8 行中文 + 第 9 行较少字节但更宽 ASCII 的布局回归，确认最右端滚动可达。

本轮 fmt、`check --locked --all-targets`、`clippy --locked --all-targets -- -D warnings`、`test --locked` 与 `build --locked --example reader` 均通过。完整测试结果为 345 项主程序测试，以及 `tests/reader.rs` 的 24 项（19 项阅读器 + 5 项由复用 domain 模块带入的测试）。真实 Windows 验收仍待负责人完成；此前进程有窗口句柄而屏幕捕获仅见墙纸，不能据此宣布视觉或原生输入通过。

## 后续

- 接入前置：`ui/mod.rs` 增加模块声明；仓库页面把 `data` 层的正文与会话上下文交给 `ReaderView`，并由集成层实现 `ReaderAction`（外部 URL、同 commit 文件切换）。
- 真实窗口验收未做，`merge_owner=coordinator`、`human_review_required=true / approved=false`：PR 完成后保持 OPEN，由负责人真实验收后再决定合并。
- 若上游后续修好「构造期长文本不绘制」的问题，可去掉 `reset` 变通并删除对应注释。
