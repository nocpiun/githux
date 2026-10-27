# Markdown 阅读效果的上游接口边界

更新：2026-10-27。任务 `content-reader`，PR #27。**旧 vendor / 单 crate 补丁的批准已撤销，本任务不再包含 `vendor/ely-gpui-component`、Cargo patch、fork 或缓存补丁。** 当前依赖是官方仓库固定 revision `30c66fe1d5d23db769a6c834b771d054d7f288cf` 的 `ely-core`、`ely-documents`、`ely-editor`，GPUI `=0.3.10`。

## 已完成的迁移

- reader 的 Markdown 使用上游 `ely_documents::MarkdownDocument` Preview 模式；Source 使用 reader 自有只读虚拟列表和官方 `ely_core::forms::code_highlights`。
- 上游文档自己持有真实 `ListState` 并提供右侧滚动条；不伪造外层 handle，也不做 overlay 双滚动条。公开可测的滚动位置是 `top_line()`，测试验证真实滚轮会改变它。
- 上游文档有异步解析。由于 Markdown 图片/HTML 可能触发上游文件或 data 资源路径，reader 在把正文交给文档前用公开 mdast 树在后台移除 `Image`、`ImageReference`、`Html` 节点，保留换行并显示纯文本占位。完成后校验目标文档实体，旧结果不回写。2 MiB 边界、只读选择/复制、链接不自动外跳和版本化危险 scheme 拒绝仍由 reader 负责。
- 依赖只增加 `ely-documents` 与 `ely-editor`（后者由 documents 使用并提供代码着色），不使用汇总所有垂直组件的 facade。

## 未满足的视觉要求（证据）

固定上游公开接口无法保持旧补丁提供的四项视觉细节。原验收测试仍保留并**明确失败**，没有删除、静默跳过或把上游同名组件当作完成：

| 要求 | 当前证据 | 最小上游方案 |
| --- | --- | --- |
| fence 语言标签上移 | 文档预览不转发 `MarkdownRenderer::code(...)`；没有对应 code-block 语言元素。`code_fence_language_sits_near_the_block_top` 失败。 | 在文档绘制路径转发已有 renderer code hook，或在文档 API 暴露语言标签 inset。 |
| inline code 圆角、小 padding、代码字体 | 文档只使用 `HighlightStyle`，无独立字体/圆角/padding 与字形位置。相关 inline 测试失败。 | 提供只读段落的代码 run 布局扩展点，保持同一文本布局用于选择/复制。 |
| Markdown 3 px 圆角半透明 hover Fade 滚动条 | 上游 `Scrollbar` 固定主题 6 px、自身 activity 显隐，无法接 reader 的 3 px/140 ms 双向 fade；`ListState` 仍私有，无法外接。测试失败。 | 允许文档暴露真实滚动源或 style/presence/drag 接入点；不复制一套私有列表。 |
| Source 行号 0.75rem+6px | 通过 reader `gutter_gap` 保留，真实布局和 1/2/4/5 位行号测试通过。 | 不需要上游改动。 |

## 维护边界

不得恢复整库或单 crate vendor、fork、Cargo 缓存补丁，也不应为了这四项复制整套 `ely-documents` 的 Markdown 布局。上述缺口需要上游提供长期可维护的公开扩展点；此前“从上游局部提取 Markdown 架构”只是讨论稿，没有获批。PR #29 的共享滚动条核心也未合并，本任务不复制其实现；后续统一接线点应在它合并后由协调任务处理。
