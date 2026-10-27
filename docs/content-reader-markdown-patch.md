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

## 验证

见 [阅读器文档](content-reader.md) 与 [开发日志](devlog/2026-10-25-content-reader.md)。真实窗口自动验证与未覆盖项分别记录，不把测试平台结果当作原生视觉验收。
