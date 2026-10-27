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

## 2026-10-26 第二轮负责人真实窗口反馈

PR #27 保持人工 hold，原 head `25e9e19` 未合并。本轮按反馈处理并记录了未通过项：

**已实现并有测试证据：** 源码 gutter 右侧改为 6px 真实布局 padding，行文本与选择命中仍使用同一 `TextLayout`（`gutter_padding_keeps_selection_coordinates_aligned`，拖选后 Ctrl+C 内容一致）；源码纵/横滚动条绑定 `UniformListScrollHandle.base_handle`，范围来自列表 `max_offset()`，hover 显示、滚轮同步（`source_scrollbars_follow_the_real_list_and_fade_out`）。抽取 `src/ui/scrollbar.rs` 共享 3px/透明度/140ms `Fade`，已合并的 `tab_scrollbar.rs` 保持原实现与参数，原有标签条用例全部通过；`ui/mod.rs` 仅增加模块声明。

**审查修复：** 已恢复 `Fade` 动画期间的 `window.request_animation_frame()`，修复负责人视角下“进入/离开后停在同一透明度”的问题。

**未通过、未宣称解决：** 阅读器滚动条 thumb 拖动在 GPUI 测试平台上没有移动真实偏移（能进入按下，但 `on_drag_move` 未执行），同一共用实现在标签条用例中可用；真实 hover 淡入的测试断言也没有通过。已停止继续试探性改法并移除强制 `snap` 的替代断言，保留范围/滚轮/显示证据，拖动、淡入淡出时序和双轴角落命中交负责人真实窗口验收。

**待裁决：** Markdown inline code 字体/圆角/padding，以及 fenced-code 语言标签位置，都需要 Ely 文档预览暴露真实布局或代码块绘制钩子；现有公开 API 不足。已记录 `githux:arbitration-required:v1`，`arbitration_ever_required=true`/`merge_owner=coordinator`，未改 checkout 或 vendor。

**第二轮审查修复：**

1. 预览/测试的模块树曾用嵌套 `#[path]`，使 `cargo fmt --check` 在 Linux 上解析到错误目录而 CI 失败。现在由 `src/bin_support/reader_modules.rs` 单点声明，所有 `#[path]` 相对该文件解析，两个平台的 fmt/check 一致。
2. 选择拖动原先只挂在行内文本上，拖到短行尾空白或空行就停住。现在按 prepaint 记录的行 bounds 与 `TextLayout` 映射指针，并把列 clamp 到行尾；新增“长行拖到空行后复制”的回归。
3. 两个轴的滚动条共用 owner，垂直拖动可能被水平轨道消费。现在同时比较 `axis`。
4. 行命中映射曾跨帧累积，滚动后旧行可能在相同像素位置胜出。现在帧末用本次 prepaint 的记录整体替换，并新增滚动后映射数量不膨胀的回归。

