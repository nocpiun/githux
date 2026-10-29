# reader-html：Markdown 中的 HTML 与注释

- 日期：2026-10-29（Asia/Shanghai），任务 `reader-html`，分支 `fix/reader-html`。
- 状态：实现与本地验证完成，待真实窗口人工验收（PR 加 `pending`，不合并）。
- 目标：README 中的 `<div align="center">`、HTML 注释、常见 HTML 标签不再作为原文显示；保留 Reader 的跨块选择、复制与图片接口。

## 改动与决定

- 新增 `src/ui/reader/markdown/html.rs`：按 WHATWG HTML 语法/分词章节实现的有界子集解析器。只做分词、属性与实体的最小解码、容错元素树，不执行、不解析 URL、不回写原始 HTML。注释、doctype 与处理指令直接丢弃；`script`/`style`/`template` 及其内容整段丢弃；`iframe`/`object`/`embed`/`head` 等非内容元素也丢弃。
- `ast.rs` 不再生成 `Block::Html`：块级 HTML 先缓冲到 `End(HtmlBlock)`，再由 `html_blocks` 转成既有 `Block`；行内 `Html`/`InlineHtml` 映射到既有 `Inline` 的样式与链接范围。
  - 容器：`div`/`p`/`center`/`section`/`details`/`summary` 等展开为段落；`h1`–`h6` 成为标题；`ul`/`ol`/`li` 成为列表；`blockquote`/`hr`/`br` 对应引用/分隔线/换行。
  - 行内：`strong`/`b`、`em`/`i`、`del`/`s`、`code`/`kbd`/`samp`、`a href` 映射到与 Markdown 相同的 `Style`/`links`；HTML 内联元素在 `InlineHtml` 事件之间用独立栈维护，`</tag>` 通过新增的 `Node::End` 关闭，不干扰 Markdown 的 open 栈。
  - 图片：`<img src alt>` 生成 `Block::Image`（单元格内保留 alt 文本），URL 不在这里解析，仍由 reader 的 `links::resolve` 与图片管线判定，保持同一账号/commit 图片身份与既有接口。
  - 表格：`table`/`thead`/`tbody`/`tr`/`th`/`td` 转成 `Block::Table`；首行全 `th` 作表头，否则第一行提升为表头；`align`/`style="text-align:..."` 决定列对齐；单元格内块级内容展平为行内文本。
  - 代码：`pre`/`code` 成为 `Block::Code`（`class="language-x"` 取语言）；Markdown 行内代码与围栏代码是 `Code`/`CodeBlock` 事件，HTML 原样保留。
- 删除注释后相邻文本的空格会合并（`html_gap`），因此复制文本读作 `before after` 而不是 `before  after`；跨行与行内注释都不进入 `DocText`。
- `Block::Html` 变体保留但不再被生成，以免改动 reader-images 任务独占的 `document.rs`/`doctext.rs`。其删除与渲染分支清理留给两者合并后统一处理。
- 明确边界（已写入 [阅读器 HTML 子集](../../content-reader.md#html-子集2026-10-29)）：`align` 等布局属性忽略视觉对齐（内容保留）、不支持 `colspan`/`rowspan`、SVG/MathML/CSS 布局、`<details>` 折叠交互；未闭合标签按容错规则丢弃或展开。没有新增依赖，未改 `Cargo.toml`/`Cargo.lock`，未触碰 `src/ui/reader/mod.rs`、`markdown/document.rs` 或图片网络模块。
- 预览便利：`examples/reader.rs` 新增 `HTML` fixture 与可选的首个命令行参数（fixture 索引），只用于脚本化截图，不影响默认启动。

## 验证

- `cargo check --locked --all-targets`：通过（4m06s，首次依赖构建）。
- `cargo clippy --locked --all-targets -- -D warnings`：通过。
- `cargo test --locked`：全部通过（599+599+54+168+147+500+16 项，0 失败），其中 `tests/reader.rs` 147 项含新增的 11 项 `markdown::html_tests`，覆盖用户场景 fixture、注释（跨行/行内）不进入模型与复制、容器/强调/链接/图片/表格/列表语义、代码内 HTML 原样、脚本与事件属性不可见、单元格展平、实体解码、标题索引、畸形边界与 UTF-8 边界。
- `cargo build --locked --example reader`：通过。
- `cargo fmt --all -- --check`、`git diff --check`：通过。
- 真实原生预览（同一 fixture，1280x900，DPI 144，Windows）：
  - 修复前：`target` 基线 `ast.rs` + 同一 fixture，`.agents/output/before-html.png` 显示 `<div align="center">`、`<img>`、`<!-- ... -->` 与 `<table>` 等作为灰色/正文文本原样出现。
  - 修复后：`after-html.png`（已另存为 `evidence-after-top.png`）显示图片占位「项目徽标 — picture not loaded」、加粗/斜体/行内代码/链接、`展开说明` 加粗、带列对齐的表格与列表；`after-html-bottom.png` 显示行内代码中的 `<div align="center">`/`<span>x</span>` 原样保留、围栏 HTML 代码块（含注释）原样保留、注释与 `<script>` 不出现在正文、末尾「尾部文本。」正常。这些截图为审查修复前的同一行为实现；审查修复只增加深度上限与实体解码窗口，不改变正常 fixture 行为。
  - 证据保存注意：窗口截图需先 `MoveWindow` 定位再抓取；直接 `SetForegroundWindow` 后抓屏可能拍到桌面壁纸，不能作为行为证据。
  - 未覆盖：图片真正加载依赖尚未合入的 reader-images 任务，本任务只保证 `<img>` 进入既有 `Block::Image` 管线；真实 IME/多 DPI/跨屏仍未由本任务验收。

## 后续

- 与 reader-images 合并时处理 `docs/content-reader.md`、`docs/content-reader-markdown-patch.md` 的同文件冲突；两边都改了这两份主题文档。
- 合并 reader-images 后确认 `<img>` 在真实仓库正文中加载，并考虑移除 `Block::Html` 及其渲染/复制分支。
- 候选可复用教训（交由本批 workflow-lessons 决定是否写入 `LESSONS.md`）：构建容错 HTML 树时不要把同一节点先克隆进结果、再把原件压栈；否则子节点会挂到被丢弃的副本上，解析结果看似“标签全部消失”。本任务第一版即因此让所有块级 HTML 变成空块，靠 fixture 测试暴露，修复为闭合时自底向上挂载。证据：`html_tests` 首轮 11 项失败、修复后 147 项通过。
