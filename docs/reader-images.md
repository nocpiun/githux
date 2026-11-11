# README 图片加载与显示

更新：2026-11-11（`reader-svg` 返工）。README 图片通过既有账户/provider/cache 通路读取；SVG 支持有界静态渲染与有限的一次性 CSS 入场时间线。正式 Repository 与 Profile 的接线见[正式页面接入](shell-page-integration.md)。本文记录解码、授权、预算、缓存及 Reader 呈现的实际边界；人工原生验收不由自动化通过替代。

## 分层与文件

| 层 | 文件 | 职责 |
| --- | --- | --- |
| 图片契约与预算 | [`src/content/images/`](../src/content/images/mod.rs) | `ImageLimits`、`ImageTarget`、`ImageKey`、`ImageScope`、`ImageProvider`、`ImageSink`、格式嗅探与有界解码。无 I/O，无 GPUI |
| 无凭据外部读取与条件请求 | [`src/network/reader_images.rs`](../src/network/reader_images.rs) | `ExternalImageBytes`、`ValidationHeaders`、`ExternalImageResponse` 与生产实现 `ReqwestExternalImages`：仅 HTTPS、手动逐跳验证重定向、总超时、流式字节上限、If-None-Match/If-Modified-Since 条件头与 304 处理 |
| 图片持久缓存存储适配 | [`src/data/reader_image_cache.rs`](../src/data/reader_image_cache.rs) | `ReaderImageCache` 与 `ExternalFreshness`：基于 `CacheStore`、`CacheScope` 与 `CacheKey` 的持久适配，固定 blob SHA 命名空间与外部 URL 独立键管理、`max-age` 新鲜度判断、304 时间戳更新与 `no-store` 失效 |
| 固定 commit 的仓库图片与缓存编排 | [`src/data/reader_images.rs`](../src/data/reader_images.rs) | `RepositoryImageService`：复用已捕获的 `RepositorySession` 的 revision/directory/blob 能力，支持可选注入 `(CacheStore, CacheScope, is_private)`，优先从持久缓存读取，网络回包异步入库，不持有 PAT |
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

常见 HTML `<img>` 经受限 HTML 解析转换为同一图片模型，保留正整数 `width` / `height` 提示，不执行 HTML 页面。相邻图片在同一纯图片段落内保留为 `ImageRow`，空行分隔各行；`align="center"` 容器保留居中作用域。含文字与图片的混合段落仍有下文注明的原位置限制。

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
- 生产构造只接受 `https`；无 userinfo；拒绝 loopback/`localhost`/未指定/链路本地/私网（含 10/8、172.16/12、192.168/16）与 100.64/10 共享地址。IPv4-mapped IPv6（`[::ffff:127.0.0.1]`）先归一化为 IPv4 再套用同一策略，IPv6 链路本地（`fe80::/10`）、site-local（`fec0::/10`）与 unique-local（`fc00::/7`，内网地址段）也拒绝，避免换一种拼写绕过本地来源边界；同一函数用于首跳和每个重定向 `Location`。（`file:`、`data:`、`javascript:` 等在进入 URL 校验前就已被 reader 拒绝。）
- `Policy::none` 后**手动逐跳**重定向：用与连接方相同的 `reqwest::Url` 解析和 `join`，每一跳重新校验 scheme/host/userinfo，拒绝本地来源；超过 `max_redirects` 报 `TooManyRedirects`，不把 3xx 当成功。这避免了把凭据送到重定向目标（本路径根本没有凭据，但本地来源校验不能省）。
- 总超时覆盖连接、每一跳和整个 body；`Content-Length` 只做提前拒绝提示，实际字节在流式累计时限制，超限即 `TooLarge`，不会先缓冲完整超大 body。
- 测试构造 `with_local_http()`（`#[cfg(test)]`）允许 loopback `http`，用真实 socket 验证「请求里没有 Authorization/Cookie/Proxy-Authorization」「同源重定向跟随」「file/data 重定向不接触目标」「重定向循环有界」「Content-Length 缺失的流式上限」「停顿超时」。

仓库图片的 PAT 只经过既有 `read_authenticated` 链路；外链不触碰 `GitHubClient` 的认证头。测试不读取真实用户 token。

## 工程预算

`ImageLimits` 的默认值与行为：

