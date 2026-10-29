# README 图片加载与显示

更新：2026-10-29（`reader-images` 任务）。为现有可复用 [阅读器](content-reader.md) 增加有界的 README 图片能力：固定 commit 的仓库相对图片、公开 HTTPS 图片、失败/拒绝占位，以及可取消的异步生命周期。**正式 Shell 尚未接入**：本切片通过独立预览 `examples/reader_images.rs` 验收，主界面的接线点见下文「后续正式接线契约」。

## 分层与文件

| 层 | 文件 | 职责 |
| --- | --- | --- |
| 图片契约与预算 | [`src/content/images/`](../src/content/images/mod.rs) | `ImageLimits`、`ImageTarget`、`ImageKey`、`ImageScope`、`ImageProvider`、`ImageSink`、格式嗅探与有界解码。无 I/O，无 GPUI |
| 无凭据外部读取 | [`src/network/reader_images.rs`](../src/network/reader_images.rs) | `ExternalImageBytes` 与生产实现 `ReqwestExternalImages`：仅 HTTPS、手动逐跳验证重定向、总超时、流式字节上限 |
| 固定 commit 的仓库图片 | [`src/data/reader_images.rs`](../src/data/reader_images.rs) | `RepositoryImageService`：复用已捕获的 `RepositorySession` 的 revision/directory/blob 能力，自己持有 executor 与外部字节源，不持有 PAT |
| 阅读器图片状态 | [`src/ui/reader/images.rs`](../src/ui/reader/images.rs) | 每个 reader 每个文档代次的 `ImageBoard`：请求排队、在途预算、完成应用、淘汰、拒绝与占位绘制 |
| Markdown 块接入 | [`src/ui/reader/markdown/document.rs`](../src/ui/reader/markdown/document.rs) | `Block::Image` 的解析目标解析、状态查询，以及图片尺寸确定后的块重测 |
| 预览与测试装配 | [`src/bin_support/reader_images_modules.rs`](../src/bin_support/reader_images_modules.rs)、[`src/bin_support/reader_images_fixture.rs`](../src/bin_support/reader_images_fixture.rs)、[`src/bin_support/reader_images_view_tests.rs`](../src/bin_support/reader_images_view_tests.rs)、`examples/reader_images.rs`、`tests/reader_images.rs` | 真实流水线的确定性预览和验收测试，非产品二进制的一部分 |

Reader 原构造器、`show`、`set_markdown_layout`、`SourceView`、`ReaderAction`/链接回调、`ResourceContext` 与 2 MiB 正文边界保持不变。没有传 provider 的 Reader 仍然可以显示所有文档，只是每张图片显示诚实的占位，不会假报「已加载」。

## 调用契约

```rust
use std::sync::Arc;
use githux::ui::reader::{ImageLimits, ImageProvider, ReaderView, DocumentKind};
use githux::data::reader_images::RepositoryImageService;
use githux::network::reader_images::ReqwestExternalImages;

// `session` 是当前页面已验证的 RepositorySession（它拥有账户/授权上下文）；
// `repository` 是正文所属的 RepositoryIdentity；
// `executor` 是应用已有的 Arc<BackgroundExecutor>。
let external = Arc::new(ReqwestExternalImages::new()?);
let provider = Arc::new(RepositoryImageService::new(
    session.clone(),            // 账户/epoch/选择由 session 复核，reader 不碰凭据
    repository.clone(),         // 必须与正文同仓库，否则返回 StaleContext
    executor.clone(),
    external,                   // 公开外链独立无凭据通路
    ImageLimits::default(),
));
let reader = cx.new(|cx| {
    let mut reader = ReaderView::from_prepared(content, kind, window, cx);
    reader.set_markdown_layout(MarkdownLayout::Repository, cx);
    reader.set_image_provider(Some(provider as Arc<dyn ImageProvider>), cx);
    reader
});
```

