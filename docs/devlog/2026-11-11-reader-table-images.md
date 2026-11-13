# 表格单元格图片

- 日期：2026-11-11（Asia/Shanghai）；任务 ID `reader-table-images`；分支 `fix/reader-table-images`，worktree `D:\githux-reader-table-images`，基线 `f9e31a0`。
- 状态：实现与本地验证完成，待 PR 审查与负责人视觉验收。PR 链接补在 PR 记录中。

## 背景与目标

负责人反馈 `opanel-mc/opanel` README 的 Screenshots 表格只显示 `Dashboard` / `Monitor` 等 alt 文本，看不到截图。原因是 AST 的 table cell 只有 `Inline`，没有块列表可以挂图片，`in_cell` 分支刻意把图片降级成 alt 文本。本任务让 Markdown 与 sanitized HTML 表格 cell 内的图片走既有 account/document 作用域的图片生命周期，按 cell 宽度等比缩放，同时保留复制、选区与链接行为。

## 改动与决定

- `markdown/ast.rs`
  - `Inline` 新增 `cell_images: Vec<CellImage>`；`CellImage` 记录 `alt`、`url`、HTML `width`/`height` 提示（`ImageHints`）以及 alt 在 cell 文本中的字节范围。除 table cell 外，这个列表始终为空。
  - 新增 `cell_parts(cell)`：把 cell 拆成按阅读顺序的「文本段 / 图片」序列。`DocText` 与渲染器都遍历这份序列，避免两边各写一套拆分规则。
  - 新增 `Inline::slice(range)`（clamp 并重定位 styles/links）供 cell 文本段使用。
  - Markdown `![alt](url)`：`Building::close` 在 `in_cell` 时记录 `CellImage` 而不是 `Block::Image` note，alt 文本留在原位。
  - HTML `<img src>`：`append_html_element` 在 `in_cell` 时同样记录 `CellImage`（保留 `width`/`height` 提示）；`src` 为空仍按 alt 文本处理。
  - `merge_cell_builder` / `finish_inline` / `trim_inline_end` 都会平移或 clamp `alt_range`，`normalize_cell_images` 在 cell 结束时排序并消除重叠范围。
- `markdown/doctext.rs`
  - `Block::Table` 改为按 `cell_parts` 逐 piece 登记单元：cell 内文本段与图片（alt 文本）各一个单元，只有 cell 的最后一个 piece 带 `\t`/换行分隔符。复制文本与以前逐字节相同（仍是原 cell 文本），但单元数可以大于 1；重复表头也按 piece 数登记空单元，保持渲染游标同步。
- `document.rs`
  - 顶层与嵌套表格共用一个 `table_cell` 帮助函数：无图片 cell 保持原段落路径；有图片 cell 用 `flex_col` 按阅读顺序堆叠文本段与图片，文本段仍是 `ParagraphView`（选区/链接/列对齐不变），图片走新的 `cell_picture`。
  - `cell_picture` 复用 `images::resolve_target`、`ImageBoard::want(ix, ..)` 与 `images::image_element`，因此账户/文档代次、取消、预算、no-store、失败降级全部沿用现有实现；图片按 `ix`（顶层块）登记，落地时重测该块/行。它不实现自己的 placeholder：非 Ready 的可见呈现由共享 presenter 决定（后续按负责人指令统一为完全不绘制）。
  - 只有 Ready 状态不再把 alt 交给 presenter（否则同一 row 的 caption 会重复）；未加载/失败/无 provider 时仍把 alt 传入，保留可读的失败说明和语义。alt 始终在 `DocText` 复制模型里。
  - 图片的绘制区域登记为该单元的真实命中目标（`PictureHit`）：图片没有可命中的字形排版，若不登记，从图片起手的拖选会被当成空白并从文档起点开始。命中后按拖拽方向取该单元范围的端点，选区从图片所在 cell 开始。
  - `[![alt](img)](link)`：解析时把**包住该图片自己的**链接记录在 `CellImage::link` 上，而不是从 `Inline::links` 的字节包含关系反推（空 alt 图片的范围是零长度，无法与相邻链接区分）。渲染据此显示手形光标，点击沿用文档的 press/click 路径（任何拖动、移出窗口、失焦或清空选区都会取消），危险 scheme 仍由 `links::resolve` 拒绝。