验证：`cargo fmt --all -- --check`、`cargo check --locked --all-targets`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo test --locked`（368 主程序 + 32 integration）、`cargo build --locked --example reader` 与 `git diff --check` 均通过；视觉、拖动与淡入淡出时序不因这些命令视为验收通过。

## 2026-10-26 第三轮：纠正第二轮的四项实际缺口

负责人再次确认四项视觉反馈仍未达成。逐条处理：

**1. 行号间距（已实现）。** 第二轮把 `.pr_3()`（0.75rem）换成 `px(6.0)`，实际是**变窄**。现在改为在原有间距上叠加：`source::gutter_gap(rem) = rem * 0.75 + 6px`（16px rem 下 18px），仍是真实布局 padding。回归：`gutter_padding_keeps_selection_coordinates_aligned` 用实测字形宽度反推间距并断言 ≥ 原基线；`gutter_keeps_its_gap_for_every_number_width` 在 12,000 行文件滚动采样，1/2/4/5 位行号的文字列不移动。

**2. 源码滚动条拖动（已修复根因，附真实偏移证据）。** 因果：GPUI 的 `on_drag` 在**跨过阈值的第一次移动**里创建拖动并立即 `cx.stop_propagation()`；同一事件的 Capture 阶段先执行，因此那一次捕获监听看不到 `active_drag`。旧测试只发一次移动，所以永远停在第 0 步。不是产品缺陷，也没有测试平台特例。新增 `dragging_the_source_scrollbar_moves_the_real_offset`（五段移动，offset 0 → -18.5 → -37.0 → -47px，并断言 clamp 到 `max_offset`）与 `dragging_the_horizontal_scrollbar_leaves_the_vertical_offset`（x 动、y 不动）。断言未放宽，未用 `snap` 替代。

**3. Markdown 三项（未实现，方案待裁决）。** 真实滚动条、inline code 圆角/代码字体/小 padding、fence 语言标签上移都受 Ely 公开接口限制。已写出 [最小可审查补丁方案](../content-reader-markdown-patch.md)：三处新增入口（文档 scroll handle、fenced-code 渲染钩子透传、inline code 行内序列钩子）、换行与跨段选择的影响、tag+rev 固定与干净 CI 复现、回滚方式，并列出裁决需回答的三个问题。未改 checkout、未 vendor、未升级。

**4. 过期内容修正（已实现）。** PR 正文与文档改为当前 head、模块已声明、`arbitration_ever_required=true`，CI 链接只对应最新 head。

验证见 PR #27 与 `docs/content-reader.md`；视觉与真实窗口交互仍以负责人验收为准。

## 后续

- 接入前置：`ui/mod.rs` 增加模块声明；仓库页面把 `data` 层的正文与会话上下文交给 `ReaderView`，并由集成层实现 `ReaderAction`（外部 URL、同 commit 文件切换）。
- 真实窗口验收未做，`merge_owner=coordinator`、`human_review_required=true / approved=false`：PR 完成后保持 OPEN，由负责人真实验收后再决定合并。
- 若上游后续修好「构造期长文本不绘制」的问题，可去掉 `reset` 变通并删除对应注释。


## 2026-10-26 第四轮：协调裁决后的四项实现

协调裁决已批准最小 Ely 补丁。实际采用根 `[patch]` + `vendor/ely-gpui-component`，基线仍为 `da3fdcaf39f2fa85ce1dd9cad07c3c81cf1dd721`，不修改 Cargo 缓存，不升级版本。fence 标签局部上移 8px；inline code 在真实连续字形排版中使用 mono 字体、圆角浅背景和小 padding，长代码断行，跨代码段拖选复制不加字符；Markdown 滚动条直接绑定真实 `ListState`。Source gutter 保持 0.75rem+6px。

阅读器 41 项集成测试通过，覆盖真实滚动/拖动/无溢出隐藏、长文虚拟化、实际代码字体/几何、窄窗口断行与跨代码段复制，既有链接和 Source 测试也保持通过。完整命令与最新 head、预览 PID 见本轮 PR/交付状态；真实 Windows 最终视觉仍待负责人验收，未自行合并。


### 第四轮 Codex 修复与验证补充

本轮 review 指出的硬换行、重复全量扫描、单字符链接右半区域、文件系统图片和表格对齐均按实际行为修复。额外定位到固定 GPUI `x_for_index` 每次从头扫描字形，原先每字符两次调用导致密集段落平方级排版；改为一次位置索引后，恢复 20,000 span 的真实 resize 预算测试并通过。短段落返回固有宽度，表格左/居中/右列用实测坐标核验；长段落保持换行并跳过屏外绘制。安全策略在 loader 创建前拒绝文件系统/data 图片，覆盖 UNC 和绝对路径，不依赖点击链接回调。

新增真实 hover 淡入淡出与 resize/替换内容的 ListState clamp 测试，不用 snap 代替交互；保留单字符链接右半位置点击测试。预览新增 Inline wrap fixture，仍只复用实际 reader，不接主界面。补丁差异重新生成到 vendor/ely-gpui-component/patches/reader.patch。

全量测试曾有两个网络 bounded 用例在并发负载下失败（Network 而非 ResponseTooLarge），独立模块 5 项重跑通过；不修改无关网络实现，也不把该次全量执行写成通过。最终 CI/审查结果以 PR 当前 head 的证据为准，永久 true/coordinator 和人工验收 hold 保留。

后续审查指出连续代码 span 之间的样式边界会拆成多个绘制 piece，而换行预算只计算整个 code region 的一组 padding。现改为换行与绘制共用有序边界，每个 piece 都计算左右 inset。`adjacent_differently_styled_code_budgets_every_padding_inset` 在 190px 视口核对每个实际背景矩形不越界；reader 49 项通过。

真实窗口补充：在 150% DPI 下先修正截图脚本的 DPI 感知，避免把捕获位图截短误判为布局溢出。已实际核对长 inline 窄窗换行、代码围栏、Source 的 1/2/4/5 位行号和大 Markdown 原生滚轮/右侧细条。首次异步 blocks 采用时增加 ListState reset，真实窗口无需滚轮即可自动显示 Section 0；近 2 MiB fixture 的后台解析约十几秒，仍是性能边界，不宣称即时加载。新增无 window.refresh/输入的首块绘制回归。最终视觉与原生输入由负责人验收。

## 2026-10-27 官方 Ely / GPUI 0.3.10 迁移

同步 main `7ed70cda44c2f3ba36eeb740fd0925f20f5882b8`（本地 merge `ef0c37da0a1ffa2da6d7469e5f0649199bcd07ab`）后，负责人撤销整库/单 crate vendor 批准。已按明确路径删除本 PR 的 1,273 个 `vendor/ely-gpui-component` 跟踪文件，Cargo 只用官方仓库固定 SHA `30c66fe1d5d23db769a6c834b771d054d7f288cf`。reader 实际增加 `ely-documents`/`ely-editor`；`ely-core` 仍 default-features=false、保留 `runtime_shaders`，GPUI 统一 0.3.10。

实际迁移了核心 imports、Source 的 `ScrollEvent`/`ScrollSource::Pointer` 和 `KeyBinding` 初始化；Markdown 改用上游文档异步解析。公开文档不提供图片 resolver，reader 先通过公开 mdast 删除图片/HTML，再检查当前文档实体写入；新增替代旧文档写入的 stale-result 回归。Source 的只读、选择复制、真实双轴滚动、危险链接拒绝和大内容虚拟化继续通过。

迁移后的上游无法提供 fence 标签、inline 圆角代码字体和 3 px fade 滚动条的扩展点；原有验收测试未删除，仍以失败结果保留。未 vendor、fork 或改缓存，未复制 PR #29 核心。详细边界与最小方案见 [Markdown 上游接口](content-reader-markdown-patch.md)。真实窗口最终视觉仍需负责人验收，本 PR 未合并。

## 2026-10-27 负责人授权本地维护 Markdown 组件

负责人明确授权从 Ely 拆出 Markdown 阅读组件、由 Githux 维护改造；“只能等上游接口”的旧限制失效，整库 vendor 禁令不变。已在 `src/ui/reader/markdown/` 提取并重写解析、只读文档、inline 排版、选择复制、代码块和真实 ListState 滚动；官方 Ely 仅保留 `ely-core`，移除 `ely-documents`/`ely-editor`，新增的只有 `pulldown-cmark` 与 `unicode-segmentation`。源映射和 MIT 说明见主题文档。

六项 UNMET 转为真实实现：fence 标签位置、inline code 字体/圆角/padding 与跨段复制、Markdown 真实滚动条、表格/列表/链接布局、异步首屏、密集 20,000 span 性能。密集章节改为一次 cursor 分段；测试由约 60 秒降到预算内。reader 55 项、主程序 390 项全部通过；fmt、locked all-target check、严格 clippy 和预览构建通过。真实窗口已核对 Markdown/Source/窄窗/代码块/滚动；最终视觉仍由负责人验收，PR 未合并。

同步 main 的 PR #29 后，reader 原有过渡细条与旧 `scrollbar.rs`/`tab_scrollbar.rs` 被删除，Markdown 与 Source 均改用 `src/ui/scroll` 的共享 `Bars`/`Bar`/`ScrollTarget`；ListState 仍是唯一真实滚动源。合并后主程序 405 项、reader 67 项全部通过，严格 clippy 通过；共享滚动核心没有复制到 reader，测试/预览边界不变。

后续审查修复：表格单元格与嵌套列表段落的 keyed selection 状态改为完整路径键（未共享状态下复制不会串到别的单元格），长 inline code 换行续行重新计入左 inset，嵌套任务父项勾选状态保存在所在 list 容器。新增表格复制、嵌套任务 marker、代码续行边界回归；主程序 408 项、reader 70 项与严格 clippy 通过。

审查复核又发现嵌套列表 marker 丢失编号/未勾选语义；顶层与嵌套现共用 `item_marker`（勾选、未勾选、有序起始号、圆点），并加入 `nested_lists_keep_ordered_and_unchecked_markers` 解析回归。主程序 409、reader 71 项通过。

审查又指出表格单元格内格式内容未 flush、嵌套容器被空字符串替代、图片 alt 的样式区间残留，以及代码续行 padding。已修复：单元格先 flush 独立的 inline builder；嵌套引用/表格/脚注递归渲染并沿用同一 marker 逻辑；移除图片描述时同步删除其 styles/links；换行代码每个 piece 都预算左 inset。新增单元格链接/粗体、格式图片后 Unicode 文本、嵌套引用表格、marker 单测；主程序 413、reader 75 项通过。

审查指出单个超大列表/表格只算一个顶层 block，子项会在可见时一次性全部构建。`flatten` 现把超过 64 个条目的列表按块切分（起点编号连续）和表格按行切分（表头逐段重复），每段成为独立虚拟行；`a_large_list_splits_into_virtualized_rows` 用 20,000 项验证列表被切分且远处行未构建。主程序再增该项后 414 项、reader 76 项通过。

再一轮审查指出：点击行尾空白会把指针回退到下一行首个字节并误开链接；嵌套标题只渲染纯文本丢失链接。现 `glyph_at` 在字形区域外返回 None（硬换行仍转为等长空格送入单行塑形，字节偏移不变），嵌套标题改用 keyed `ParagraphView`。新增空白点击和引用内链接标题回归；主程序 417、reader 79 项及严格 clippy 通过。

链接可发现性审查也成立：此前链接只记录动作、没有可见样式。本地 inline 排版现把 link ranges 纳入同一字形的样式切分，使用主题 link 色与下划线；命中、选择仍共用同一排版坐标。新增链接范围回归；主程序 418、reader 80 项和严格 clippy 通过。

链接的每一种 run 都扫描整条 link 列表会再次形成平方级开销。`runs()` 现在把排序后的 link ranges 用单调游标前进，与样式事件共用同一次边界遍历；新增 20,000 个链接段落的真实 resize 预算回归。reader 81 项通过。主程序全量运行时曾有一个本地网络 bounded 用例失败，模块单独重跑 5 项通过，按既有基线记录为并发环境偶发，不修改无关网络实现。

## 2026-10-27 文档级选择与行号左距

负责人反馈：README 无法跨不同文字元素拖选，标题↔正文/正文↔标题会停在起始元素，Ctrl+A 只选最后点击的元素，点阅读区空白也只有元素内部才取消选区。根因确认在 `inline.rs`：每个 `ParagraphView` 各自持有 anchor/head 与 Ctrl+A/C 处理。

现在选择所有权移到 `MarkdownView`：新增 `doctext.rs` 在解析后按渲染顺序建立文档文本与单元表，段落只做真实布局到文档偏移的映射并绘制文档选区交集。跨块拖动使用 `on_drag`/`on_drag_move`（GPUI 在全拖动期间跨元素分发），方向由 head 相对 anchor 决定；空白处按下清空整篇选区。`Ctrl+A`/`Ctrl+C` 作用于全模型，包括未实例化的可见块之外内容。单元 key 仍按完整嵌套路径唯一。

Source 行号左侧 padding 由 0.5rem 增至 0.75rem（`gutter_left` 常量），右侧 0.75rem+6px 保持不变，新增 `reader-source-number-*` 选择器以便测试读取真实内距。

验证：`cargo fmt --all -- --check`、locked all-target `check`、`clippy -D warnings`、`cargo test --locked` 主程序 432 项、reader 94 项均通过。新增用例覆盖标题↔正文双向跨块、跨空白与三块、Ctrl+A 含视口外、空白清选区、越界拖动、拖过链接不打开、换文档清选区、超大单列表 Ctrl+A 与虚拟化、行号左内距。真实窗口交互仍待负责人验收。

审查修复（同一轮）：文档模型在单元之间插入分隔符（块之间空行、表格单元格之间 tab），复制不再把 `intront` 拼成 `intronext`；链接查找改为半开区间，单元首字节开始链接时仍可命中；嵌套代码块等非段落单元也会消费自身单元，后续段落不会拿到代码块范围；命中只使用本帧实际布局登记的单元，滚动离开的行不会在旧屏幕位置抢先命中。新增复制分隔、单元首字节链接、嵌套代码+链接段落、滚动往返后命中四个回归；主程序 437、reader 99 通过。

补充：空表格单元格也必须登记为单元，因为两个表格渲染器都按单元格消费单元；跳过空单元格会让后面所有单元格错位（含链接命中）。新增空单元格+链接的模型与渲染回归；主程序 439、reader 101 通过。

再两条审查修复：命中测试改为事件触发时读取当前已布局单元（渲染前快照会漏掉刚滚入的段落，链接点不动）；空白/行尾拖动先按最近段落行的端点解析，再退回整块边界（此前会把换行段落剩余全部选中）。新增“滚入后新段落可点击”和“换行行尾拖动不吞后文”回归；主程序 441、reader 103 通过。

又两条审查修复：围栏代码块现在也注册可选择的真实布局（此前只有 Ctrl+A 能选，鼠标拖动会当成空白并回退到前文）；分块长表格的重复表头只作为呈现元素登记占位单元，不再重复进入复制文本（65 行以上表此前会每 64 行复制一次表头）。新增“代码块可拖动复制且不吞前文”和“80 行表格 Ctrl+A 只含一次表头”回归；主程序 443、reader 105 通过。

再有两条审查修复：围栏代码的语法区间改用游标扫描（此前每个 run 都从头重扫全部语法区间，20,000 个 token 的 fence 在布局时近乎卡死）；表格复制改为按行拼接（单元格之间 tab、行之间换行、表末空行），不再把整张表压成一行。新增 80,000 字节 fence 布局预算和表格分行复制回归；主程序 445、reader 107 通过。

再两条审查修复：表格列对齐改为作用在真实排版行坐标上（此前只给单元格盒子设 `text_right/center`，本地段落一律从 x=0 绘制，右对齐/居中根本看不出来），绘制、选区与命中共享同一偏移；虚拟化表格块的重复表头占位单元不再产生任何文本或分隔符，块间只保留行分隔，不再在 64 行边界插入空行。新增“右/中/左对齐字形落点”几何回归与“两块同表不插空行”模型回归；主程序 446、reader 108 通过。

再一条审查修复：分块表的重复表头虽然只有空单元，却仍用原始 inline 渲染成可命中段落，点击会被按空偏移解析成下一行正文的链接。现在重复表头只作纯文本呈现（仍消费自己的单元保持游标一致），不参与文档偏移命中选择。新增“滚到第二块表头点击不会打开下一行链接”回归；主程序 447、reader 109 通过。

再一条审查修复：表格对齐此前对每个换行行重扫全部 piece，超长换行单元格是二次复杂度；现在按行顺序单次扫描应用位移（新增 `row_shift`），大量换行仍是线性。新增 20,000 个换行词、右对齐单元格的 5 秒布局预算回归；主程序 448、reader 110 通过。

再一条审查修复：按下链接后拖动、再拖回原位置松开，此前仍会打开链接（只比较了最终收拢的选区）。现在拖动一旦开始就清除待激活链接，取消手势不会导航。新增“链接上拖动往返再松开不导航”回归，并确认移除该修复后回归确实失败；主程序 449、reader 111 通过。

再两条审查修复：`next_unit()` 现在直接用已算出的全局单元索引取范围（不再 `units_of(ix).nth(at)` 逐个跳过前序单元），宽表不再做二次扫描，新增 1,200 列表格每格范围独立的回归；Source 的行几何改为在 prepaint 时立即写入 `rows`（此前要等到下一次 render 才替换，滚动中拖动可能用上一帧的行位置），新增“按住拖动期间滚动后继续拖动仍从按下行开始选择”的回归。模拟事件下新旧实现耗时相近，故该条以“当前帧发布”为正确性依据，不宣称已复现旧的错误选中。主程序 451、reader 113 通过。

再两条修复：引用/列表/脚注里的表格现在也走同一套“表尾”处理，嵌套表后接段落时复制不再把最后一个单元格和下一段粘在一起（嵌套块仍归属原顶层块的单元索引）；把几条密集布局测试的固定 5 秒预算改成共享常量 `BUDGET`（20 秒）——这些测试只用于挡住平方级爆炸，CI 共享 runner 上一次实测 5.018s 已误报，20 秒仍能抓住数量级回归。新增嵌套引用/代码归属断言；主程序 452、reader 114 通过。

再一条审查修复：行内 HTML（如 `<kbd>`）在没有段落的紧凑列表项/表格单元格里此前被塞进块级 HTML 缓冲，而这两个上下文永远不会发出 `End(HtmlBlock)`，于是开标签丢失、只剩闭合标签（或整格消失）。现在 `InlineHtml` 走与普通文本相同的落地路径（已有 builder / 单元格 / 新建 builder），块级 `Html` 仍只缓冲到自己的结束事件。新增紧凑列表项与表格单元格内联 HTML 保留测试，并确认移除修复后该测试失败；主程序 453、reader 115 通过。
