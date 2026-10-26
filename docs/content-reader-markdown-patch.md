# Markdown 阅读效果的 Ely 补丁

更新：2026-10-26。任务 `content-reader`，PR #27。协调裁决已批准单 crate 补丁，本文记录**实际实现**，不再等待上游审批。

## 固定与复现

根 `Cargo.toml` 的普通依赖与 dev-dependency 都经 `[patch]` 指向 `vendor/ely-gpui-component`。基线是 `da3fdcaf39f2fa85ce1dd9cad07c3c81cf1dd721`，Ely 0.1.1、GPUI 0.3.8、Rust 1.98.1 均未升级。`cargo metadata` / `cargo tree -i ely-gpui-component` 核对同一解析结果；`cargo check --locked` 可在不访问 Ely 上游的干净环境重建。

目录只含构建必需的 `src/`、`assets/`、`Cargo.toml` 和 MIT `LICENSE`，不含整个依赖树、`.git`、缓存或上游 gallery。`README.githux.md` 记录来源、基线、实际改动与维护方法，`patches/` 保留逐文件差异。本机 Cargo cache/checkout 未修改。

## 已修正的旧 API 说法

旧方案称 `list(...).track_scroll(&handle)` 可用于 GPUI 0.3.8 的 `List`，这是错误的。现在由 `MarkdownDocument::list_state()` 暴露**原有** `ListState`，阅读器用 `viewport_bounds`、`max_offset_for_scrollbar`、`scroll_px_offset_for_scrollbar` 绘制细滚动条，并用 `set_offset_from_scrollbar` / `scrollbar_drag_started/ended` 拖动。列表不重建，正文仍是虚拟化；滚动通知让 overlay 随真实偏移重绘。

## 实际改动

1. `document/mod.rs`：真实 `ListState` 访问器和滚动通知。
2. `document/visual.rs`、`render.rs`：Preview 可选启用 reader 排版；编辑模式保持 Ely 原有点到源码映射。
3. `render/inline.rs`：一个连续段落元素。每段按正文和 `theme.mono_family` 的真实 `TextRun` 塑形；代码 padding 参与推进、换行、圆角背景和命中计算。长代码按 grapheme 边界断行，复制直接读原 inline 字节，不塞入 padding 字符。硬换行强制另起一行，链接按字形命中，与选择 caret 分开。短段落返回实际固有宽度以保留表格列对齐。样式边界用 sweep，字形位置只索引一次，避免逐字调用 GPUI 线性 `x_for_index`；只绘制裁剪区内的行。测试快照来自同一帧实际布局，包含代码字体与几何。
4. fence 语言标签仅局部上移 8px；代码文本保持其布局与语法着色。
5. `picture.rs`：仓库 reader 的图片策略在创建 loader 前拒绝文件系统与 data 来源，包括 `file:`、绝对/UNC 路径、带 base 的相对路径；外网图片仍只显示未下载提示。通用上游调用方的默认行为保持原样。`documents/mod.rs` 仅在 `test-support` 下导出布局/策略核验接口。

阅读器侧 `ReaderView` 持有与源码/标签条同源的 `Fade`，hover 淡入淡出、拖动保持反馈、无溢出不画条。Source gutter 仍为原 0.75rem 加 6px。

## 验证边界

GPUI 测试平台覆盖真实 ListState 滚轮/拖动、hover 淡入淡出、resize/替换范围 clamp、长文虚拟化、代码字体和布局、窄窗口断行、跨代码段选择复制、fence 几何、表格对齐和文件图片拒绝。密集段落使用 20,000 个 inline code span，计时实际 resize 排版而非切换到普通 fixture。Windows 真实窗口仍由负责人验收；不声称测试平台等于最终视觉验收。
