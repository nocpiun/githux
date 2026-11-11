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
  - `cell_picture` 复用 `images::resolve_target`、`ImageBoard::want(ix, ..)` 与 `images::image_element`，因此账户/文档代次、取消、预算、no-store、失败降级全部沿用现有实现；图片按 `ix`（顶层块）登记，落地时重测该块/行。它不实现自己的 placeholder：pending/failed/unavailable 的可见呈现仍是共享 presenter 的决定。
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

新增回归：

- `ast::tests`：`a_markdown_table_cell_keeps_its_picture_at_the_alt_text`、`pictures_and_text_keep_their_cell_order`、`a_cell_picture_records_its_own_enclosing_link`（空 alt 图片保留自己的链接而不是相邻链接）、`an_html_picture_in_a_cell_keeps_its_size_hints`、`an_html_table_cell_picture_uses_the_same_inline_model`、`a_picture_without_alt_text_still_marks_its_place_in_a_cell`，并扩展 `an_image_in_a_table_cell_keeps_its_alt_text`。
- `doctext::tests`：`a_picture_cell_copies_its_alt_text_in_place`（复制文本不变）、`a_picture_cell_registers_one_unit_per_piece`（单元数、顺序、图片后链接仍可解析）。
- `ui::reader::tests`：
  - `screenshot_table_cells_draw_their_pictures_and_keep_the_caption_rows`：以 OPanel README 形态的两列 Screenshots 表格 + 测试内确定性 PNG，断言固定 commit 的仓库相对路径、宽/高图各自比例、图片在 cell 内不越界、caption 行在图片行下方、窄窗口缩小且不拉伸、复制文本保留 alt 与 caption 行。
  - `a_delayed_cell_picture_remeasures_its_row_after_it_lands`：图片落地后 cell 行与顶层块增高、`top_block` 不变（不丢滚动锚点）。
  - `a_picture_wrapped_in_a_link_in_a_cell_opens_the_link`：图片链接通过 `ReaderAction::Link` 报告外链；`a_drag_that_starts_on_a_linked_cell_picture_does_not_open_it` 证明拖选经过图片不会误导航；`dragging_from_a_cell_picture_selects_from_the_picture_not_the_document_start` 证明从图片起手的拖选不会选中整篇文档；`an_empty_alt_cell_picture_opens_its_own_link_not_the_neighbouring_one` 覆盖空 alt 图片与相邻链接的归属。
  - `a_text_link_beside_a_cell_picture_still_opens`：同一 cell 内的文字链接仍按原偏移解析到固定 commit。
  - `copying_a_table_with_pictures_keeps_the_cell_text_and_tabs`：Ctrl+A/C 在有图片的表格里仍以 alt 文本 + tab/换行复制。
  - `a_table_cell_picture_without_a_provider_stays_honest`：没有 provider 时仍走共享的诚实占位，alt 仍在复制文本里。
  - `an_html_table_cell_picture_uses_the_same_image_lifecycle`：sanitized HTML 表格 cell 图片走同一解析与 provider 路径。

## 与未合并 SVG 任务（PR82）的集成边界

- 表格图片只调用 `images::image_element`，没有修改 `src/ui/reader/images.rs`、`src/content/images/**`、network/data 图片适配器或 Cargo。PR82 的通用 pending presenter 改动合入后，表格图片自动跟随；本任务没有复制或另造 pending 呈现。
- 当前 main 的 presenter 在 Ready 时会把 alt 作为图注画出，Pending 时仍画 140px 占位框。表格代码不依赖这两点：Ready 传空 alt 避免与 README 自己的 caption 行重复，Pending/失败仍传 alt。PR82 合入后这两处表现会随之变化，表格侧无需改动。
- PR82 的 `picture_unit` 额外用 `on_children_prepainted` 上报 `painted(key)`（用于有限 SVG 动画起播）。本任务的 `cell_picture` 不做这层包装，因此表格内的 SVG 动画在集成后不会起播。已在 `.agents/task/cross-task-findings.md` 记录精确的集成请求：由图片任务把「可见性上报 + 通用 presenter」抽成可复用函数，或让表格直接调用它。

## 后续

- 需要负责人原生窗口视觉验收（真实截图表格、窄窗口、缩放比例）。
- 表格内的图片与文本按块堆叠（各占一行），不是文字行内原位置；与段落内联图片的既有取舍一致，已在主题文档写明。
- 未新增表格图片的端到端 fixture 场景到 `reader-images` 预览（该装配与 PR82 重叠）；本任务的布局/完成回归使用 `ui::reader` 测试内的确定性 provider。
