# NSH Reader 原位替换

更新：2026-11-09。原位替换后，Reader 的最终公开路径是 `crate::ui::reader`，宿主按页面接线契约调用 `ReaderHandle::poll()`（UI 排空）与 `ReaderHandle::retire()`（撤权）；替换正文、隐藏与重新授权语义由同一句柄的 `replace` / `suspend` / 换新桥承担。原 `src/nsh/reader` 已移除，`githux-nsh` 预览 package 中不再存在第二份 Reader 实现。

## 定位与边界

- 复用现有业务层不变：`content::PreparedContent`/`PreparedBody`（2 MiB 正文门槛、二进制/NUL 拒绝、BOM/行尾保留）、`content::images::{ImageProvider, ImageLimits, ImageTarget, ImageKey, ImageScope, ImageSink, decode_image}`、`domain::blob::BlobIdentity` 与账户/缓存/授权语义全部照旧。
- 组件自身是纯 Rust NSH v0.4.0（commit `e626a8dd8f8d8b277b9507cfd5ac74d0e67020f3`）视图：`nsh-base` 的 `TextView`（Markdown/HTML 阅读）与只读 `Editor`（源码），关闭 JS/QuickJS 默认 feature，不引 GPUI/Ely 渲染路径。
- 图片能力是**注入的**：组件只把现有 provider 校验过的字节转成内存图片源交给 NSH 绘制，**原始 README URL 永不进入 NSH 默认图片 loader**（所有 `<img>`/`![]` 都走 `TextView::render_image` 覆盖）。没有第二套远程图片网络、没有业务缓存替代、没有 disk-cache。

## 文件

| 文件 | 职责 |
| --- | --- |
| `src/ui/reader/mod.rs` | 公开面：`ResourceContext`、`DocumentKind`、`ReaderDocument`/`ReaderBody`、`ReaderHandle`、`ReaderLayout`、`ReaderAction`、`dark_tokens()`、`reader()`、`ImageBridge`/`Picture` 及 `content::images` 全部类型的 re-export |
| `src/ui/reader/model.rs` | 纯数据准备：`PreparedBody` → NSH `text_view::Document`（Markdown）或源码文本；2 MiB/空/二进制/超限状态；block 数量与访问器供视图虚拟化 |
| `src/ui/reader/markdown/{ast,html,doctext}.rs` | 既有 GPUI reader 解析器原位保留（MIT，Githux 自有）。保留既有 HTML 子集语义（注释丢弃、`script/style` 等整段丢弃、表格/GFM/告警/脚注映射） |
| `src/ui/reader/links.rs` | 同一纯函数链接解析：仓库固定 commit 相对链接、外部 http(s)、锚点、危险 scheme/反斜杠/控制字符拒绝 |
| `src/ui/reader/images.rs` | `ImageBridge`：provider 注入、能力令牌、请求排队/去重、撤权永久退休、隐藏推进代次、迟到结果丢弃、预算；新增 `Released` 状态（预算释放但同一代次内不重复请求）与 `is_retired()`/`limits()` 探针面 |
| `src/ui/reader/view.rs` | NSH 原生渲染：逐块 `TextView`、跨块共享选择、链接 Report、源码只读 `Editor`（行号）、Flow 自然高度、锚点跳转、正文 padding、长文块虚拟化 |
| `tests/reader.rs`、`tests/reader_images.rs`、`tests/nsh_reader/` | 组合行为回归 + 图片桥/真实 `RepositoryImageService` 回归 + headless 兼容证据；装配经 `tests/support/reader_modules.rs` 复用真实 `src/ui/reader` |

## 组件接口（给集成作者 / 其他页面作者）

