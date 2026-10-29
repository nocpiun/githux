# 可复用只读阅读器

更新：2026-10-28（公共 Motion 接线）。R3 的 UI 切片，位于 `src/ui/reader/**`、独立预览 `examples/reader.rs` 和测试入口 `tests/reader.rs`。它消费已有的 [`content::PreparedContent`](../src/content/mod.rs) 与 `domain::blob::BlobIdentity`，把「已准备好的正文」呈现成本模块自行维护的 Markdown 阅读视图或只读源码视图；不读网络、不读凭据、不落盘、不下载图片。

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

- `PreparedBody::Utf8 { text, .. }` → `DocumentKind::Markdown` 走 `src/ui/reader/markdown/` 内的本地组件（解析、布局、选择、滚动都由本模块持有）；`DocumentKind::Source { language }` 走 [`SourceView`](../src/ui/reader/source.rs)。`DocumentKind::for_path` 只按文件名判断 README / `*.md` / `*.markdown` / `*.mdx`，集成层可覆盖。
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

原始 HTML 不再按文本显示：注释被丢弃，常见容器、行内、链接、图片与表格标签映射为与 Markdown 相同的块和样式，`<img>`/`<a>` 继续走既有图片与链接安全边界。支持子集、实体解码与明确未支持项见 [HTML 子集](#html-子集2026-10-29)。

## 只读源码

先核对了 Ely 的 `CodeEditor`（`src/editor/state.rs`、`view.rs`、`line.rs`）：它有行号、选区、`Ctrl+C`、`uniform_list` 纵向虚拟化，`read_only()` 会拒绝 `insert`/`paste`/`cut`/IME `replace_and_mark_text_in_range`。**但它仍有本轮不可接受的边界**：只读时顶部永远画一条带「Make editable」按钮的 `ReadOnlyBanner`（点击即解锁编辑），且 `overflow_hidden` 没有横向滚动，长行被裁掉。因此本模块不直接复用它，而是复用它的好部分——Ely 公开的 `code_highlights`、代码字体、gpui 的 `uniform_list` ——并删除全部输入路径：

- 没有文本输入处理器、没有 IME 处理器、没有编辑 action，因此键入、粘贴、删除都无法修改文档。测试用 `simulate_input("hello 世界 🌍")` 和 `backspace delete enter tab` 对比全文前后一致。
- 行号右对齐到实际行数位数，等宽字体取主题的 `mono_family`。行号 gutter 右侧间距是**原有 0.75rem 再增加 6px**（`source::gutter_gap`，16px rem 下约 18px），用真实布局 padding 实现、不是字符串空格；行文本与选择命中共用同一 `TextLayout` 起点，1/2/4/5 位行数的文字列不随之移动。
- 纵向/横向滚动条绑定 `UniformListScrollHandle.base_handle`（列表真实测量与滚动的那个句柄），不复用外层空容器 offset。范围直接来自列表的 `max_offset()`，不构建全量行。条使用与 workspace tabs 相同的 3px、20%/38% 透明度与 140ms `Fade` 状态机，见 [共享滚动条](#共享滚动条与未完成项)。
- 语法着色走 Ely 公开的 `code_highlights`（通用代码着色）。`language` 只是标签：Ely 把 `syntax_of` / `code_highlights_with` 保持为 crate 私有，按语言逐 fence 选语法会需要本地复制语法表或改上游，因此本轮明确只提供通用着色，不声称逐语言准确。
- 纵向用 `uniform_list`；横向用 `ListHorizontalSizingBehavior::Unconstrained` 让内容宽度可以超过视口。初始按字节长度取 8 个候选实测，这只是启动探测；每个可见行在 prepaint 时上报实际 `StyledText` 宽度，发现更宽的行便扩展横向范围。屏外行滚入视口后同样参与，避免中文/ASCII/emoji 混排时漏掉字节更短但视觉更宽的行，也避免预先排版所有行。选中背景在同一次 `StyledText` 布局里切分，不用第二套宽度估算；语法区间与选区边界顺序合并，复杂度为线性，不为每段重新扫描全部语法区间。
- 选择：左键按下/拖动扩展选区，`Shift` 扩展、`Ctrl+A` 全选、`Ctrl+C` 写入剪贴板。`Ctrl+A`/`Ctrl+C` 通过 `on_key_down` 检查 `secondary()`，不依赖焦点在某个输入控件上。

## 文档级选择

选区属于整个 Markdown 文档，不属于某一段：`DocText` 按渲染顺序登记每个文本单元（段落、标题、列表项、引用、表格单元格、围栏代码块、数学/图片替代文本或 HTML 文本）及其文档偏移、链接和顶层块映射，`MarkdownView` 只持有一份 anchor/head。指针经段落自己的真实排版换算成本地字节，再加单元起点得到文档偏移，因此可以跨标题/正文/列表/引用/代码/表格正反向连续拖选，拖过链接只扩展选区、不触发打开。

- `Ctrl+A` 覆盖整个模型，包含尚未渲染的虚拟化块；`Ctrl+C` 按阅读顺序拼接，块之间留空行、表格单元格之间留 tab，不会把相邻段落粘成一个词。
- 围栏代码块与正文一样注册可选择的真实布局并绘制同一份选区；它不再是只能靠 `Ctrl+A` 选中的死区。
- 超长列表/表格会被切成多个虚拟化块，每块重复绘制的表头只是呈现元素：它占一个空单元以保持单元游标同步，但不进入复制文本。
- 阅读区空白（含表格外、块之间）按下会清空选区；在视口外松开、滚动、切换文档或异步结果替换内容都会结束拖动并丢弃旧选区，不回填到新文档。

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
- 表格排版与正文同源：表头行与表体共用正文的 `body` 字号（`TextSize::Base` 换算）和 `body * 1.6` 行高，在表格公共父容器上显式设置，单元格的 shaped 文本、选区、链接与换行高度都按这一尺寸测量。顶层表格、引用/列表/脚注内嵌套表格，以及长表按 `LAZY_CHILDREN` 分块后重复绘制的表头都走同一层，所以字号不随父容器或分块变化。表头仍保留 `MEDIUM` 字重与 `fg_muted` 颜色层级，单元格 padding、边框、列对齐与 `flex_1` 宽度策略不变；单元格内的 inline code 仍用代码字体，但字号与该单元格正文一致。
- 图片不加载（加载接线属于 reader-images 任务），HTML 规范化后走同一 `Block`/`Inline`，链接经版本化 `ReaderAction` 回调且危险 scheme 拒绝；异步旧结果不覆盖新文档，2 MiB 边界保持在内容层。
- Source gutter 仍为原 0.75rem 加 6px。

## HTML 子集（2026-10-29）

README 中常见的原始 HTML 由本地 `src/ui/reader/markdown/html.rs` 规范化后交给同一套 `Block`/`Inline` 渲染，不再作为惰性文本显示。规则按 WHATWG HTML 的语法/分词章节实现，不引入 HTML 解析依赖；解析只产生节点树，URL 从不在这里解析。

- 注释（`<!-- ... -->`，含跨行、行内）与 `<!doctype>`、处理指令一律丢弃，不进入可见正文、文档模型或复制文本。
- 容器与结构：`div`/`section`/`article`/`p`/`center`/`details`/`summary`/`figure` 等展开为段落；`h1`–`h6` 成为标题并进入导航索引；`ul`/`ol`/`li` 成为真实列表；`blockquote`、`hr`、`br` 分别成为引用、分隔线、换行。
- 行内语义：`strong`/`b`、`em`/`i`、`del`/`s`、`code`/`kbd`/`samp`、`a href`、`img src` 映射为与 Markdown 相同的样式、链接和图片块。`<img>` 只产生 `Block::Image`，URL 仍由 `links::resolve` 与图片管线判定。
- 表格：`table`/`thead`/`tbody`/`tr`/`th`/`td` 转成 `Block::Table`；首行全为 `th` 时作为表头，否则第一行提升为表头；`align` 属性或 `style="text-align:..."` 决定列对齐；单元格内的块级内容按行内文本展平。
- 代码：`pre`/`code` 成为围栏代码块（`class="language-x"` 取语言）；Markdown 行内代码与围栏代码中的 HTML 原样保留，因为它们是 `Code`/`CodeBlock` 事件而非 HTML 事件。
- 安全：`script`/`style`/`template`/`iframe`/`object`/`embed`/`head` 等整段丢弃，事件处理属性只被忽略，绝不回写原始 HTML；实体按常见命名引用与数值引用解码，未知或畸形实体保留字面文本。
- 明确不支持：`align` 等布局属性的视觉居中/对齐（内容保留、布局忽略）；`colspan`/`rowspan`；SVG/MathML 与 CSS 布局；`<details>` 的折叠交互（默认全部展开）；未闭合标签按容错规则丢弃或展开。复杂嵌套或表格跨度会退化为内容保留的近似排版。

## 滚动条（共享核心）

main 的 PR #29 已提供 `src/ui/scroll/`：统一 3 logical px、两端圆、半透明、140ms 可反向淡出的细条，以及只读 `ScrollTarget`（`ScrollHandle` 与真实 `ListState` 适配，拖动开始/结束冻结列表测量）。Reader 直接使用这套共享实现，没有复制第二套样式或通用 scroll 子系统：

- Markdown 纵向条绑定本地 `MarkdownView::list_state()` 的真实 `ListState`（PR #29 的 `ScrollTarget for ListState` 适配）；滚轮、thumb 拖拽、范围更新、长文异步落地均作用于同一个列表。无溢出时 `Bar::render` 不构建元素，也不保留透明拦截区域。
- Source 纵/横条沿用同一 `Bars`/`Bar`/`Style`，绑定包装真实 `UniformListScrollHandle.base_handle` 的持久 `Motion<ScrollHandle>`，两轴独立、互不抢角落；Markdown 绑定自身真实 `ListState` 的持久 `Motion<ListState>`。wheel 与 Bar 共享实例，离散输入约 130ms 缓动，precise Pointer / Touch / reduced-motion 直接写真实目标，不再额外运行列表内建 wheel。
- Source 保留原生 x/y 双轴，Shift 仅在缺少原生 x 时映射 y；Markdown 仅纵向，不将水平输入变成纵向。现有代码块/表格是换行布局，没有内层横向 ScrollHandle，本轮保留该布局、字号与选择模型。
- hover 入场、快速反向和拖动移出后释放都由 `Bars`/`Fade` 处理；140ms fade 与 wheel 动画分离，拖动期间保持可见，结束后回到 hover 状态。
- wheel tick 与输入使用 owner 的同一时钟，只有动画未完成才请求帧。thumb drag、程序 offset 写入、选择按下、reset/adopt、正文替换、Esc、窗口失活/隐藏、移出布局和 dispose 均取消适当目标与 Bars；真实 ListState 的测量/drag hooks 没有替换或 `measure_all`。
- 失活/隐藏/取消只结束未完成的拖动与待激活链接，保留完成的文本选区供重新激活后复制；正文替换与显式空白取消仍清空选区。
- 集成宿主在账户上下文失效而仍保留 reader 时调用新增 `ReaderView::cancel_interactions(cx)`；既有构造、show、SourceView、PreparedContent 接口保持兼容。宿主改变正文仍走 show 或换实体，网络/账户状态不进入 reader。本轮实现与验证见 [reader-motion 日志](devlog/2026-10-28-reader-motion.md)。

## 未覆盖与待验收

- **真实 Windows 窗口**：鼠标拖选、`Ctrl+C`、Shift+滚轮横向滚动、中文 IME 组合态在源码视图中的行为、emoji 与中文字形回退、多 DPI。测试平台不经过 Windows 原生输入分发，不能替代。
- **锚点滚动**：`#fragment` 只报告目标，不滚动到标题。
- **图片**：仓库模式不允许本地文件/data 图片，图片 URL 由既有管线判定；没有网络下载、资产缓存或受控版本化 resolver。HTML 的 `img`/`a` 与 Markdown 共用同一安全边界，不新增读取路径。
- **Markdown 等价性**：Ely 使用 GFM + 数学扩展；本轮不声称与 github.com 逐字节等价（例如 HTML 的 CSS 布局、`colspan`/`rowspan`、脚注样式、绝对 URL 重写）。无法支持的语法按文本或内容保留的近似排版显示，不在本模块另造。
- **接入**：仓库页面的文件选择与正文请求、data 层会话接线还没有。本轮的预览与 fixture 只用于验收，不冒充真实数据。


## Repository 专用正文留白配置（2026-10-28）

`ReaderView::set_markdown_layout(MarkdownLayout::Repository, cx)` 是小型宿主配置；默认 `Default` 保留原有外层px_2/py_3，Profile/Home/reader example及源码默认表现不变。repository在reader构造时设置一次，其他Markdown文件同样沿用；相同配置重复设置no-op，不重建ReaderView/MarkdownView、不reset正文/选区/滚动。`show`替换正文时保留宿主配置。source/非文本状态不应用该布局，不改行号padding/字号。

Repository模式下Markdown根无固定上下padding，真实ListState视口从标题栏下缘延伸至底部可用区域；水平由原0.5rem增加4px，每侧只加一层。原0.75rem量级上下内容inset加入真实首块/末块的布局padding，在item测量总高度内滚动；不添加假AST/文字/列表项，不改变block/selection索引。短文末尾也保留这份真实内容布局空间。

**为何不用list.py_3**：锁定GPUI的native wheel及set_offset会考虑list padding，但公开`max_offset_for_scrollbar()`忽略它；共享Motion/Bar据较小reach clamp会截掉底部inset。这一中间实现已被实际反馈否定，现改为首尾真实块测量，wheel/thumb/程序跳转共同看到包含inset的item总高度。`scroll_to_end()`临时使用item_count末尾锚点；repository模式在其反向惰性测量前不按旧reach clamp，待真实list布局后继续Motion tick。未改共享scroll或依赖缓存。

回归验证全高viewport、AST块数不变、重复layout不丢选区、上下空白点击清选、段落/列表/代码/表格末块各由wheel和同一thumb target到底，正文bounds下方仍有inset，以及source实体/gutter不变。原生长README的段落/列表/表格wheel末尾截图已实际查看；最终code在实际HWND投递native wheel及mouse drag消息后取帧，代码容器下方到语言条之间有明确留白；这是Windows消息驱动验证，实体设备thumb手感仍受前台/Access denied限制，未冒充负责人原生验收。
