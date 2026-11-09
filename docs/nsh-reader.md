# NSH Reader（原生迁移组件）

更新：2026-11-09。`migration/nsh-reader` 分支，唯一写入范围 `src/nsh/reader/**` 与本任务的独立测试装配。这是 Native Shell 迁移第一阶段的 Reader 交付，供 `migration/native-shell` 集成作者摘取；**不代表 Repository/Profile 已正式接线，也不代表全迁移完成**。

## 定位与边界

- 复用现有业务层不变：`content::PreparedContent`/`PreparedBody`（2 MiB 正文门槛、二进制/NUL 拒绝、BOM/行尾保留）、`content::images::{ImageProvider, ImageLimits, ImageTarget, ImageKey, ImageScope, ImageSink, decode_image}`、`domain::blob::BlobIdentity` 与账户/缓存/授权语义全部照旧。
- 组件自身是纯 Rust NSH v0.4.0（commit `e626a8dd8f8d8b277b9507cfd5ac74d0e67020f3`）视图：`nsh-base` 的 `TextView`（Markdown/HTML 阅读）与只读 `Editor`（源码），关闭 JS/QuickJS 默认 feature，不引 GPUI/Ely 渲染路径。
- 图片能力是**注入的**：组件只把现有 provider 校验过的字节转成内存图片源交给 NSH 绘制，**原始 README URL 永不进入 NSH 默认图片 loader**（所有 `<img>`/`![]` 都走 `TextView::render_image` 覆盖）。没有第二套远程图片网络、没有业务缓存替代、没有 disk-cache。

## 文件

| 文件 | 职责 |
| --- | --- |
| `src/nsh/reader/mod.rs` | 公开面：`ResourceContext`、`DocumentKind`、`ReaderDocument`/`ReaderBody`、`ReaderHandle`、`reader()`、`ReaderLayout`、`ReaderAction`、`dark_tokens()` |
| `src/nsh/reader/model.rs` | 纯数据准备：`PreparedBody` → NSH `text_view::Document`（Markdown）或源码文本；2 MiB/空/二进制/超限状态 |
| `src/nsh/reader/parser/ast.rs`、`html.rs` | 从 `src/ui/reader/markdown/{ast,html}.rs` 拷贝（MIT，Githux 自有）。保留既有 HTML 子集语义（注释丢弃、`script/style` 等整段丢弃、表格/GFM/告警/脚注映射） |
| `src/nsh/reader/links.rs` | 从 `src/ui/reader/links.rs` 拷贝。同一纯函数链接解析：仓库固定 commit 相对链接、外部 http(s)、锚点、危险 scheme/反斜杠/控制字符拒绝 |
| `src/nsh/reader/images.rs` | `ImageBridge`：provider 注入、能力令牌、请求排队/去重、撤权永久退休、隐藏推进代次、迟到结果丢弃、预算 |
| `src/nsh/reader/view.rs` | NSH 原生渲染：逐块 `TextView`、跨块共享选择、链接 Report、源码只读 `Editor`（行号）、Flow 自然高度、锚点跳转、正文 padding |
| `tests/nsh_reader/**` | 独立 Cargo 装配（隔离 manifest，锁 NSH 版本、关闭 JS feature）。`src/fixture.rs` 是 native 预览根，`tests/compatibility.rs` 是 headless 兼容证据 |

## 组件接口（给集成作者）

```rust
use githux::nsh::reader::{
    reader, DocumentKind, ImageBridge, ReaderDocument, ReaderHandle, ReaderLayout,
};

// 1. 在宿主后台执行器上准备（与现有 page-data/仓库会话读取同一处）：
let document = ReaderDocument::from_prepared(prepared_content, kind); // Arc<ReaderDocument>
// 2. 图片桥：复用现有 RepositoryImageService/ReqwestExternalImages，或 None。
let provider: Option<Arc<dyn ImageProvider>> = /* 现有服务 */;
let bridge = ImageBridge::new(provider, capability_check, ImageLimits::default());
// 3. 在 NSH UI 线程的响应式作用域里建句柄并挂载一次：
let handle = ReaderHandle::new(document, bridge, ReaderLayout::Repository, move |action| {
    match action {
        ReaderAction::Link(target) => /* 走统一导航/外链策略 */,
        ReaderAction::LayoutChanged { height } => /* Flow 宿主重排 */,
        ReaderAction::AuthorizationRetired => /* 已清除正文，宿主可移除 reader */,
    }
});
// 挂载： page.child(reader(handle.clone()))
```

