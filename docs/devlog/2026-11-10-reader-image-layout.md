# reader-image-layout：图片成功态尺寸与占位背景（2026-11-10）

- 日期：2026-11-10（Asia/Shanghai）；任务 ID：reader-image-layout；执行者：主 Agent 派发的基层 Agent。
- 目标与状态：实现中（代码与测试完成，待本地核验后提交 PR；PR 链接后补）。

归属：reader-image-layout（`src/ui/reader/**`、reader 图片 fixture/测试、`docs/content-reader.md`、本日志）。

## 问题与目标

用户 issue 6：已成功加载的 Markdown 图片被包裹在一个深色圆角占位面板里，并拉伸到 Markdown 栏宽。期望：成功态直接显示真实像素与 alpha，没有任何占位装饰；只有加载中/不可用/失败/释放时才保留占位。同时尊重解码原生尺寸与受支持的 HTML `img width`/`height` 提示：小图默认不放大，超宽图按比例缩到可用栏宽。issue 7 的视口/Flow 填充与异步重测在此顺带核对，未改共享滚动几何。

## 实现

- `src/ui/reader/images.rs`
  - 移除成功态的 `MIN_DISPLAY_RATIO`/`MAX_DISPLAY_RATIO`/`MAX_DISPLAY_HEIGHT` 钳制；新增 `ImageHints { width, height }` 尺寸提示结构（`src/ui/reader/**` 内共享）。
  - Ready 分支不再叠加 `.bg(colors.sunken)` / `.rounded(radius)` / 占位面板：只画图片本身。图片包在一个 `w_full + max_w(显示宽) + aspect_ratio(显示比例)` 的透明框里（无背景、无圆角、无裁剪），图片绝对定位 `ObjectFit::Contain` 填满该框。框高由比例导出，不强制任何高度上限，因此超高图保持原生高度。
  - 显示宽高由原生尺寸与提示共同决定：显式 `width`/`height` 均不超过原生尺寸（只缩不放），只有一边时按原生比例补另一边；无提示时用原生尺寸。`max_w(显示宽)` + `w_full` 让超宽图被可用栏宽压窄且比例不变。
- `src/ui/reader/markdown/ast.rs`
  - `Block::Image` 增加 `hints: images::ImageHints`。Markdown `![...](...)` 的 hints 为空；HTML `<img>` 通过 `html_image_hints` 读取 `width`/`height`（只接受正整数像素，百分比、单位、`0`/负数忽略）。块级 `img`（`push_html_image`）与行内 `img`（`append_html_element`）两条路径都携带。
- `src/ui/reader/markdown/document.rs`
  - `picture_unit` 接收并透传 `hints` 到 `image_element`；顶层与嵌套图片块都适用。
- `docs/content-reader.md`
  - 记录成功态绘制规则与 `width`/`height` 提示语义。

## 验证

- 解析层单测：`html_tests.rs` 新增 `html_img_width_and_height_become_image_hints`、`markdown_images_never_carry_hints`，证明提示只在 HTML `<img>` 上出现、格式非法值被忽略。
- 视图层（真实管线）：`reader_images_view_tests.rs` 新增
  - `ready_pictures_draw_at_their_own_size_without_a_placeholder`：320×180 原生尺寸不被拉伸；HTML `width=64 height=36` 提示被遵守。
  - `an_oversized_picture_shrinks_to_the_column_and_keeps_its_ratio`：1600×200 横幅缩进栏宽且比例 8:1 不变；120×900 高图保持原生尺寸、不再被高度上限截断。
  - Healthy fixture 的 README 追加一个 `<img ... width="64" height="36">` 徽标块，作为显式尺寸的确定性本地覆盖。
- 现有回归（占位态、懒加载、代次隔离、Flow 自然高度、选区/复制）仍通过同一测试目标。

## 限制与待验收

- 自动化测试证明的是状态机与测量布局，不是设备像素。真实 Windows 原生窗口下的透明 alpha、图标宽高与视觉观感仍属用户原生验收，本任务不声称已通过。
- HTML 仅支持整数像素提示；百分比、`auto`、`style`、响应式 `srcset` 等不在范围。`<img>` 仍走与 Markdown 相同的图片安全边界，未新增任何 HTML 布局能力。
- `docs/reader-images.md` 归 reader-svg owner 维护，本任务未改动；`DecodedImage` 契约保持不变。