| 参数 | 默认 | 拒绝点 |
| --- | --- | --- |
| `max_response_bytes` | 5 MiB | 接收时（blob JSON/外部流式）与解码前双重检查，超限 `TooLarge`，不先缓冲 |
| `max_decoded_bytes` | 32 MiB | 解码前按 `width*height*4` 预算，另作为 `image::Limits::max_alloc`（SVG 同时约束光栅化 pixmap 尺寸；编码后的 PNG 输出字节也受 `max_response_bytes` 约束） |
| `max_pixels` | 8,000,000 | 先读 header 或 SVG `width`/`height`/`viewBox` 得到尺寸，超过即 `TooManyPixels`，在危险分配之前 |
| `max_dimension` | 8192 | 同上，单边上限，同时设置 decoder 严格维度上限 |
| `total_timeout` | 20 s | 一次完整加载的**总**截止时间：覆盖提交/每一级目录/blob 多个请求、外链重定向与 body，以及解码/光栅化；超时 `Timeout` |
| `max_concurrent` | 3 | 每个 provider 同时执行的加载数，semaphore 限制 |
| `max_in_flight` | 12 | reader 已交给 provider 但未完成的请求上限；其余按可见顺序排队 |
| `max_ready_images` | 32 | 每个 reader 当前文档保留的已解码图片数，超出按最老释放 |
| `max_ready_bytes` | 64 MiB | 按 `w*h*4` 估计的保留预算，超出同样释放 |
| `max_redirects` | 5 | 外链重定向上限 |

拒绝发生在危险分配之前：`decode_image` 先嗅探栅格格式，再只读 header 的尺寸，预算检查通过后才在 `image::Limits` 下完整解码验证；PNG/JPEG/WebP/GIF 由固定 `image` 依赖支持。对 SVG/XML 负载，由 `is_svg_like` 识别后走有界静态 SVG 光栅化：

- 使用工作区已锁定的 `resvg` / `usvg 0.46.0`（关闭 `system-fonts` 与 `memmap-fonts`，仅启用 `text` 与 `raster-images`），并将应用内置的 `Githux UI`（Noto Sans SC 衍生字面）与 `Google Sans Code` 注入私有 `fontdb`，不扫描系统字体、不读取磁盘字体或资源文件。
- 内嵌 `<image>` 只允许 `data:`。SVG logos 经过与顶层相同的 XML/主动内容校验，最多四层资源解析；内嵌 SVG 自身的时间线只取最终值，不另开子资源播放器；节点总额 50,000、输入数据总额 2 MiB，光栅资源先检查真实文件头尺寸，并累计像素及解码/绘制缓冲成本。GIF 先在 `image::Limits` 下解码首帧并转为 PNG，防止首帧网格大于逻辑屏幕头时绕过预算。超限或无效内嵌资源跳过，外层可独立呈现的内容保留。
- 禁用 XML DTD（`allow_dtd: false`，阻止实体展开炸弹）、限制 XML 节点数（`50,000`），并在调用递归 XML 解析器之前用有界扫描拒绝超过 `256` 层的嵌套（XML 没有隐式 void 元素：`<rect>` 只有写成 `<rect/>` 才算自闭合；扫描会跳过注释、CDATA、处理指令与 DOCTYPE，并尊重引号内的属性值，因此 `a="/>">` 之类的值不会被误判为自闭合），避免深度树在解析阶段栈溢出；同时禁用 `<image>` 外链文件/网络解析（`resources_dir = None`，`resolve_string` 恒返回 `None`，仅允许上述有界内嵌 data URI）。
- 显式拒绝 `<script>`、事件属性（`on*`）、`<foreignObject>`、`@import` 样式与 SMIL 动画元素（`<animate>`/`<animateTransform>`/`<animateMotion>`/`<set>`），返回 `ImageFailure::Unsupported`；畸形 XML、零尺寸或缺少固有尺寸/`viewBox` 的 SVG 返回 `ImageFailure::Malformed`。
- 在分配像素缓冲前按 `ImageLimits` 检查逻辑尺寸、像素数与解码字节预算，并按最大 2x（受像素/尺寸上限钳制）光栅化、反预乘后编码为 PNG，以 `DecodedImage` 的既有四个字段（`format: ImageFormat::Png`、`width`、`height`、`bytes`）返回最终 PNG（保留 SVG 固有 / `viewBox` 宽高，不增加 `Svg` 格式变体）；可选 `animation` 元数据携带有限 PNG 帧、已解码 BGRA8 像素及时间戳，UI 不在每帧启动异步 PNG 解码。`DecodedImage::decoded_bytes()` 按 PNG 实际网格与保留帧数计费，淘汰时也按同一实际成本扣除。
- 外部 HTTPS 请求 `Accept` 头包含 `image/svg+xml`（`image/png,image/jpeg,image/webp,image/gif,image/svg+xml,*/*;q=0.8`），不附加凭据或代理。

## 有界声明式 SVG 时间线