宿主职责（组件不碰凭据/存储/网络）：

- **能力令牌** `CapabilityCheck` 是 `Arc<dyn Fn()->bool>`：集成层把「本次回复捕获的 `CacheAuthorization` + session/context 校验」封进去。它只允许原子/内存判定，**不得在闭包里重新捕获新令牌**。令牌失效是永久的：`ImageBridge::retire()` 取消在途、清空已就绪图片，重新授权也必须换新桥（`ReaderHandle::replace` 或换实体）。
- **UI drain**：宿主自己的异步回复路径每次排空后调用一次 `handle.poll()`（纯内存：校验令牌、接受当前代次完成、按预算向 provider 交派新请求）。没有隐式后台轮询；`provider.start` 只入队到现有后台执行器。
- **隐藏**：`handle.suspend()` 推进图片代次并保留已完成图片；窗口隐藏/失活由宿主调用。
- **替换正文**：`ReaderHandle::replace(document, bridge)` 清空旧选区与图片代次。

## 与既有 Reader 的语义对齐

- `PreparedBody::Empty/Unsupported/TooLarge` 与 2 MiB 门槛：`model.rs` 在转换前判定，状态文案与 GPUI reader 一致。
- 链接：同一 `links::resolve`，`Report` 模式只回调，**绝不自动外跳**；`#fragment` 回调 `LinkTarget::Anchor`，有界布局内由组件本地滚动（`jump_to_anchor`），Flow 交给宿主页面滚动。
- 图片：相对路径固定到 commit、`https` 独立无凭据通路、`http/file/data/javascript//host/UNC/反斜杠/#` 全部拒绝，与 `reader-images.md` 边界一致；`NoProvider` 时显示诚实占位「no image service attached」。
- 源码：NSH 只读 `Editor`（行号、语法着色、选择、`Ctrl+C`、长行横向滚动、软换行关闭、无编辑路径），用「Google Sans Code」；GPUI 版「右对齐行号 gutter + 0.75rem」的精确像素属于 GPUI 排版细节，NSH 用原生 Editor 行号。
- Markdown 排版：NSH `TextView` 自带阅读字号/标题/列表/代码框/表格/告警/脚注；字体经 `dark_tokens()`（`font-sans="Githux UI"`、`font-mono="Google Sans Code"`）与宿主 `.font()` 注册。正文上下 padding 与 Repository/Flow 布局由组件外层 `py/px` 施加。

## 兼容矩阵（2026-11-09 证据）