- **不传 provider**：`ReaderView` 的默认图片板未附着，任何 `Block::Image` 显示 `alt — picture not loaded (no image service is attached)`；不会发起任何请求，也不会报告加载成功。
- **传 `None` 或换 provider**：`set_image_provider` 先取消旧 scope 的在途工作，再清空未完成的请求；已完成的图片保留，但旧代次的完成不会发布。
- **每个 reader 一份 provider**：接口允许共享，但推荐一份文档代次一份 provider。`ImageScope { reader, generation }` 让两个 reader 即使 generation 数字相同也不会互相取消或接收对方的完成；`RepositoryImageService` 的快照/目录缓存只在它自己的实例内，SHA 相同不是跨账户的授权依据。
- **更换文档/账户/选择**：与现有 reader 规则一致，推荐换实体（或至少调 `show`）。`show` 推进图片代次、取消在途、清空旧图片与旧错误。账户失效时宿主在 `cancel_interactions` 的同时移除 reader；图片服务自己的 `verify()` 复核 session，旧授权下的字节不会变成当前文档的图片。
- **隐藏窗口**：`observe_window_visibility` 取消当前 scope 的在途工作、保留已完成的图片；恢复可见时下一帧按需重新请求。
- **解码与绘制**：解码在 provider 的后台线程完成，UI render 不做网络、不做解码、不做阻塞 I/O。已接受的图片以 `gpui::Image`（原始已验证字节）交给 GPUI 绘制，像素缓冲不在本模块常驻。

## 地址解析与边界

图片地址复用 `links::resolve` 的同一套纯函数边界，再转成 `ImageTarget`：

| 地址形态 | 结果 | 行为 |
| --- | --- | --- |
| `images/diagram.png`、`../`、`/docs/images/a.png` | `Repository { owner, name, commit, path }` | 当前 README 所在目录、固定 repo/commit；`..` 到根停止；`%XX` 严格解码，坏转义拒绝 |
| `https://…` | `External { url }` | 独立无凭据 HTTPS 通路 |
| `http://…` | `Refused(InsecureExternal)` | 外部图片仅 HTTPS，不静默降级 |
| `file:`、`data:`、`javascript:`、其他 scheme、`//host`、UNC/反斜杠/控制字符、空地址、`#fragment` | `Refused(..)` | 不请求、不执行、不读本地/SMB，与链接边界一致 |
| `![alt](file:///...)` | 同上 | alt 保留，显示「local or unsupported source」，不会因为地址危险就丢弃文字 |

`raw HTML` 仍按既有行为作为惰性文本，`<img src>` 不执行、不额外加载；不新增 HTML 浏览器。

## 固定 commit 的仓库图片

`RepositoryImageService` 不发明新的读取方式：它复用 `RepositorySession` 的能力，在固定 commit 上还原图片的真实 Git 对象：

1. `RevisionSpec::Commit(sha)` → `RepoSnapshot`（响应 SHA 必须等于请求 SHA）；
2. `SnapshotDirectory::root` → 按路径段逐级 `session.directory`（每个 tree 的 SHA 与请求一致）；
3. 末段必须是 `TreeEntryKind::File`，`BlobIdentity::from_file` 得到同 commit 的 blob 身份；**符号链接、submodule 不解引用**，返回 NotFound；
4. `session.blob` 取原始字节（`BlobLimits` 由图片预算推导），再到 `content::images::decode_image`。

因此：不会读当前默认分支，不会用 contents API 的 UTF-8 `content` 当图片，不会请求或持久化 `download_url`，不会把固定 commit 的图片当成跨账户的缓存授权。请求到的 owner/name 必须与 session 的仓库一致（大小写不敏感比较），否则 StaleContext；目标路径与仓库不匹配直接 NotFound。目录快照与目录树的小缓存只在服务实例内，最多 4 个 snapshot、8 棵目录树、4096 个目录条目，并随服务销毁。

`ImageKey` 是解析后的源标识：`repo:owner/name@commit:path` 或 `url:https://…`。同一路径在两个 commit 是两个源；同一图片在一个文档的两个块里共享一次请求。外链按完整 URL 标识；**动态 badge URL 不因 README 固定 commit 而被视为永久不变**，本切片也不做磁盘缓存或 TTL 持久化。

## 外部 HTTPS 通路

`ReqwestExternalImages` 使用自己的 `reqwest::Client`：