## 验证

全部通过 `node scripts/verify.mjs`（共享 Windows Cargo 互斥），记录在 `.agents/task/runs/`：

- `cargo check --locked --all-targets`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo fmt --all -- --check` 通过。
- `cargo test --locked --bin githux -- ui::reader` 通过（200 passed）。
- `cargo test --locked --test reader` 通过。
- `cargo test --locked` 全量通过：1249 / 103 / 589 / 300 / 265 / 794 / 890 / 44，0 failed（run `ea9828c5`）。
- 首轮审查修复后再跑全量（run `23225418`）：1253 passed，唯一失败是既有抖动的 `app::pages::home::tests::online_cache_reopens_and_offline_reads_the_same_home_feed_and_older_page`（`left: ["first"]` vs `right: ["first","second"]`），单独重跑通过；另一次更早全量运行（`1019eb86`）只有 `app::pages::home::tests::authentication_failure_cancellation_and_uncached_pages_are_not_historical_success`（`state=Loading`）失败，同样单独重跑通过。两者都在 `app/pages`（cache-first-paint 区域），不在本任务改动范围，按 L002 记为既有负载敏感抖动；最终 head 的完整检查以 PR CI 为准。
- 审查修复后新增/更新的测试（图片命中目标、链接归属、拖选范围、空 alt 链接、无 provider、复制、同 cell 文字链接）逐个针对性运行通过。

## 第二轮审查修复（同一 PR，head `8738cbb` 的反馈）

- **混合标记语法的链接归属**：`CellImage::link` 只在同一种语法栈里查找，`[<img>](guide.md)`（Markdown 链接包 HTML 图片）和 `<a href>![Shot](shot.png)</a>`（HTML 链接包 Markdown 图片）会丢掉链接。现在 Markdown 图片关闭时同时检查 Markdown `open` 与 HTML `opens`；HTML `<img>` 走 `push_html` 时把当前仍打开的 Markdown 链接传给新记录的 cell 图片（HTML `<a>` 更内层时优先）。新增 `a_cell_picture_keeps_its_link_across_markup_syntaxes`。
- **列对齐落到真实图片框**：共享 presenter 的 Ready 外层是 `w_full`，只有 `justify_*` 无法移动窄图。`cell_picture` 返回 presenter 的显示宽度（intrinsic 或更小的 HTML 提示，绝不放大），cell 把对齐行约束到该宽度再应用列对齐；frame 的真实尺寸/比例仍由 presenter 负责，PR82 把外层改成本征宽度后同样成立。新增 `a_narrow_cell_picture_follows_the_column_alignment`，断言左/中/右列图片分别贴内左、居中、贴内右。
- 修复后重新通过 `cargo fmt --all -- --check`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo test --locked --bin githux -- ui::reader`、`cargo test --locked --test reader`。全量 `cargo test --locked`（run `151a83bb`）为 1255 passed、1 failed：唯一失败是 `app::pages::profile::tests::partial_pins_and_calendar_and_section_miss_preserve_distinct_states`（`profile page did not reach expected state`），单独重跑通过；与前面两次 home 抖动同属 `app/pages`（cache-first-paint 区域）的既有负载敏感抖动，按 L002 记录，最终以 PR CI 的全量结果为准。

新增回归：