`resvg` 本身不执行 CSS 时间线。`content/images/timeline.rs` 在后台对 XML 节点匹配受支持的 CSS 规则，解析自定义属性及 `var()` fallback，计算每个时刻的属性，再交给 resvg 光栅化；它不执行脚本，也不通过修改所有 `opacity=0` 来显示内容。

- 支持标准 `@keyframes` 的 `from` / `to` / 百分比停点，单条一次性 `animation` shorthand 或 name/duration/delay/timing-function/fill-mode longhand，毫秒/秒（包括负 delay）、`none` / `forwards` / `backwards` / `both`、linear/ease/ease-in/ease-out/ease-in-out。
- 插值属性为 opacity、fill-opacity、stroke-opacity、stroke-dashoffset 和仿射 transform；已验证样本使用 translate/scale。静态 transform-origin 在转为 SVG transform 时保留。静态隐藏和 `!important` 属性保持其优先级。
- 播放最多五秒、最多 100 个采样帧（约 20 fps），动画使用固有网格以限制内存，所有帧与最终 PNG 共享 `max_response_bytes` / `max_decoded_bytes` 总额。支持的时间线过长或帧总额超限时保留计算后的最终 PNG；这是静态 fallback，不作为动画验收证据。
- 评估的 CSS ≤ 128 KiB，选择器/节点比较 ≤ 2,000,000，规则/变量/停点展开各受 2 MiB 预算约束。未加载的 `@font-face` / 条件规则不进入评估预算；不读取网络/文件字体。多条动画、重复/无限播放、其他动画属性及浏览器条件 CSS 不属于本次支持范围，保持基础呈现，不声称完整 CSS 浏览器兼容。
- `workspace.rs` 在绘制前估算 resvg 的画布、内嵌图片、隔离层和滤镜工作面；超采样不满足预算时退回固有网格，再超限则拒绝。会分配未受 resvg 画布约束的 pattern tiles 暂不支持。
- Provider 在每帧准备之间检查取消与总截止时间；账户/session 发布复核及 `no-store` 路径保持原有约束。持久缓存仍存原始响应，不存由当前账户以外继承的动画授权。

Reader 复用既有 pump 选择单帧、已解码的 `gpui::RenderImage`，从实际可见绘制开始计时；只重绘，不因中间帧重测尺寸。最终帧到达后释放其他帧并停止计时；reduced motion、窗口/页面暂停及超过 150ms 未绘制可见交集时同样收尾为最终 PNG。文档/账户替换按代次丢弃整组旧帧。静态图片不产生新的持续工作。

GIF 的原有独立播放语义不变；SVG 的有限时间线不使用 GPUI 会持续循环的多帧 `Img` 路径。


## 显示、尺寸与滚动

- 保留 #81 的固有/作者请求尺寸、列宽适配和透明成功路径：不放大、不强加背景或圆角；图片绝对定位在其实际比例盒内。小徽章的外盒按图片本身宽度参与行布局，高图保持原有尺寸语义，不恢复旧的 640px / 比例钳制。
- Ready 图片不强制显示 alt 图注；alt 保留在文档文本/失败信息中。Loading/Failed/Refused/无 provider 的状态仍给出与该资源相关的信息。
- 未知尺寸（加载中/失败/拒绝）预留 140px 占位高度；图片完成后对该顶层块调用真实 `ListState::remeasure_items`，以**锚定当前滚动位置**的方式重测，不重排版整篇文档，不插入假 AST 项，不把文末 inset 挤出测量范围，不因图片落地产生全文档高频重绘。在 `MarkdownLayout::Flow`（自然高度）下改为请求一次重绘并由 `on_layout_changed` 通知宿主，宿主的页面容器随之增长。
- 跨块选择/复制不受影响：图片块的 alt 仍在 `DocText` 中，图片本身不注册字形选区；选区坐标来自段落真实排版，不因图片尺寸变化而错位。表格内图片仍受 AST 限制（见下）。滚动条继续由共享 `Motion`/`Bar` 读取同一个真实 `ListState`。
- 同一纯图片段落（包括软换行间隔的链接徽章）保留为可换行的 `ImageRow`，空行另起段落；极长图片段落按最多 64 张图片切成有界组，跨片段居中 wrapper 最多保留 256 层。居中容器中的段落/图片行保持居中，但渲染前仍按原段落拆成独立虚拟项，不让整篇 Profile 成为一个无界行。混合文字和图片的段落仍将图片提取到文字后，未声称支持文字行盒中原位置嵌图。

## 生命周期与取消