```rust
use crate::ui::reader::{
    reader, DocumentKind, ImageBridge, ReaderDocument, ReaderHandle, ReaderLayout, ResourceContext,
};

// 1. 在宿主后台执行器上准备（与现有 page-data/仓库会话读取同一处）：
let document = ReaderDocument::from_prepared(prepared_content, kind); // Arc<ReaderDocument>
// 2. 图片桥：复用现有 RepositoryImageService/ReqwestExternalImages，或 None。
let bridge = ImageBridge::new(provider, capability_check, ImageLimits::default());
// 3. 在 NSH UI 线程的响应式作用域里建句柄并挂载一次：
let handle = ReaderHandle::new(document, bridge, ReaderLayout::Repository, move |action| {
    match action {
        ReaderAction::Link(target) => /* 走统一导航/外链策略 */,
        ReaderAction::LayoutChanged { height } => /* Flow 宿主重排 */,
        ReaderAction::AuthorizationRetired => /* 已清除正文，宿主可移除 reader */,
    }
});
// 4. 宿主异步回复每次排空后调用 handle.poll()；隐藏 handle.suspend()；撤权 handle.retire()。
// 挂载： page.child(reader(handle.clone()))
```

宿主职责（组件不碰凭据/存储/网络）：

- **能力令牌** `CapabilityCheck` 是 `Arc<dyn Fn()->bool>`：集成层把「本次回复捕获的 `CacheAuthorization` + session/context 校验」封进去。它只允许原子/内存判定，**不得在闭包里重新捕获新令牌**。令牌失效是永久的：`ImageBridge::retire()` 取消在途、清空已就绪图片，重新授权也必须换新桥（`ReaderHandle::replace` 或换实体）。
- **UI drain**：宿主自己的异步回复路径每次排空后调用一次 `handle.poll()`（纯内存：校验令牌、接受当前代次完成、按预算向 provider 交派新请求）。没有隐式后台轮询；`provider.start` 只入队到现有后台执行器。
- **隐藏**：`handle.suspend()` 推进图片代次并保留已完成图片；窗口隐藏/失活由宿主调用。
- **替换正文**：`ReaderHandle::replace(document, bridge)` 清空旧选区、锚点与图片代次。

## 长文档虚拟化（本轮的阶段一缺口处理）

原 `view.rs` 逐块全量构建 `TextView`；对超大 Markdown 是真实性能缺口。本轮在 bounded（Default/Repository）布局下引入 `WINDOW_AFTER = 512` 的块阈值：超过该块数的文档经 NSH `virtual_list` 挂载，只渲染视口附近的行，`estimated_item_size(64pt)`、行间距与正文 padding 与原全量列一致。

- **正确性边界（不是静默降级）**：虚拟化后，原生跨块选择（Windows 后端 `selectable_view.rs` 遍历当前挂载的文本）只覆盖已挂载行，Ctrl+A 不再覆盖整篇模型。这是 NSH 原生选择语义的固有限制，与阶段一文档中「核验屏外回收行全选语义」是同一缺口。512 块以下的普通文档仍保持全量挂载，Ctrl+A/跨块拖选覆盖整篇。
- **锚点**：虚拟化下 `jump_to_anchor` 用 `scroll_to_index`（块下标）而非像素偏移；全量挂载下仍用实测像素偏移。两种路径都先经 `handle_link` 报告 `LinkTarget::Anchor`。
- **证据**：probe `--test measure`（未提交）实测 `fixtures::large_markdown()`（28488 块，2 MiB）首帧 `~188ms`、仅挂载顶部 `33` 行、`scroll_to_end` 命令 `~124ms` 直达文末（Section 14243）。这是 headless 探针数据，不冒充真实 Windows 窗口的滚动/选择验收（L002）。

## 与既有 Reader 的语义对齐