- `ast::tests`：`a_markdown_table_cell_keeps_its_picture_at_the_alt_text`、`pictures_and_text_keep_their_cell_order`、`a_cell_picture_records_its_own_enclosing_link`（空 alt 图片保留自己的链接而不是相邻链接）、`an_html_picture_in_a_cell_keeps_its_size_hints`、`an_html_table_cell_picture_uses_the_same_inline_model`、`a_picture_without_alt_text_still_marks_its_place_in_a_cell`，并扩展 `an_image_in_a_table_cell_keeps_its_alt_text`。
- `doctext::tests`：`a_picture_cell_copies_its_alt_text_in_place`（复制文本不变）、`a_picture_cell_registers_one_unit_per_piece`（单元数、顺序、图片后链接仍可解析）。
- `ui::reader::tests`：
  - `screenshot_table_cells_draw_their_pictures_and_keep_the_caption_rows`：以 OPanel README 形态的两列 Screenshots 表格 + 测试内确定性 PNG，断言固定 commit 的仓库相对路径、宽/高图各自比例、图片在 cell 内不越界、caption 行在图片行下方、窄窗口缩小且不拉伸、复制文本保留 alt 与 caption 行。
  - `a_delayed_cell_picture_remeasures_its_row_after_it_lands`：图片落地后 cell 行与顶层块增高、`top_block` 不变（不丢滚动锚点）。
  - `a_picture_wrapped_in_a_link_in_a_cell_opens_the_link`：图片链接通过 `ReaderAction::Link` 报告外链；`a_drag_that_starts_on_a_linked_cell_picture_does_not_open_it` 证明拖选经过图片不会误导航；`dragging_from_a_cell_picture_selects_from_the_picture_not_the_document_start` 证明从图片起手的拖选不会选中整篇文档；`an_empty_alt_cell_picture_opens_its_own_link_not_the_neighbouring_one` 覆盖空 alt 图片与相邻链接的归属。
  - `a_text_link_beside_a_cell_picture_still_opens`：同一 cell 内的文字链接仍按原偏移解析到固定 commit。
  - `copying_a_table_with_pictures_keeps_the_cell_text_and_tabs`：Ctrl+A/C 在有图片的表格里仍以 alt 文本 + tab/换行复制。
  - `a_table_cell_picture_without_a_provider_stays_honest`：没有 provider 时保持静默（不绘制占位），alt 仍在复制文本里（后续按负责人指令改为与所有非 Ready 状态一致）。
  - `an_html_table_cell_picture_uses_the_same_image_lifecycle`：sanitized HTML 表格 cell 图片走同一解析与 provider 路径。

## 与未合并 SVG 任务（PR82）的集成边界

- 表格图片只调用 `images::image_element`，没有修改 `src/ui/reader/images.rs`、`src/content/images/**`、network/data 图片适配器或 Cargo。PR82 的通用 pending presenter 改动合入后，表格图片自动跟随；本任务没有复制或另造 pending 呈现。
- 当前 main 的 presenter 在 Ready 时会把 alt 作为图注画出，Pending 时仍画 140px 占位框。表格代码不依赖这两点：Ready 传空 alt 避免与 README 自己的 caption 行重复，Pending/失败仍传 alt。PR82 合入后这两处表现会随之变化，表格侧无需改动。
- PR82 的 `picture_unit` 额外用 `on_children_prepainted` 上报 `painted(key)`（用于有限 SVG 动画起播）。本任务的 `cell_picture` 不做这层包装，因此表格内的 SVG 动画在集成后不会起播。已在 `.agents/task/cross-task-findings.md` 记录精确的集成请求：由图片任务把「可见性上报 + 通用 presenter」抽成可复用函数，或让表格直接调用它。

## 2026-11-12 同步最新 main（reader-svg #82 / scroll geometry #84 / workflow #87）