| 项目 | 状态 | 证据 |
| --- | --- | --- |
| 中文 HTML README（含 `<div>`/`<strong>`） | **verified**（headless + 原生窗口） | `native_document_mounts_…`；原生窗口截图 `reader-native.png` |
| 隐藏注释丢弃 | **verified** | `plain_text()` 不含 `secret-hidden`；窗口不显示 |
| 表格（对齐/GFM） | **verified** | `plain_text()` 含 `表头\t值`；窗口表格渲染 |
| 围栏代码块 | **verified** | 窗口渲染 `fn main(){…}` |
| 相对链接/锚点解析 | **verified** | `prepared_states_…` 断言 commit 固定与路径解析 |
| 危险链接 scheme 拒绝 | **verified** | `links::resolve` 纯函数 + 图片桥同源用例 |
| 图片拒绝（file/data/http/UNC/#） | **verified** | `images_refuse_dangerous_sources_…`；窗口「local or unsupported source」 |
| 图片授权令牌/迟到结果/撤权不可复活 | **verified**（桥层） | `images_refuse…`、`hiding_advances_scope_…`：撤权后旧 scope 完成被丢弃、重新授权不复活 |
| 认证图片持久缓存命中 | **gap（组件层）** | 缓存命中由既有 `RepositoryImageService::with_cache` 承担，组件只消费 provider；本任务未在 NSH 装配里重跑 CacheStore 命中测试，集成时须以现有 reader-images 服务接线并保留其测试 |
| Flow 自然高度 + 仅外层滚动 | **verified**（headless 挂载） | `flow_has_real_natural_height_…`：无内层 `reader-scroll`，整段内容挂载，外层滚动可达文末后续内容 |
| 只读源码行号/不可编辑 | **verified**（headless，NSH Editor） | 行号文本存在、type/backspace/enter 后正文不变 |
| 长文滚动 + 2 MiB 块虚拟化 | **gap** | 2 MiB 门槛保留在 `model.rs`；但当前 `view.rs` **逐块全量构建 `TextView`**，不做块虚拟化，也未复刻 GPUI 的 `LAZY_CHILDREN` 长列表/表格切块。对超大 Markdown 是真实性能缺口，不影响正确性。NSH 已提供 `virtual_list` + `ListLayout::estimated_item_size`，后续可把块列表虚拟化；需同时核验「跨屏回收行的全选」语义（见下）。 |
| 跨块拖选/复制（原生） | **gap（需原生交互验证）** | 代码侧已把整篇块置于同一 `selectable` 视图（NSH Windows 后端 `selectable_view.rs` 支持跨文本选区、Ctrl+A/Ctrl+C）；headless 只能证明「各文本在同一可选区」，**鼠标拖选、真实 Ctrl+C 复制、IME 未在真实 Windows 窗口核验** |
| 锚点跳转原生滚动 | **gap（需交互验证）** | `jump_to_anchor` 已实现，headless/窗口未做点击后滚动断言 |
| Profile Flow 文末可达 | **verified**（headless 挂载 + 真实高度） | 同上 Flow 行 |

区分：**verified**=有 headless 或真实窗口证据；**gap**=已实现但缺对应证据，或明确未实现（块虚拟化）。原生证据见 `.agents/task/reader-native.png` 与 `.agents/task/runs/` 记录；真实鼠标拖选/IME/复制必须以负责人真实窗口验收为准，不以上述截图或 headless 冒充（L002）。

## 已知缺口与集成注意事项

1. **块虚拟化**：当前全量构建块，超大 Markdown 的性能缺口。不改产品/后端路线，属普通适配缺口，建议集成阶段用 NSH `virtual_list` 处理；在虚拟化前须先确认 NSH 的 `selectable` 视图对「屏外回收行」的全选/Ctrl+A 是否覆盖全部文本，否则长文跨屏选择会退化（这是需要实际核验的反例，不静默降级）。
2. **表格单元格内图片**、**行内图片原位**：沿用既有 AST 边界（cell 内保留 alt、inline 图提为块），与 GPUI reader 一致，不在本轮另造。
3. **`nsh-base` 默认 feature 含 `highlight`（syntect）**：纯 Rust，无 JS/QuickJS；`math` feature 未启用，`$$…$$` 显示为 TeX 或由集成层 `render_math` 接管（与 GPUI reader 的 math 呈现不同，属呈现差异）。
4. **配色/字体最终值**：`dark_tokens()` 只设字体族，调色板沿用 NSH 默认（shadcn neutral）。Githux 深色外观、主题 token、Noto Sans SC 回退的最终对齐归集成作者（`native-shell`）统一，本任务不重复实现主题系统。

## 验证命令

```sh
# 组件 + headless 兼容测试（隔离 manifest，走共享 runner 与独立 target）
node scripts/verify.mjs -- cargo test --manifest-path tests/nsh_reader/Cargo.toml --test compatibility
# 原生预览窗口（Windows）
node scripts/verify.mjs -- cargo run --manifest-path tests/nsh_reader/Cargo.toml --bin githux-nsh-reader-probe
```

全部 Cargo 经 `node scripts/verify.mjs -- cargo …`，共用 Windows mutex `Local\Githux-Cargo-20261029`，独立 `CARGO_TARGET_DIR=target/nsh-reader`，命令环境 `CARGO_PROFILE_DEV_DEBUG=0`、`CARGO_PROFILE_TEST_DEBUG=0`、`CARGO_INCREMENTAL=0`。