- 不设置 `Authorization`、`Cookie`、`Proxy-Authorization`，请求只带图片 Accept；`.no_proxy()` 禁用系统代理，避免把代理凭据转发到图片主机。
- 生产构造只接受 `https`；无 userinfo；拒绝 loopback/`localhost`/未指定/链路本地地址（`file:`、`data:`、`javascript:` 等在进入 URL 校验前就已被 reader 拒绝）。
- `Policy::none` 后**手动逐跳**重定向：用与连接方相同的 `reqwest::Url` 解析和 `join`，每一跳重新校验 scheme/host/userinfo，拒绝本地来源；超过 `max_redirects` 报 `TooManyRedirects`，不把 3xx 当成功。这避免了把凭据送到重定向目标（本路径根本没有凭据，但本地来源校验不能省）。
- 总超时覆盖连接、每一跳和整个 body；`Content-Length` 只做提前拒绝提示，实际字节在流式累计时限制，超限即 `TooLarge`，不会先缓冲完整超大 body。
- 测试构造 `with_local_http()`（`#[cfg(test)]`）允许 loopback `http`，用真实 socket 验证「请求里没有 Authorization/Cookie/Proxy-Authorization」「同源重定向跟随」「file/data 重定向不接触目标」「重定向循环有界」「Content-Length 缺失的流式上限」「停顿超时」。

仓库图片的 PAT 只经过既有 `read_authenticated` 链路；外链不触碰 `GitHubClient` 的认证头。测试不读取真实用户 token。

## 工程预算

`ImageLimits` 的默认值与行为：

| 参数 | 默认 | 拒绝点 |
| --- | --- | --- |
| `max_response_bytes` | 5 MiB | 接收时（blob JSON/外部流式）与解码前双重检查，超限 `TooLarge`，不先缓冲 |
| `max_decoded_bytes` | 32 MiB | 解码前按 `width*height*4` 预算，另作为 `image::Limits::max_alloc` |
| `max_pixels` | 8,000,000 | 先读 header 得到尺寸，超过即 `TooManyPixels`，在危险分配之前 |
| `max_dimension` | 8192 | 同上，单边上限，同时设置 decoder 严格维度上限 |
| `total_timeout` | 20 s | 单次完整读取（含重定向与 body），超时 `Timeout` |
| `max_concurrent` | 3 | 每个 provider 同时执行的加载数，semaphore 限制 |
| `max_in_flight` | 12 | reader 已交给 provider 但未完成的请求上限；其余按可见顺序排队 |
| `max_ready_images` | 32 | 每个 reader 当前文档保留的已解码图片数，超出按最老释放 |
| `max_ready_bytes` | 64 MiB | 按 `w*h*4` 估计的保留预算，超出同样释放 |
| `max_redirects` | 5 | 外链重定向上限 |

拒绝发生在危险分配之前：`decode_image` 先嗅探格式，再只读 header 的尺寸，预算检查通过后才在 `image::Limits` 下完整解码验证。畸形、截断、签名正确但内容损坏都报 `Malformed`；PNG/JPEG/WebP/GIF 由固定 `image` 依赖支持。**SVG 明确 Unsupported**：当前依赖不做有界安全光栅化，不执行 raw SVG、不新增组件库、不静默改依赖；`is_svg_like` 用于区分「不支持」与「损坏」。

GIF 可以有多帧；GPUI 的 `Img` 在非 reduced-motion 且窗口活动时按帧播放，reduced motion 下显示首帧。本模块不为动画另开线程或缓存。

## 显示、尺寸与滚动

- 每个图片块保持比例：图片盒用 `width/height` 比例，宽度是唯一固定的维度（受内容列宽限制，并 `max_w(intrinsic)`），高度由比例推导，因此**小图不被拉大为模糊**、宽图不撑破列、很高的图通过收窄宽度限制高度（有效高度上限约 640px）；图片本身绝对定位在这个盒内 `ObjectFit::Contain`——若让它参与盒子的正常流，它自己的高度会让盒子突破比例（实测 320×180 曾变成 320×479）。
- alt 文本在 Ready 时作为图注显示，在 Loading/Failed/Refused/无 provider 时与原因一起显示；空 alt 也有可读的占位文本。
- 未知尺寸（加载中/失败/拒绝）预留 140px 占位高度；图片完成后对该顶层块调用真实 `ListState::remeasure_items`，以**锚定当前滚动位置**的方式重测，不重排版整篇文档，不插入假 AST 项，不把文末 inset 挤出测量范围，不因图片落地产生全文档高频重绘。在 `MarkdownLayout::Flow`（自然高度）下改为请求一次重绘并由 `on_layout_changed` 通知宿主，宿主的页面容器随之增长。
- 跨块选择/复制不受影响：图片块的 alt 仍在 `DocText` 中，图片本身不注册字形选区；选区坐标来自段落真实排版，不因图片尺寸变化而错位。表格内图片仍受 AST 限制（见下）。滚动条继续由共享 `Motion`/`Bar` 读取同一个真实 `ListState`。
- 行内图片与嵌套块（引用/列表/脚注）中的 `Block::Image` 走同一状态与占位逻辑；段落中夹在文字里的图片在解析阶段仍被提取为紧随段落的图片块（沿用既有 AST 行为），**不在一行文字中间原位置绘制**。

