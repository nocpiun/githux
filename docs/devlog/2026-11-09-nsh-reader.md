# 2026-11-09 nsh-reader

Native Shell 迁移第一阶段 Reader 任务。分支 `migration/nsh-reader`，base `daeb0f86`。本地交付，无远端操作。

## 结果

- `src/nsh/reader/**`：NSH v0.4.0（commit `e626a8dd`，未跟踪浮动 main）纯 Rust Reader 组件。复用现有 `content` 准备与图片契约，只读 `src/ui/reader` 作行为参考，未改它。
- 解析层拷贝 `src/ui/reader/markdown/{ast,html}.rs` 与 `links.rs`，保持既有 Markdown/GFM/HTML 子集（注释丢弃、危险 scheme 拒绝、固定 commit 相对链接）语义；转成 NSH `text_view::Document`。
- 图片桥 `ImageBridge` 落实 L022：能力令牌只读原子校验，撤权永久退休、迟到结果丢弃、重新授权不复活；所有图片经 `render_image` 覆盖，原始 URL 不进 NSH loader。
- 源码用 NSH 只读 `Editor`（行号、选择复制、无编辑路径）。
- `tests/nsh_reader/**`：隔离 manifest（锁 release、关 JS feature、独立 `target/nsh-reader`）+ headless 兼容测试 + Windows 原生预览 binary。

## 验证

- headless `tests/nsh_reader/tests/compatibility.rs` 5 用例：准备状态/HTML/注释/表格/链接；图片拒绝与迟到/撤权；隐藏推进代次且不影响其他 reader；原生挂载与图片覆盖；Flow 自然高度。5/5 通过（最终 head）。
- 真实 Windows 窗口：`cargo run --bin githux-nsh-reader-probe`，`MainWindowTitle="Githux NSH Reader compatibility probe"`、`Responding=True`；截屏 `.agents/task/reader-native.png` 证实中文 README/HTML/表格/代码块/链接/被拒绝 `file:` 图片占位。真实拖选/Ctrl+C/IME 未在窗口验收。

## 关键事实与边界

- NSH `TextViewProps.selectable` 是整篇一个选区（跨块）；其 `view.rs` 顶部注释「each block on its own」已与实现不一致，以源码 `selectable_view.rs`（Windows 后端跨文本选区、Ctrl+A/Ctrl+C）为准。
- NSH `Tokens` 无公开 `new()`，用 `Tokens::default()` + `set_font`；字体族经 `Root::font()` 注册后按 family name 命中。
- NSH `ElementRef::new` 是 crate 私有，公开构造是 `element_ref()`。
- headless 选择器测试 id 是 `id:…`，裸 `#…` 不是 test id 前缀。

## 缺口（交集成）

- 块虚拟化未实现（当前逐块全量构建，超大 Markdown 性能缺口）。NSH `virtual_list` + `estimated_item_size` 是路径，但须先核验屏外回收行的全选语义。
- 认证图片持久缓存命中未在 NSH 装配重跑（组件只消费 provider）。
- 跨块拖选/复制、锚点原生滚动、中文 IME、多 DPI 缺真实窗口交互证据。