- 一个 reader 的每次文档代次有一个 `ImageScope`；`show`/换 provider/换预算推进代次并取消旧 scope，旧完成一律丢弃。reader 的 pump 在 GPUI 执行器上以轻量节拍（20ms）驱动，**有工作才运行，结清即停**，不做跨线程唤醒，不做永久后台轮询，不在 render 中发 I/O。
- 文档替换、账户变化、窗口隐藏/关闭都会取消无用工作；provider 内部对每个 scope 一个取消源，Drop 时取消全部。取消的 scope 不再向 sink 发布完成。
- 两个 reader 显示同一图片时，各自有独立的 board、请求、取消和保留预算。即使宿主共享一个 provider，`ImageScope.reader` 也把工作分开（专项测试覆盖 `cancel` 只取消一个 scope）。
- 授权失败/过期在数据层按 `StaleContext` 报告；reader 不因此保留旧授权图片，不制定私有内容移除或授权失效保留政策，不新增磁盘缓存。

## 支持边界（明确未覆盖）

- **表格单元格内的图片不加载**：AST 在 cell 中没有块列表可挂图片，现有行为是保留 alt 文本在单元格内（见 `ast.rs` 的 `in_cell` 分支）。这不属于「已覆盖表格图片」。
- 段落中的 inline 图片被提取为段落后的图片块；不是文字行内原位置。
- 静态 SVG 经有界光栅化支持（典型徽章、统计卡片、Star History 折线图与常见文本/渐变/滤镜/裁剪）；**`<foreignObject>`、`<script>`、`on*` 事件、SMIL 动画、`@import` 样式、XML DTD/实体展开、外部 `<image href="https://...">` / 本地路径、WOFF/TTF `@font-face` 自定义网络字体**均不支持或按边界拦截/不抓取；AVIF、BMP、TIFF、ICO 仍为 `Unsupported`。
- 注意区别**公开样本响应可达性**与**离线解码验证**：本轮已实地核查公开徽章与 Star History SVG 响应形态（见 `docs/devlog/2026-11-10-reader-svg.md`；`github-readme-stats` 公开端点当时返回部署暂停文本而非 SVG），但所有自动化回归均基于确定性本地 SVG fixture，不依赖外部网络。
- 不做跨账户/跨实例的持久授权复用；动态 badge 仍遵循 HTTP 缓存新鲜度与重验证规则。
- 真实 Windows 窗口的原生验收（DPI、动画、IME、真实输入分发）以预览观察和后续负责人验收为准，测试平台不能替代。

## 持久缓存适配与后续正式接线契约

图片服务提供旧 constructor 与支持持久缓存的新 constructor：
```rust
// 旧无持久缓存 constructor（保持完全向后兼容）
RepositoryImageService::new(session, repository, executor, external, limits);

// 支持可选注入 CacheStore 与 CacheScope 的新 constructor
RepositoryImageService::with_cache(
    session,
    repository,
    executor,
    external,
    limits,
    cache_opt, // Option<(CacheStore, CacheScope, bool)>
);
```

1. **账户验证隔离**：注入的 `CacheScope` 会与 `session.context().auth().account` 进行一致性校验，不匹配则拒绝挂载持久缓存；私有属性 `is_private` 由调用方明确提供，不凭 SHA 穿透鉴权。
2. **仓库图片对象缓存**：固定 commit 的图片按 `BlobIdentity` 解析得到的 blob SHA 存入固定对象缓存命名空间 `reader-image-bytes-v1`。重启 store 后离线读取直接命中；新分支新图使用独立对象 SHA，互不干扰。
3. **外部图片与徽章重验证**：外部图片按 URL 建立可变条目。包含 `Cache-Control: max-age=...` 的资产在新鲜期内本地命中，过期或 `no-cache` 时携带 `If-None-Match` / `If-Modified-Since` 发送条件请求；服务端返回 304 时原子更新验证时间并复用原字节；返回 `no-store` 时立即删除旧缓存并阻止落盘。
4. **缺失降级与错误处理**：缺失图片直接以 `ImageFailure::NotFound` 返回占位，其余图片和正文文档正常解码并渲染；`CredentialRejected` / `AccessDenied` 时安全降级。
5. **正式 Shell 接线**：后续由协调者安排正式 Shell 离线接线任务，统一在 `RepositoryPage` 创建 `RepositoryImageService` 时将账户管理层获取的 `CacheStore` 与当前激活账户的 `CacheScope` 注入。

## 运行与验证

```sh
cargo run --example reader_images
cargo test --locked --test reader_images
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
```

预览场景：Healthy（默认，相对/根路径/公开 HTTPS/延迟图片）、Extremes（宽图与高图）、Failure（404/文本文件/file: 拒绝）、Oversize（编码超限与像素超限）、Long document（长文按需加载）、Switch（旧图片在途时切文档）。底部 action 栏显示当前文档与最近的 reader 事件；fixture 未建模的请求会显示为配置错误而不是假装权限结果。