## 生命周期与取消

- 一个 reader 的每次文档代次有一个 `ImageScope`；`show`/换 provider/换预算推进代次并取消旧 scope，旧完成一律丢弃。reader 的 pump 在 GPUI 执行器上以轻量节拍（20ms）驱动，**有工作才运行，结清即停**，不做跨线程唤醒，不做永久后台轮询，不在 render 中发 I/O。
- 文档替换、账户变化、窗口隐藏/关闭都会取消无用工作；provider 内部对每个 scope 一个取消源，Drop 时取消全部。取消的 scope 不再向 sink 发布完成。
- 两个 reader 显示同一图片时，各自有独立的 board、请求、取消和保留预算。即使宿主共享一个 provider，`ImageScope.reader` 也把工作分开（专项测试覆盖 `cancel` 只取消一个 scope）。
- 授权失败/过期在数据层按 `StaleContext` 报告；reader 不因此保留旧授权图片，不制定私有内容移除或授权失效保留政策，不新增磁盘缓存。

## 支持边界（明确未覆盖）

- **表格单元格内的图片不加载**：AST 在 cell 中没有块列表可挂图片，现有行为是保留 alt 文本在单元格内（见 `ast.rs` 的 `in_cell` 分支）。这不属于「已覆盖表格图片」。
- 段落中的 inline 图片被提取为段落后的图片块；不是文字行内原位置。
- SVG、AVIF、BMP、TIFF、ICO 等不在支持范围（Unsupported）；不执行任何图片内脚本或外部资源。
- 不做磁盘缓存、离线图片、跨账户/跨实例的持久授权复用；不把 badge 当永久固定。
- 尚未接入正式 Shell：`src/ui/repository/mod.rs`、profile、Home 仍使用旧 Reader API，无 provider 时行为与以前一致（显示诚实占位）。
- 真实 Windows 窗口的原生验收（DPI、动画、IME、真实输入分发）以预览观察和后续负责人验收为准，测试平台不能替代。

## 后续正式接线契约

最小调用点（以 `RepositoryPage` 为例）：

1. 页面已有 `RepositorySession` 与 `RepositoryIdentity`（`apply_content` 分支，reader 构造处）。
2. 用 `RepositoryImageService::new(session.clone(), repository.clone(), executor.clone(), Arc::new(ReqwestExternalImages::new()?), ImageLimits::default())` 建 provider；`ReqwestExternalImages::new()` 的错误按页面现有错误模型处理（不静默当无图片）。
3. `reader.set_image_provider(Some(provider as Arc<dyn ImageProvider>), cx)`；`set_markdown_layout` 等旧调用保持原顺序。
4. 页面切换分支/文件/账户、deactivate/rebind/close 时，沿用现有「丢弃 reader 实体」的行为即可，图片代次随 reader 结束；如保留实体则调用 `reader.show(...)` 或 `reader.set_image_provider(None, cx)`，不要复用旧 provider 跨授权。
5. 不需要为图片新增 PAT、存储、路由、缓存键或账户 API。

预览 `examples/reader_images.rs` 里的 `Pipeline::open` / `open_document` 与 `open_reader` 就是上述流程的真实实现（`src/bin_support/reader_images_fixture.rs`），预览和测试共用同一装配。

## 运行与验证

```sh
cargo run --example reader_images
cargo test --locked --test reader_images
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
```

预览场景：Healthy（默认，相对/根路径/公开 HTTPS/延迟图片）、Extremes（宽图与高图）、Failure（404/文本文件/file: 拒绝）、Oversize（编码超限与像素超限）、Long document（长文按需加载）、Switch（旧图片在途时切文档）。底部 action 栏显示当前文档与最近的 reader 事件；fixture 未建模的请求会显示为配置错误而不是假装权限结果。
