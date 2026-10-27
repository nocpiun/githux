# Markdown 组件的本地维护边界

更新：2026-10-27。任务 `content-reader`，PR #27。负责人已明确授权把 Ely 中 Markdown 阅读相关组件拆出，由 Githux 单独维护和改造；此前“局部提取只是讨论、只能等上游接口”的限制已被覆盖，不再作为阻塞。

## 依赖与来源

- 官方 Ely Git：`https://github.com/ELY-Open-Source/Ely-GPUI-Components`，固定 revision `30c66fe1d5d23db769a6c834b771d054d7f288cf`；GPUI / platform `=0.3.10`，Rust 1.98.1。
- 仍使用 `ely-core 0.2.0` 的主题、字体资源、图标、代码着色（`forms::code_highlights(_with)`、`syntax_fenced`）与滚动条样式常量。
- 不再依赖 `ely-documents` / `ely-editor`；没有整库或单 crate vendor、Cargo patch、fork、缓存修改，也没有复制整个 `ely-documents`、编辑器、block editor 或大型图标字体。
- 新增直接依赖仅 `pulldown-cmark 0.13.5`（已关闭默认的 html/getopts feature）和 `unicode-segmentation 1`。锁文件由 main 基线下降。

## 源文件到本地文件映射

| 上游固定源（`ely-documents/src`） | 本地文件 | 处理 |
| --- | --- | --- |
| `lib.rs` 文档入口、`document/mod.rs`（Preview 路径） | `src/ui/reader/markdown/document.rs` | 裁剪为只读文档；去掉编辑模式、块编辑、历史、附件、阅读器 |
| `render.rs` 段落与排版、`render/style.rs` | `src/ui/reader/markdown/inline.rs` | 重写 inline 排版，代码字体/圆角/padding、选择、复制与链接命中 |
| `tree/*`、`markdown/*`（mdast） | `src/ui/reader/markdown/ast.rs` | 以 `pulldown-cmark` 事件流重写块解析；仅保留阅读所需语法 |

上游 MIT 许可证适用于这些派生文件；Githux 自身新增代码同样按仓库许可维护。上游升级时按上表逐文件审阅差异，禁止脚本全量覆盖本地改造。

## 维护的功能边界

- 解析在短文本同步、长文本后台进行；每次 reset 推进代次，旧结果不能覆盖新文档。2 MiB 上限先由内容层拒绝。
- 列表使用真实 `gpui::ListState`，只构建可见块；`list_state()` 是公开的真实滚动源；PR #29 合入后，reader 的细条已改用共享 `src/ui/scroll` 的 `ScrollTarget for ListState`，没有过渡条或重复实现。没有假外层 handle、重复滚动条或透明拦截区域。
- 行内代码由带真实 padding 的圆角框绘制；padding 参与换行、绘制和命中的同一组字形坐标，复制直接读原文，长代码可断行。
- fenced code 语言标签是真元素，在代码块内上移约 6px（叠加 `mb_1`），代码正文和块间距不变。
- 图片不创建任何 loader，仅显示 alt/未载入提示；HTML 作为惰性文本；危险链接仍由 reader 的 `links::resolve` 拒绝；链接只回调，不自动外跳。

## 文档级选择与行号间距

选择不再是每个段落各持一份状态：`MarkdownView` 持有全文档的 `anchor/head`、拖动生命周期和焦点，`DocText` 在解析后按渲染顺序登记每个可选择单元的文档范围。段落只把指针位置经自身真实 `TextLayout` 映射为文档偏移，并绘制文档选区与自身范围的交集，因此可以选择跨标题、正文、列表、引用、代码、表格的连续文本。

- `Ctrl+A` 选择整篇模型的文本（含视口外未实例化的块），`Ctrl+C` 按阅读顺序复制；空选区不写剪贴板。
- 阅读区留白、段间空隙、短行尾、内容之后的空白按下即清除整篇选区并结束旧拖动。
- 拖动使用 `on_drag`/`on_drag_move`（GPUI 在整个拖动期间跨元素分发），按当前 head 的方向越过空白、跨块与视口边界扩展；失焦或释放都会结束拖动。
- 单元仍使用唯一 key（含嵌套路径），不同单元不会串选区；单元格文字（含空单元格）也各自登记为一个单元，渲染按单元游标消费，前后单元不会错位。
- 围栏代码块登记自己的可选择布局并复用同一份文档选区与代码字体排版，不再只能靠 `Ctrl+A` 选中。
- 超长列表/表格按 `LAZY_CHILDREN` 切块；重复表头只占一个空单元以保持游标同步，不进入复制文本。
- 指针命中只读取事件发生时已布局的单元，刚滚入的段落立刻可点；拖到段落行尾空白时先按最近行的端点解析，再退回块边界，不会吞掉后面整块。
- Source 行号左侧为 0.75rem 真实布局 padding（原 0.5rem 增至 0.75rem），右侧仍是原 0.75rem+6px，数字右对齐，正文列与命中、横滚共用同一布局。

## 验证

见 [阅读器文档](content-reader.md) 与 [开发日志](devlog/2026-10-25-content-reader.md)。真实窗口自动验证与未覆盖项分别记录，不把测试平台结果当作原生视觉验收。