- `PreparedBody::Empty/Unsupported/TooLarge` 与 2 MiB 门槛：`model.rs` 在转换前判定。状态文案在本轮改为英文（`This file is empty.` / `This file cannot be shown as text.` / `This file is too large to show.`）；旧 NSH 组件的中文状态文案（`文件为空` 等）随 `src/nsh/reader` 一并移除。若负责人要求保留中文文案，属一处常量替换。
- 链接：同一 `links::resolve`，`Report` 模式只回调，**绝不自动外跳**；`#fragment` 回调 `LinkTarget::Anchor`，bounded 布局内由组件本地滚动（`jump_to_anchor`），Flow 交给宿主页面滚动。
- 图片：相对路径固定到 commit、`https` 独立无凭据通路、`http/file/data/javascript//host/UNC/反斜杠/#` 全部拒绝，与 `reader-images.md` 边界一致；`NoProvider` 时显示诚实占位「no image service attached」。
- 源码：NSH 只读 `Editor`（行号、语法着色、选择、`Ctrl+C`、长行横向滚动、软换行关闭、无编辑路径），用「Google Sans Code」；GPUI 版「右对齐行号 gutter + 0.75rem」的精确像素属于 GPUI 排版细节，NSH 用原生 Editor 行号。
- Markdown 排版：NSH `TextView` 自带阅读字号/标题/列表/代码框/表格/告警/脚注；字体经 `dark_tokens()`（`font-sans="Githux UI"`、`font-mono="Google Sans Code"`）与宿主 `.font()` 注册。正文上下 padding 与 Repository/Flow 布局由组件外层 `py/px` 施加。

## 兼容矩阵（2026-11-09，原位替换后）

| 项目 | 状态 | 证据 |
| --- | --- | --- |
| 中文 HTML README（含 `<div>`/`<strong>`） | **verified**（headless） | `tests/nsh_reader` `native_document_mounts_…`；`markdown_and_html_keep_…` 断言 `plain_text()` |
| 隐藏注释丢弃 | **verified** | `plain_text()` 不含 `secret-hidden`/`hidden cell`；headless 不显示 |
| 表格（对齐/GFM） | **verified** | `plain_text()` 含 `表头\t值` / `列\t含义` |
| 围栏代码块 | **verified** | 窗口渲染 `fn main(){…}`；`plain_text()` 含 `fn main()` |
| 相对链接/锚点解析 | **verified** | `relative_links_keep_…`、`parent_segments_…`、`a_fragment_is_…` 断言 commit 固定与路径解析 |
| 危险链接 scheme 拒绝 | **verified** | `dangerous_schemes_are_refused` + 图片桥同源用例 + headless `a_dangerous_link_is_refused_…` |
| 图片拒绝（file/data/http/UNC/#） | **verified** | `resolving_a_picture_uses_the_link_boundary`；headless `pictures_never_reach_a_loader_…` |
| 图片授权令牌/迟到结果/撤权不可复活 | **verified**（桥层） | `a_capability_retirement_clears_…`：撤权后旧 scope 完成被丢弃、重新授权不复活；`two_readers_showing_…` 证明 reader 隔离 |
| 图片预算释放不重复请求 | **verified**（桥层） | `the_decoded_budget_releases_the_oldest_picture_without_refetching_it`：`Released` 状态同代次不重请求，隐藏/恢复后重新请求 |
| 认证图片持久缓存命中 | **gap（组件层）** | 缓存命中由既有 `RepositoryImageService::with_cache` 承担；`the_real_repository_image_service_loads_through_the_bridge` 证明真实服务经桥加载，但未在 NSH 装配里重跑 CacheStore 命中测试，集成时保留其测试 |
| Flow 自然高度 + 仅外层滚动 | **verified**（headless 挂载） | `flow_has_real_natural_height_…`：无内层 `reader-scroll`，整段内容挂载，外层滚动可达文末后续内容 |
| 只读源码行号/不可编辑 | **verified**（headless，NSH Editor） | 行号文本存在、type/backspace/enter 后正文不变（`source_is_read_only_…`、`native_document_mounts_…`） |
| 长文滚动 + 2 MiB 块虚拟化 | **verified**（headless 探针） | `long_documents_mount_only_…`（300 块场景）+ probe 28488 块首帧/尾部数据；见上「长文档虚拟化」 |
| 跨块拖选/复制（原生） | **gap（需原生交互验证）** | headless 证明各文本在同一可选区（`in_selectable_view`）；**鼠标拖选、真实 Ctrl+C 复制、IME 未在真实 Windows 窗口核验** |
| 锚点跳转原生滚动 | **gap（需交互验证）** | `jump_to_anchor` 已实现（全量像素 / 虚拟化块下标两路），headless 只断言 `LinkTarget::Anchor` 被报告，未做点击后滚动断言 |
| Profile Flow 文末可达 | **verified**（headless 挂载 + 真实高度） | 同上 Flow 行 |

