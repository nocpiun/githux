# 可复用只读阅读器

更新：2026-10-26（第二轮真实窗口反馈）。R3 的 UI 切片，位于 `src/ui/reader/**`、独立预览 `examples/reader.rs` 和测试入口 `tests/reader.rs`。它消费已有的 [`content::PreparedContent`](../src/content/mod.rs) 与 `domain::blob::BlobIdentity`，把「已准备好的正文」呈现成 Ely 的 Markdown 预览或本模块的只读源码视图；不读网络、不读凭据、不落盘、不下载图片。

**状态边界：** 阅读器组件与预览已实现并通过测试平台验证；**尚未接入 `src/ui/mod.rs` / `src/app` / 仓库页面**，因此主界面仍不显示真实仓库正文。接入属于后续 data⇄UI 接线任务，本轮不把预览当作「真实数据已接主界面」。

## 调用契约

```rust
use crate::ui::reader::{DocumentKind, ReaderView, ResourceContext, LinkTarget};

let context = ResourceContext::from_prepared(&prepared); // 或 ::from_identity(&BlobIdentity)
let kind = DocumentKind::for_path(context.path());
let view = cx.new(|cx| ReaderView::from_prepared(prepared, kind, window, cx));
view.update(cx, |view, _| {
    view.on_action(Rc::new(move |action, window, cx| match action {
        // 集成层决定真正打开外部 URL 或切换到同一 commit 的另一个文件。
        ReaderAction::Link(LinkTarget::Repository(link)) => open_same_commit(link),
        ReaderAction::Link(LinkTarget::External { url }) => cx.open_url(&url),
        ReaderAction::Link(_) => {}
    }));
});
```

- `PreparedBody::Utf8 { text, .. }` → `DocumentKind::Markdown` 走 Ely 的 `MarkdownDocument`（`Preview` 模式）；`DocumentKind::Source { language }` 走 [`SourceView`](../src/ui/reader/source.rs)。`DocumentKind::for_path` 只按文件名判断 README / `*.md` / `*.markdown` / `*.mdx`，集成层可覆盖。
- `Empty` / `Unsupported(..)` / `TooLarge { .. }` → 居中状态面板，分别是「文件为空」「无法作为文本显示」「超出读取上限」；这些是阅读器的实际结果，不是给未设计页面用的占位卡片。
- `ReaderView::show(body, kind, window, cx)` 在同一上下文里替换正文；`ReaderView::handle_link(url, ...)` 供集成层或测试直接投递一次链接动作。

## 链接与相对资源

链接不再由渲染器直接交给系统浏览器。`links::resolve(raw, &context)` 只做纯函数解析，输出结构化的 `LinkTarget`：

| 地址形态 | 结果 | 说明 |
| --- | --- | --- |
| `http:` / `https:` | `External { url }` | 由集成层调用系统打开；阅读器自身不打开 |
| 相对路径、`./`、`../`、`/abs` | `Repository { owner, name, commit, path, anchor }` | 携带**读取时的固定 commit**，不回退到分支 latest |
| `#fragment` 或 `file.md#fragment` | `Anchor { id }` 或带 `anchor` 的仓库链接 | 阅读器本轮不滚动锚点，只报告目标 |
| `javascript:` `data:` `file:` `mailto:` 其他 scheme | `Refused { url, reason: Scheme }` | 不交给系统，不执行 |
| `//host/path` | `Refused { reason: ProtocolRelative }` | 无仓库语义 |
| 空地址、空路径、`%XX` 非法、含反斜杠或控制字符 | `Refused { reason }` | 含反斜杠的路径在 Windows 语义下不可判定 |

`..` 向仓库根攀升并在根处停止（与浏览器一致）；查询串按静态文件地址丢弃；`%XX` 严格解码，坏转义整体拒绝。**没有** `AssetService`、网络图片下载或磁盘缓存。仓库 reader 在图片 loader 创建前拒绝所有文件系统与 data 来源（含 `file:`、绝对路径、UNC、带本地 base 的相对路径）；网络图片显示「未从网络载入」，相对资源只显示提示。图片不会绕过链接的危险 scheme 边界发起本地/SMB 读取。