- 以普通 merge 合入已发布的 `origin/main` `a2c5ade`（base 仍为 `f9e31a0`），保留原分支已审查历史，不 cherry-pick 兄弟 WIP、不 force-push。
- 冲突处理：`ast.rs` 保留 main 的 `ImageRow`/`Centered`/`headings` 与 `append_picture_notes`，并保留本任务的 `CellImage`/`cell_parts`；`document.rs` 同时保留本任务的 `table_cell`/`cell_picture` 与 main 的 `picture_row`/`pending_image`/`only_pending_images`；`docs/*` 把「表格内图片不加载」等旧边界改写为已实现，并保留 SVG/品质/静默加载的新边界；`LESSONS` 保留 main 的 L023（SVG 有效字节不等于正确显示），本任务的条目改为 L024。
- 表格侧适配 main 的实际图片 API：
  - **pending 静默**：main 的 `ImageState::Loading` 是 0×0 且 `want` 在无 provider 时返回 Loading。`cell_picture` 在 pending 时返回 `None`，cell 不生成子元素（不占行、不显示 alt/占位），但 `want` 已登记需求；provider 之后附加时自动重新请求，落地时按顶层块重测。`picture_hits` 只登记已绘制图片。
  - **SVG 可见动画**：把 main `picture_unit` 里的可见性上报抽成 `images::painted_when_visible`（共享 presenter 旁的小 helper），普通图片和表格图片都用它；表格内可见的 SVG 会经真实可见交集上报 `painted`，起播有限时间线，动画结束后 pump 结清。
  - **逻辑尺寸/DPI**：删除本任务上一版的 `ready_picture_width` 复算。main 的 Ready presenter 已按 `ImageState` 的逻辑 `width`/`height` 收缩并 `max_w_full`，表格列对齐直接作用于 presenter 的根节点；SVG 最终 PNG 可以超采样，但布局宽度仍是作者逻辑尺寸。窗口 density 变化由 main 的 scope/重请求路径处理，表格图片跟随。
  - `ImageRow`/`Centered` 与表格互不干扰；表格仍注册每 piece 一个 `DocText` 单元、真实命中目标、`CellImage::link` 链接归属与列对齐。
- 复用并扩展了预览/测试装配：`reader_images` fixture 新增 `Scene::Table`（OPanel 形态两列表格：普通栅格、宽图、延迟图、SVG、caption 行），`reader_images_view_tests` 用真实 provider/pipeline 断言 cell 内逻辑宽度、宽图比例、SVG 逻辑比例、caption 行与复制；`ui::reader` 新增表格内 SVG 逻辑尺寸 + 可见有限动画起播/结束、pending 静默后附加 provider 仍加载的回归，并把无 provider 的旧断言改为静默语义。
- 验证（均为当前工作树、共享 build slot；`TEMP`/`TMP` 指向本 worktree 的 `.agents/task/tmp`，进程内 `CARGO_PROFILE_DEV_DEBUG=0`、`CARGO_PROFILE_TEST_DEBUG=0`、`CARGO_INCREMENTAL=0`）：
  - `cargo check --locked --all-targets`、`cargo fmt --all -- --check`、`cargo clippy --locked --all-targets -- -D warnings` 通过。
  - `cargo test --locked --bin githux -- ui::reader` 通过。
  - `cargo test --locked --test reader --test reader_images --test repository --test scroll_viewports`：reader/reader_images/repository 全部通过；scroll_viewports 中只有既有负载敏感抖动 `app::pages::home::tests::append_keeps_a_delayed_first_page_description_and_refresh_rejects_old_batches`，单独重跑通过，属 cache-first-paint 区域、与本改动无关（按 L002）。
  - `cargo build --locked` 与 `cargo build --locked --example reader_images` 通过；清洁 head 上的预览产物与收据记录在 `.agents/task/preview/`。
- 未重复运行无关的整套测试；新的图片/表格组合行为由上述针对性目标与 CI 覆盖。

## 2026-11-12 非 Ready 图片完全静默（同一 PR 返工）

负责人此前“保留失败诊断”的指令已被取代：**只要图片不能显示，就什么都不画**。这不是验收，而是同一 PR 的行为返工，仍在原 worktree / 分支 / PR #86 上完成。

