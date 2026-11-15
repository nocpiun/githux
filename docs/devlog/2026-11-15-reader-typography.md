# Reader typography, callouts and link cursor（2026-11-15）

任务：`reader-typography`；worktree `D:\githux-reader-typography`，分支 `fix/reader-typography`，提交基线 `812e5fc6`（含 #86/#88 的表格图片与原图性能）。范围仅 `src/ui/reader` 的 Markdown 解析/呈现/命中，以及对应的 fixture 与测试；并行任务拥有的链接跳转、侧栏与图片管线未改动。

## 目标与基线

负责人反馈三件事：callout 全部灰、Markdown 的读写节奏与 GitHub 差了很远、链接没有指针光标。先用测试平台量了改动前的真实几何（run `4ce7eed8`，760px 宽、正文 13px）：

- 单行 bullet 的列表行距 34px（文字行盒 21px + 13px 间隙），嵌套列表行距 30px；
- `h1…h4` 为 2.25/1.75/1.5/1.25em，标题只用 `pt_2`（8px）上留白，没有下留白；
- 每个顶层块统一 `py_1`，列表/引用再叠加 `gap_2`/`gap_1`，所以段间距与列表间距来自不同层、无法预测；
- 只有表格 cell 图片带 `.cursor(PointingHand)`，文字链接没有指针光标。

## 实现

共享策略在 `src/ui/reader/markdown/rhythm.rs`：`Rhythm`（行高、块间距、标题阶梯、列表间距、引用/code/表格内边距）与 `alert_style`（五类 callout 的 accent、填充、图标）。数值取 Primer `markdown-body`/`headings`/`lists`，以正文 em 的倍数表示，随主题 font scale 缩放：

- 行高 1.6（保留 CJK 可读性，Primer 为 1.5）、相邻块 0.85em（两侧各一半，实测间距就是一个策略值）；
- 标题 2/1.5/1.25/1/0.875/0.85em、字重 600、行高 1.25、上方 1.6em；`h1`/`h2` 加 0.3em 下留白与 1px 底边；`h6` 仍是 `fg_muted`；
- `li + li` 0.25em（tight），loose 列表用 0.85em 段落间距；嵌套列表同一 tight 间距；
- 引用中性（`fg_muted` + 中性条），callout 用语义色：Note=info、Tip=success、Important=chart violet（调色板无紫色 token）、Warning=warning、Caution=danger；标题色经 `readable()` 过渡到 `fg`，light/dark/high-contrast 下对比度 ≥ 4.5:1；
- fenced code 0.9em / 0.5em 内边距、表格 cell 约 6px/13px、`hr` 上下 1.5em。

顶层块列表和嵌套渲染（引用、列表项、脚注、居中）用同一个 `quote_frame`/`heading_unit`/`list_column`，因此嵌套 callout 与顶层一致。`Block::List` 新增 `tight`：pulldown-cmark 的 tight 项不发出 `Tag::Paragraph`，解析器据此判定 loose；HTML `<li>` 列表按 tight；`flatten` 虚拟化分块保留该标记。callout 标题行是呈现元素，不进入 `DocText`，复制文本与选区偏移不变。

指针光标：`LocalLayout` 记录每个链接片段×行的真实字形矩形（`link_boxes`）。`Paragraph` prepaint 在并集上放一个 Normal hitbox，paint 时若指针落在某段矩形内就 `set_cursor_style(PointingHand)`；离开、滚动或已有拖动（`has_active_drag`）都不请求，拖动期间 `ParagraphView` 也不再注册。段落其余文字、空白与整段都不显示指针。表格 cell 文字、引用/列表内链接、锚点（`#fragment`）走同一路径；`links::resolve` 判为 `Refused` 的地址（如 `javascript:`）仍保留链接样式，但无指针、点击继续按既有策略拒绝。`MarkdownView` 在 mouse move/hover-out 更新 `pointer_over_link` 并请求一帧，使光标随 hover 切换。

## 验证

- `ui::reader::typography_tests`（6 项，run `b0902dcd` 通过）：
  - 五类 callout 的解析种类、标题行、纵向留白与正文左缩进；中性 quote 无标题；
  - tight 列表行距 = 行高 + 0.25em、loose 明显更宽、嵌套缩进与 tight 行距、段→列表间距等于一个 0.85em、heading lead-in = 1.6em、`h1`/`h2`/`h3` 字号比例、表格字号仍与正文一致（既有用例）；
  - 340px 窄窗口与 `font_scale = 1.5` 下正文/行盒按比例缩放；
  - `nested_links`：只有相对链接与锚点各一段矩形，Refused 无矩形；hover 真实链接 → `pointer_over_link() == true`，同行普通文字 → false；从链接开始拖动 → false；列表/引用/表格链接都有矩形；300px 下换行链接每行一段矩形；
  - Refused 专用用例：无矩形、hover false，但点击仍返回 `Link(Refused)`（导航策略未变）。
- Reader 全量（run `bafb67db`）236 通过，2 失败均已修复：一是测试误用另一 fixture 的标题，二是为 cell 图片加"贴合图片的 wrapper"会破坏既有缩窄/居中行为，改为保留原 image 行、把光标条件收紧为"可导航且不在拖动"。
- `cargo check --all-targets`（run `4997cb94`）通过；`cargo fmt --all`（run `ecc679b2`）。
- 自动化只断言矩形与 hover 决策：GPUI 测试平台不暴露平台光标，原生光标外观、callout 配色和真实窗口节奏留待负责人验收。

## 边界与后续

- 普通引用保持中性；嵌套容器内标题沿用同一字号阶梯但不画 `h1`/`h2` 底边（避免列表项里出现分割线）。
- 表格 cell 中"被链接包裹的图片"仍以 cell 图片行作为光标目标（图片在行内居中），与既有图片行为一致；段落级 markdown 图片至今不保留外层链接，所以不作为可交互目标。
- 未做：锚点滚动、逐语言代码着色（沿用既有边界）。
- 待负责人验收：callout 颜色/间距、README 节奏、指针光标（含拖动中不显示）。

## 审查修复（PR #94，Codex P2）

Codex 指出 loose 列表项内多个段落失去分隔：移除了 item 列的 `gap_1` 后，`nested_keyed` 不为段落加边距，`- first\n\n  second paragraph` 的两段会贴在一起。修复为 item 内块列使用 `item_inner_gap`：tight 项为 0（对应 Primer `ul ul { margin-top: 0 }`），loose 项为 0.85em 段落间距；顶层与嵌套列表共用。`lists` fixture 增加 loose 多段 item，回归断言 item 内两段间距等于一个块间距，heading lead-in 断言改用该 fixture 的末段（插入新块后原测量跨过了新内容）。

修复后 `cargo clippy --all-targets -D warnings`（run `93b7f0ac`）与 `cargo test --locked`（run `a01e78f1`，1411 项）通过。