`raw HTML` 由 Ely 的 GFM 标签过滤器处理：`script`/`style`/`iframe` 等按文本显示而不执行，`<a href>`/`<img src>` 转为文本与普通链接，因此也不存在走 HTML 执行脚本的路径。

## 只读源码

先核对了 Ely 的 `CodeEditor`（`src/editor/state.rs`、`view.rs`、`line.rs`）：它有行号、选区、`Ctrl+C`、`uniform_list` 纵向虚拟化，`read_only()` 会拒绝 `insert`/`paste`/`cut`/IME `replace_and_mark_text_in_range`。**但它仍有本轮不可接受的边界**：只读时顶部永远画一条带「Make editable」按钮的 `ReadOnlyBanner`（点击即解锁编辑），且 `overflow_hidden` 没有横向滚动，长行被裁掉。因此本模块不直接复用它，而是复用它的好部分——Ely 公开的 `code_highlights`、代码字体、gpui 的 `uniform_list` ——并删除全部输入路径：

- 没有文本输入处理器、没有 IME 处理器、没有编辑 action，因此键入、粘贴、删除都无法修改文档。测试用 `simulate_input("hello 世界 🌍")` 和 `backspace delete enter tab` 对比全文前后一致。
- 行号右对齐到实际行数位数，等宽字体取主题的 `mono_family`。行号 gutter 右侧间距是**原有 0.75rem 再增加 6px**（`source::gutter_gap`，16px rem 下约 18px），用真实布局 padding 实现、不是字符串空格；行文本与选择命中共用同一 `TextLayout` 起点，1/2/4/5 位行数的文字列不随之移动。
- 纵向/横向滚动条绑定 `UniformListScrollHandle.base_handle`（列表真实测量与滚动的那个句柄），不复用外层空容器 offset。范围直接来自列表的 `max_offset()`，不构建全量行。条使用与 workspace tabs 相同的 3px、20%/38% 透明度与 140ms `Fade` 状态机，见 [共享滚动条](#共享滚动条与未完成项)。
- 语法着色走 Ely 公开的 `code_highlights`（通用代码着色）。`language` 只是标签：Ely 把 `syntax_of` / `code_highlights_with` 保持为 crate 私有，按语言逐 fence 选语法会需要本地复制语法表或改上游，因此本轮明确只提供通用着色，不声称逐语言准确。
- 纵向用 `uniform_list`；横向用 `ListHorizontalSizingBehavior::Unconstrained` 让内容宽度可以超过视口。初始按字节长度取 8 个候选实测，这只是启动探测；每个可见行在 prepaint 时上报实际 `StyledText` 宽度，发现更宽的行便扩展横向范围。屏外行滚入视口后同样参与，避免中文/ASCII/emoji 混排时漏掉字节更短但视觉更宽的行，也避免预先排版所有行。选中背景在同一次 `StyledText` 布局里切分，不用第二套宽度估算；语法区间与选区边界顺序合并，复杂度为线性，不为每段重新扫描全部语法区间。
- 选择：左键按下/拖动扩展选区，`Shift` 扩展、`Ctrl+A` 全选、`Ctrl+C` 写入剪贴板。`Ctrl+A`/`Ctrl+C` 通过 `on_key_down` 检查 `secondary()`，不依赖焦点在某个输入控件上。

## 大内容与线程边界

| 内容 | 行为 | 证据 |
| --- | --- | --- |
| 源码接近 2 MiB | `uniform_list` 只构建可见行；约 2 万行的 fixture 中 `reader-source-row-20000` 不存在 | `a_large_source_only_draws_visible_rows` |
| Markdown 接近 2 MiB | Ely 在后台线程解析，只绘制可见块；`markdown-block-20000` 不存在 | `a_large_markdown_document_only_draws_visible_blocks` |
| 超过 2 MiB | `PreparedBody::TooLarge` 状态，不构建任何节点 | `empty_unsupported_and_too_large_draw_a_status` |

正文解析发生在 `MarkdownDocument` 自己的 `background_spawn`（Ely 的既有实现，`AT_ONCE = 128 KiB` 以内同步、以上异步），本模块没有新增 UI 线程解析。

**已定位的 Ely 行为边界：** 若把长文本直接传给 `MarkdownDocument::new(text, ..)`，其异步读取会 splice 进一个「已经按 0 项测量过」的列表，结果**一个块都不绘制**。在测试平台上以 130 KiB 起稳定复现（独立于本模块，直接用 `MarkdownDocument` 也复现）。`new(String::new(), ..)` 后再 `reset(text, cx)` 会请求在读取落地后定位到首行，列表仍锚定在 block 0，因此能正常绘制到 2 MiB。本模块因此采用后者，并在注释与开发日志里记录原因。这是上游行为观察，不是本仓库的补丁，也不改 Ely revision。

## Markdown 阅读（本地维护组件）

Markdown 阅读使用 `src/ui/reader/markdown/` 内由 Githux 单独维护的组件（来源、映射与维护规则见 [Markdown 组件边界](content-reader-markdown-patch.md)）：公开 API 仍使用官方 `ely-core` 的主题/字体/图标/代码着色，解析使用 `pulldown-cmark`。Source 仍是 reader 的只读虚拟列表。

- 文档使用真实 `gpui::ListState` 虚拟化块；只有可见块被构建。公开 `list_state()` 是真实滚动源，当前由 reader 的 3px 圆角半透明滚动条读取，hover/拖动使用 140ms 双向 `Fade`；无溢出时不构建条。
- inline code 使用主题 `mono_family`（Google Sans Code）、3.5px 左右与 1px 上下真实 padding、4px 圆角；padding 参与换行/绘制/命中的同一排版坐标，跨中文/emoji/长代码的选择复制直接读原文。
- fenced code 语言标签为真实元素并在代码块内上移；代码正文位置保留。
- 图片不加载，HTML 惰性显示，链接经版本化 `ReaderAction` 回调且危险 scheme 拒绝；异步旧结果不覆盖新文档，2 MiB 边界保持在内容层。
- Source gutter 仍为原 0.75rem 加 6px。

## 滚动条（共享核心）

main 的 PR #29 已提供 `src/ui/scroll/`：统一 3 logical px、两端圆、半透明、140ms 可反向淡出的细条，以及只读 `ScrollTarget`（`ScrollHandle` 与真实 `ListState` 适配，拖动开始/结束冻结列表测量）。Reader 直接使用这套共享实现，没有复制第二套样式或通用 scroll 子系统：

- Markdown 纵向条绑定本地 `MarkdownView::list_state()` 的真实 `ListState`（PR #29 的 `ScrollTarget for ListState` 适配）；滚轮、thumb 拖拽、范围更新、长文异步落地均作用于同一个列表。无溢出时 `Bar::render` 不构建元素，也不保留透明拦截区域。
- Source 纵/横条沿用同一 `Bars`/`Bar`/`Style`，绑定 `UniformListScrollHandle.base_handle`，两轴独立、互不抢角落。
- hover 入场、快速反向和拖动移出后释放都由 `Bars`/`Fade` 处理；拖动期间保持可见，结束后回到 hover 状态。

## 未覆盖与待验收

- **真实 Windows 窗口**：鼠标拖选、`Ctrl+C`、Shift+滚轮横向滚动、中文 IME 组合态在源码视图中的行为、emoji 与中文字形回退、多 DPI。测试平台不经过 Windows 原生输入分发，不能替代。
- **锚点滚动**：`#fragment` 只报告目标，不滚动到标题。
- **图片**：仓库模式不允许本地文件/data 图片，图片/HTML 在交给上游文档前被移除；没有网络下载、资产缓存或受控版本化 resolver。
- **Markdown 等价性**：Ely 使用 GFM + 数学扩展；本轮不声称与 github.com 逐字节等价（例如 HTML 表格布局、脚注样式、绝对 URL 重写）。无法支持的语法按 Ely 的现有行为显示或按文本显示，不在本模块另造。
- **接入**：仓库页面的文件选择与正文请求、data 层会话接线还没有。本轮的预览与 fixture 只用于验收，不冒充真实数据。