- 共享 presenter `images::image_element` 只保留一条绘制分支：`Ready` 画真实像素（alpha、原生/作者请求尺寸、逻辑 DPI 宽度、列对齐、链接、有限 SVG 动画全部不变）；`Loading`、`Failed/Refused`、`Unavailable`、`Released` 统一画零尺寸空元素。原先的 `note` 闭包（`px_3`/`py_2`/`sunken` 背景 + alt/诊断文案）整体删除，`PictureStyle` 也随之删掉（不再需要主题颜色、小字号与圆角）。
- `markdown/document.rs`
  - `picture_unit` 改为返回 `Option<AnyElement>`，非 Ready 直接返回 `None`；`picture_row` 用 `?` 把不可见图片从 flex 行里彻底移除，因此不会留下 gap。
  - `cell_picture` 同样在非 Ready 时返回 `None`（此前只对 pending 这样做），失败/拒绝/不可用/已释放的 cell 图片都不生成子元素、不占行、不显示 alt；表头、边框与作者写的 caption 行（`**Dashboard**` 等）不受影响。
  - `pending_image` + `only_pending_images` 改为 `picture_visible` + `only_hidden_images`：判定依据从“是否 loading”变成“是否 Ready”，全部不可见的图片块高度归零、不加图片内边距。`ImageBoard` 新增 `is_ready(&ImageKey)`（只读，不登记需求），并删除了因此失去调用方的 `is_loading`、`attached()`。
  - 调试选择器 `markdown-picture-note-{ix}` 更名为 `markdown-picture-{ix}`，只在真正绘制时存在，所以“选择器不存在”本身就是“没有预留空间”的证据；cell 图片同理使用 `markdown-{cell}-picture-{part}`。`frame` 选择器保持 `markdown-picture-frame-{ix}` / `…-picture-{part}-frame`。
- 文档：`docs/reader-images.md`、`docs/content-reader.md` 把“失败/拒绝仍显示诊断”“未请求/加载中折叠”等表述统一改成“非 Ready 一律不绘制”，并说明失败原因只作为内部状态（测试/调度），alt 仍留在复制模型里。
- 回归：
  - `ui::reader::tests::a_missing_or_refused_picture_draws_nothing_at_all`（原 `…shows_an_honest_placeholder`）：失败/拒绝/不支持格式一律无 picture 选择器、无 frame，正文照常，alt 仍可复制。
  - `ui::reader::tests::a_refused_or_failed_cell_picture_draws_nothing`：cell 内的 `file:` 拒绝与真实 `NotFound` 都不画元素，cell 只保留文字高度，表头/边框/图注行不变。
  - `ui::reader::tests::a_released_cell_picture_disappears_instead_of_reserving_its_box`：`max_ready_images: 1` 触发真实淘汰后，被释放的图片不再占位，只有保留的那一张绘制，作者图注行仍在。
  - `ui::reader::tests::a_repository_file_picture_draws_nothing_without_loading`（原 `…draws_a_note_without_loading`）：`file:` 图标不绘制，源地址与 alt 仍可读。
  - `bin_support` 图片视图测试改用 `markdown-picture-{ix}`：`a_missing_or_refused_picture_draws_nothing_at_all` 断言所有失败块都不绘制，`a_reader_before_provider_attachment_keeps_pending_pictures_collapsed` 与 `quality_silent_pending_*` 断言 pending 既无 picture 也无 frame 选择器。
- 验证（共享 build slot，进程内 `TEMP`/`TMP` 指向本 worktree 的 `.agents/task/tmp`，`CARGO_PROFILE_DEV_DEBUG=0`、`CARGO_PROFILE_TEST_DEBUG=0`、`CARGO_INCREMENTAL=0`）：`cargo check --locked --all-targets`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo fmt --all -- --check`、`cargo test --locked --bin githux -- ui::reader`（220 passed）、`cargo test --locked --test reader --test reader_images --test repository` 全部通过。
- 与图片性能任务的边界：本任务只改呈现层（`image_element`、`picture_unit`/`picture_row`/`cell_picture`、`only_hidden_images`、`is_ready`）。`ImageBoard` 的调度、预算、淘汰、解码与 provider 仍归性能任务；双方各自只写自己的 worktree，接口若再变以本条与 `.agents/task/reader-performance-handoff.md` 记录为准。

## 后续

- 需要负责人原生窗口视觉验收（真实截图表格、窄窗口、缩放比例）。
- 表格内的图片与文本按块堆叠（各占一行），不是文字行内原位置；与段落内联图片的既有取舍一致，已在主题文档写明。
- 未新增表格图片的端到端 fixture 场景到 `reader-images` 预览（该装配与 PR82 重叠）；本任务的布局/完成回归使用 `ui::reader` 测试内的确定性 provider。