区分：**verified**=有 headless 或真实窗口证据；**gap**=已实现但缺对应证据，或明确未实现（原生选择/IME/真实滚动）。原生证据以负责人真实窗口验收为准，不以上述 headless 冒充（L002）。

## 已知缺口与集成注意事项

1. **虚拟化下的跨屏选择**：`WINDOW_AFTER` 以上文档的 Ctrl+A/跨屏拖选只覆盖已挂载行。若产品要求长文全选，需在 NSH 层解决「屏外回收行的选择模型」，或提供宿主侧的全文档复制命令；本轮未静默降级，阈值内文档不受影响。
2. **状态文案语言**：见「语义对齐」第二条，英文文案与阶段一中文文案的差异需负责人确认。
3. **表格单元格内图片**、**行内图片原位**：沿用既有 AST 边界（cell 内保留 alt、inline 图提为块），与 GPUI reader 一致，不在本轮另造。
4. **`nsh-base` 默认 feature 含 `highlight`（syntect）**：纯 Rust，无 JS/QuickJS；`math` feature 未启用，`$$…$$` 显示为 TeX 或由集成层 `render_math` 接管（与 GPUI reader 的 math 呈现不同，属呈现差异）。
5. **配色/字体**：组件 `dark_tokens()` 只设字体族；集成 Root 使用 NSH 自带默认主题，仅设定原字体族。真实多 DPI、中文回退与输入法验收仍需完成。
6. **唯一前缀迁移**：公开 API 前缀从 `crate::nsh::reader` 变为 `crate::ui::reader`，签名不变；`ResourceContext` 字段改为公开并补充 `owner()/name()/commit()/path()` 访问器（与旧 GPUI 契约一致），其余消费者（home/profile/repository 页面任务）按 `crate::ui::reader` 引用。

## 验证命令

```sh
# 探针 package（被忽略），汇编真实 src/ui/reader 与真实 content/domain：
node scripts/verify.mjs -- cargo test --manifest-path .agents/task/probe/Cargo.toml --test reader
node scripts/verify.mjs -- cargo test --manifest-path .agents/task/probe/Cargo.toml --test reader_images
node scripts/verify.mjs -- cargo test --manifest-path .agents/task/probe/Cargo.toml --test reader_compatibility
node scripts/verify.mjs -- cargo clippy --manifest-path .agents/task/probe/Cargo.toml --tests --bins --examples
node scripts/verify.mjs -- cargo fmt --manifest-path .agents/task/probe/Cargo.toml -- --check
# 原生预览窗口（Windows）
node scripts/verify.mjs -- cargo build --manifest-path .agents/task/probe/Cargo.toml --examples
# .agents/task/probe/target/debug/examples/reader.exe / reader_images.exe
```

全部 Cargo 经 `node scripts/verify.mjs -- cargo …`，共用 Windows mutex `Local\Githux-Cargo-20261029`，探针 package 独立 target；命令环境 `CARGO_PROFILE_DEV_DEBUG=0`、`CARGO_PROFILE_TEST_DEBUG=0`、`CARGO_INCREMENTAL=0`（verify runner 注入）。
