# README 图片加载与显示

更新：2026-11-12（图片性能核心；Reader/table/Flow 的需求探针接线仍待 #86 稳定 checkpoint）。README 图片通过既有账户/provider/cache 通路读取；SVG 支持有界静态渲染与有限的一次性 CSS 入场时间线。正式 Repository 与 Profile 的接线见[正式页面接入](shell-page-integration.md)。本文记录解码、授权、预算、缓存及 Reader 呈现的实际边界；人工原生验收不由自动化通过替代。

## 分层与文件

| 层 | 文件 | 职责 |
| --- | --- | --- |
| 图片契约与预算 | [`src/content/images/`](../src/content/images/mod.rs) | `ImageLimits`、`ImageTarget`、`ImageKey`、`ImageScope`、`ImageProvider`、`ImageSink`、格式嗅探与有界解码。无 I/O，无 GPUI |
| 无凭据外部读取与条件请求 | [`src/network/reader_images.rs`](../src/network/reader_images.rs) | `ExternalImageBytes`、`ValidationHeaders`、`ExternalImageResponse` 与生产实现 `ReqwestExternalImages`：仅 HTTPS、手动逐跳验证重定向、总超时、流式字节上限、If-None-Match/If-Modified-Since 条件头与 304 处理 |
| 图片持久缓存存储适配 | [`src/data/reader_image_cache.rs`](../src/data/reader_image_cache.rs) | `ReaderImageCache` 与 `ExternalFreshness`：基于 `CacheStore`、`CacheScope` 与 `CacheKey` 的持久适配，固定 blob SHA 命名空间与外部 URL 独立键管理、`max-age` 新鲜度判断、304 时间戳更新与 `no-store` 失效 |
| 固定 commit 的仓库图片与缓存编排 | [`src/data/reader_images.rs`](../src/data/reader_images.rs) | `RepositoryImageService`：复用已捕获的 `RepositorySession` 的 revision/directory/blob 能力，支持可选注入 `(CacheStore, CacheScope, is_private)`，优先从持久缓存读取，网络回包异步入库，不持有 PAT |
| 阅读器图片状态 | [`src/ui/reader/images.rs`](../src/ui/reader/images.rs) | 每个 reader 每个文档代次的 `ImageBoard`：请求排队、在途预算、完成应用、淘汰、拒绝；只有 Ready 才绘制 |
| Markdown 块接入 | [`src/ui/reader/markdown/document.rs`](../src/ui/reader/markdown/document.rs) | `Block::Image` 的解析目标解析、状态查询，以及图片尺寸确定后的块重测 |
| 预览与测试装配 | [`src/bin_support/reader_images_modules.rs`](../src/bin_support/reader_images_modules.rs)、[`src/bin_support/reader_images_fixture.rs`](../src/bin_support/reader_images_fixture.rs)、[`src/bin_support/reader_images_view_tests.rs`](../src/bin_support/reader_images_view_tests.rs)、`examples/reader_images.rs`、`tests/reader_images.rs` | 真实流水线的确定性预览和验收测试，非产品二进制的一部分 |

Reader 原构造器、`show`、`set_markdown_layout`、`SourceView`、`ReaderAction`/链接回调、`ResourceContext` 与 2 MiB 正文边界保持不变。没有传 provider 的 Reader 仍然可以显示所有文档；合法但尚未请求的图片保持零高度，provider 附着后仍可调度，不会假报「已加载」。

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

- **不传 provider**：合法图片处于未请求状态，不绘制任何元素；不会登记无法执行的在途工作。缺少资源上下文、真实失败与拒绝同样不绘制占位或诊断（原因留在内部状态供测试与日志使用）；alt 继续进入复制文本。
- **传 `None` 或换 provider**：`set_image_provider` 先取消旧 scope 的在途工作，再清空未完成的请求；已完成的图片保留，但旧代次的完成不会发布。
- **每个 reader 一份 provider**：接口允许共享，但推荐一份文档代次一份 provider。`ImageScope { reader, generation }` 让两个 reader 即使 generation 数字相同也不会互相取消或接收对方的完成；`RepositoryImageService` 的快照/目录缓存只在它自己的实例内，SHA 相同不是跨账户的授权依据。
- **更换文档/账户/选择**：与现有 reader 规则一致，推荐换实体（或至少调 `show`）。`show` 推进图片代次、取消在途、清空旧图片与旧错误。账户失效时宿主在 `cancel_interactions` 的同时移除 reader；图片服务自己的 `verify()` 复核 session，旧授权下的字节不会变成当前文档的图片。
- **隐藏窗口**：`observe_window_visibility` 取消当前 scope 的在途工作、保留已完成的图片；恢复可见时下一帧按需重新请求。
- **显示密度**：`ImageProvider::start_with_options(request, sink, ImageRenderOptions)` 默认委托旧 `start`，现有 provider/`ImageRequest`/页面接口继续兼容；生产 `RepositoryImageService` 将 options 传入后台解码。Reader 初始化和每次绘制读取 `Window::scale_factor()`，有限有效比例归一到 1–4。已保留超采样网格或有限帧组的 SVG 在更高密度时取消旧 scope 并保留 Ready 图，按原始来源后台重建；原始栅格的网格不随 options 变化，不因密度刷新重载 PNG 或重启 GIF。完成后替换成本，不重播已展示的时间线；失败只释放旧图，不显示任何诊断。降回较低密度复用已有清晰网格。持久缓存仍存原始响应；原有新鲜度、授权、取消与 `no-store` 规则照常复核。
- **解码与绘制**：解码在 provider 的后台线程完成，UI render 不做网络、解码或阻塞 I/O。生产路径直接移交已验证的 straight-alpha BGRA 给 `gpui::RenderImage`；静态 PNG/JPEG/WebP 与静态 SVG 不再由 GPUI 重复解码，SVG 不再为生产绘制编码中间 PNG。GIF 与动画 WebP 在同一有界解码侧保留全部帧及原 delay，仍走 GPUI 的循环多帧播放，按所有帧真实像素计费；不会静默压成首帧。保留 GPUI 锁定实现的 EXIF 方向处理及原 reader 逻辑 header 尺寸。非 UI 的兼容 `decode_image*` 仍提供 encoded fixture 输出；持久缓存始终保存原始响应。

## 地址解析与边界

图片地址复用 `links::resolve` 的同一套纯函数边界，再转成 `ImageTarget`：

| 地址形态 | 结果 | 行为 |
| --- | --- | --- |
| `images/diagram.png`、`../`、`/docs/images/a.png` | `Repository { owner, name, commit, path }` | 当前 README 所在目录、固定 repo/commit；`..` 到根停止；`%XX` 严格解码，坏转义拒绝 |
| `https://…` | `External { url }` | 独立无凭据 HTTPS 通路 |
| `http://…` | `Refused(InsecureExternal)` | 外部图片仅 HTTPS，不静默降级 |
| `file:`、`data:`、`javascript:`、其他 scheme、`//host`、UNC/反斜杠/控制字符、空地址、`#fragment` | `Refused(..)` | 不请求、不执行、不读本地/SMB，与链接边界一致 |
| `![alt](file:///...)` | 同上 | 不请求、不绘制；alt 与源地址保留在文档模型，不会因为地址危险就丢弃文字 |

常见 HTML `<img>` 经受限 HTML 解析转换为同一图片模型，保留正整数 `width` / `height` 提示，不执行 HTML 页面。相邻图片在同一纯图片段落内保留为 `ImageRow`，空行分隔各行；`align="center"` 容器保留居中作用域。含文字与图片的混合段落仍有下文注明的原位置限制。

## 固定 commit 的仓库图片

`RepositoryImageService` 不发明新的读取方式：它复用 `RepositorySession` 的能力，在固定 commit 上还原图片的真实 Git 对象：

1. `RevisionSpec::Commit(sha)` → `RepoSnapshot`（响应 SHA 必须等于请求 SHA）；
2. `SnapshotDirectory::root` → 按路径段逐级 `session.directory`（每个 tree 的 SHA 与请求一致）；
3. 末段必须是 `TreeEntryKind::File`，`BlobIdentity::from_file` 得到同 commit 的 blob 身份；**符号链接、submodule 不解引用**，返回 NotFound；
4. 图片专用 `session.image_blob` 使用固定 Git Blobs API 的 `application/vnd.github.raw+json` 二进制表示，验证实际 Git blob SHA 与 descriptor size；通用 `session.blob`、text/content 消费方继续 JSON/Base64。注入 transport 不支持有界二进制或响应为 406/415 时回退原有有界 JSON；其余错误保留原分类，不换 host、branch 或源图。代表真实响应的媒体契约证据见[性能日志](devlog/2026-11-12-reader-image-performance.md)。

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
| `total_timeout` | 20 s | `start` 时固定完整截止时间，覆盖 admission/网络/CPU 队列、metadata、body、解码和有限 SVG 后续批次；取消覆盖排队 |
| `max_concurrent` | 3 | 同时远程读取数；本地缓存 I/O 与解码分别独立限制为 2，不占网络许可 |
| `max_in_flight` | 12 | reader 已交给 provider、尚未终结的请求上限；首帧发布不提前归还此计数，provider 同时限制有字节的完整加载数 |
| `max_ready_images` | 32 | 每个 reader 当前文档保留的图片数；优先保留实际可见/近可见及最近使用的资源 |
| `max_ready_bytes` | 64 MiB | 按实际 BGRA 缓冲累计静态网格、动画帧和最终图；临时解码/重采样、等待发布与 GPU atlas 回收另列成本 |
| `max_redirects` | 5 | 外链重定向上限 |

拒绝发生在危险分配之前：`decode_image` 先嗅探栅格格式，再只读 header 的尺寸，预算检查通过后才在 `image::Limits` 下完整解码验证；PNG/JPEG/WebP/GIF 由固定 `image` 依赖支持。对 SVG/XML 负载，由 `is_svg_like` 识别后走有界静态 SVG 光栅化：

- 使用工作区已锁定的 `resvg` / `usvg 0.46.0`（关闭 `system-fonts` 与 `memmap-fonts`，仅启用 `text` 与 `raster-images`），并将应用内置的 `Githux UI`（Noto Sans SC 衍生字面）与 `Google Sans Code` 注入私有 `fontdb`，不扫描系统字体、不读取磁盘字体或资源文件。
- 内嵌 `<image>` 只允许 `data:`。SVG logos 经过与顶层相同的 XML/主动内容校验，最多四层资源解析；内嵌 SVG 自身的时间线只取最终值，不另开子资源播放器；节点总额 50,000、输入数据总额 2 MiB，光栅资源先检查真实文件头尺寸，并累计像素及解码/绘制缓冲成本。GIF 先在 `image::Limits` 下解码首帧并转为 PNG，防止首帧网格大于逻辑屏幕头时绕过预算。超限或无效内嵌资源跳过，外层可独立呈现的内容保留。
- 禁用 XML DTD（`allow_dtd: false`，阻止实体展开炸弹）、限制 XML 节点数（`50,000`），并在调用递归 XML 解析器之前用有界扫描拒绝超过 `256` 层的嵌套（XML 没有隐式 void 元素：`<rect>` 只有写成 `<rect/>` 才算自闭合；扫描会跳过注释、CDATA、处理指令与 DOCTYPE，并尊重引号内的属性值，因此 `a="/>">` 之类的值不会被误判为自闭合），避免深度树在解析阶段栈溢出；同时禁用 `<image>` 外链文件/网络解析（`resources_dir = None`，`resolve_string` 恒返回 `None`，仅允许上述有界内嵌 data URI）。
- 显式拒绝 `<script>`、事件属性（`on*`）、`<foreignObject>`、`@import` 样式与 SMIL 动画元素（`<animate>`/`<animateTransform>`/`<animateMotion>`/`<set>`），返回 `ImageFailure::Unsupported`；畸形 XML、零尺寸或缺少固有尺寸/`viewBox` 的 SVG 返回 `ImageFailure::Malformed`。
- 在分配像素缓冲前按 `ImageLimits` 检查逻辑尺寸、像素数与解码字节预算。SVG 最终图目标为设备比例的 2 倍，入场帧目标为设备比例本身；受像素/单边/绘制工作面上限逐步钳制。`format: ImageFormat::Png` 保留 raster family 身份，`width`/`height` 是作者逻辑尺寸；生产输出为 `pixels` 或 `animation` 中的已解码 BGRA，兼容解码入口另提供真实高分辨率 PNG bytes。`DecodedImage::decoded_bytes()` 累计实际最终网格、静态缓冲或全部帧长度，结束、替换与淘汰使用同一成本；没有用逻辑尺寸低估 SVG 超采样或 GIF/WebP 帧组。
- 外部 HTTPS 请求 `Accept` 头包含 `image/svg+xml`（`image/png,image/jpeg,image/webp,image/gif,image/svg+xml,*/*;q=0.8`），不附加凭据或代理。

## 字体、滤镜与物理分辨率（2026-11-11 品质返工）

Star History 的手绘噪声保留。根 `[patch.crates-io]` 统一使用 `vendor/resvg` 的 0.46.0，修复 displacement 在调用方已转换 scale 后再次相乘的问题，并按各输入自己的网格/stride 采样，越界透明，避免小数滤镜区域或画布裁切让输入相差一像素时断言崩溃。负值符号与物理坐标缩放也保留；说明见该目录 `PATCH.md`。原 SVG 几何不改写。滤镜工作面仍计费，只有 SourceGraphic→Turbulence→Displacement 的受限两 primitive 图使用逐面计算的五个工作缓冲，其余图保持保守估算。

私有内存 fontdb 除内置字体外，允许最多八个 data `@font-face`：单字体输入 ≤1 MiB，顶层与内嵌资源共用 4 MiB 解包后预算；单字面的目录/名称记录各 ≤256，名称字符串累计工作量 ≤64 KiB，拒绝可独立膨胀 face 数的字体集合。WOFF1 逐表有界 zlib 解压、校验表 checksum 并重建 SFNT，保留 CSS family alias。TTF/OTF 使用同一预算，不扫描系统、不解析网络/文件字体。WOFF2、字体集合与浏览器条件字体不在范围。最小测试字体为已有 Google Sans Code 的 OFL 子集，来源/哈希见 fixture README；公开 xkcd 字体与 Profile 响应不提交。

1×–4× 回归分别检查逻辑尺寸不变、真实帧网格和最终网格、预算及稳定播放；窗口密度变化回归检查 scope 失效、保留旧图和成本替换。当前应用的 rem 变化只缩放文字，不改变作者 px 图片尺寸；没有另一个整体 UI zoom API。若以后引入会放大图片的 zoom，宿主须把有效设备比例乘以该放大量传入 options，不能只拉伸现有位图。高 DPI 极大卡片或全画布滤镜仍可能被预算限制到低于目标网格；这是实现预算边界，不等于作者源本身低清晰度。原生视觉与真实账户页面仍需负责人验收。

## 有界声明式 SVG 时间线

`resvg` 本身不执行 CSS 时间线。`content/images/timeline.rs` 在后台对 XML 节点匹配受支持的 CSS 规则，解析自定义属性及 `var()` fallback（基础属性和 keyframe stop 都使用各动画节点的继承变量环境；自定义属性先在声明元素上计算，子元素继承该计算值），计算每个时刻的属性，再交给 resvg 光栅化；它不执行脚本，也不通过修改所有 `opacity=0` 来显示内容。

- 支持标准 `@keyframes` 的 `from` / `to` / 百分比停点；相同 offset 先按源码顺序逐属性合并，随后构造插值区间；缺少 0% / 100% 时按该元素的基础属性补齐，单独的 `to` / `from` 同样可播放，不同属性按各自的停点插值。支持单条一次性 `animation` shorthand 或 name/duration/delay/timing-function/fill-mode longhand；shorthand 各组件独立参与源码顺序、specificity 与 important 级联，节点变量解析后才提取组件，省略值恢复 shorthand 默认值。毫秒/秒（包括负 delay）、`none` / `forwards` / `backwards` / `both`、linear/ease/ease-in/ease-out/ease-in-out。
- 支持显式单色根背景（将颜色 `background` shorthand 规范为 resvg 接受的 `background-color`），使白纸图表的黑色标签保持对比；没有声明背景的图片仍透明。顶层 SVG 的 opacity/display 在完整表面上应用一次；进入库前中和已记录的前景根 opacity，避免重复相乘。内嵌 SVG 用受限内部矢量 image 包装保留整体根透明度、分数固有尺寸与比例，新增节点和隔离层仍受既有预算约束。隐藏根的背景不泄漏，静态隐藏子元素不被改成可见。
- 插值属性为 opacity、fill-opacity、stroke-opacity（数字和百分比均支持）、stroke-dashoffset（数值或 px）和仿射 transform（含 none identity、translate/translateX/translateY、scale/scaleX/scaleY）；已验证样本使用 translate/scale。静态 transform-origin 在转为 SVG transform 时保留；普通 presentation transform 作为基础值保留，动画延迟前及 none fill 收尾后恢复。静态隐藏和 `!important` 属性保持其优先级。受支持的 cascade 计算后写入节点；`:root` 只在 selector token 位置适配为同等 specificity 的虚拟属性选择器，保留 `svg:root` 组合、源码顺序及 quoted attribute/declaration 内容；并移除原样式表，避免 usvg 对同一声明再次 cascade、覆盖采样值或根可见性。
- 播放最多五秒、最多 100 个采样帧（目标约 20 fps）。先为最终图保留至多四分之一解码预算（至少满足固有网格），再按设备密度估算可保留帧数；临时工作面、编码或网格一致性不满足时减半时间采样数，保留稳定空间网格。普通卡片保持动画，高 DPI/大图会减少帧数；低至两个入场样本仍不能满足预算、或时间线超过五秒时才保留计算后的最终 PNG。所有帧与最终图仍共享编码/解码/像素总额，静态 fallback 不作为动画验收证据。
- 评估的 CSS ≤ 128 KiB，选择器/节点比较 ≤ 2,000,000，规则/变量/停点展开各受 2 MiB 预算约束。变量向同一个有界输出追加，按剩余额度在申请/拼接前检查长度，不先构造指数增长的中间字符串；每次时间线构建共享 100,000 次展开步骤上限，空值也计费，每 128 步复核取消/截止时间，遍历节点时同样复核。条件规则不进入评估预算；`@font-face` 只支持下述有界 data 字体，不读取网络/文件字体。多条动画、重复/无限播放、reverse/alternate/alternate-reverse 方向（包括 shorthand）、其他动画属性及浏览器条件 CSS 不属于本次支持范围，保持基础呈现，不声称完整 CSS 浏览器兼容。
- `workspace.rs` 在绘制前估算 resvg 的画布、内嵌图片、隔离层和滤镜工作面；矢量 `<image>` 每级按当前目标画布计入额外表面，向内传播祖先缩放与隔离后的目标大小，不按资源固有 1×1 尺寸代替。滤镜 primitive 的完整区域与被 renderer 裁切的 group layer 分开计费，不能沿用 layer 的内部画布裁切来低估滤镜分配；mask/clip 引用链按同时保留的目标画布逐级累加，遍历所有引用内容并受节点/深度上限约束；clip 子组的裁剪工作面按完整目标大小计费。超采样不满足预算时退回固有网格，再超限则拒绝。会分配未受 resvg 画布约束的 pattern tiles 暂不支持。
- Provider 在每帧准备之间检查取消与总截止时间；账户/session 发布复核及 `no-store` 路径保持原有约束。持久缓存仍存原始响应，不存由当前账户以外继承的动画授权。

Reader 复用既有 pump 选择单帧、已解码的 `gpui::RenderImage`，从实际可见绘制开始计时；只重绘，不因中间帧重测尺寸。首帧到达后即可开始，后续批次按原时间戳加入而不重启时钟。最终帧到达后释放其他帧与 atlas tiles 并停止计时；reduced motion、窗口/页面暂停及超过 150ms 未绘制可见交集时同样收尾为清晰最终像素。文档/账户替换按代次丢弃整组旧帧。静态图片不产生持续工作；仅错误后仍有明确 SWR 许可的可见旧图保留一次到期唤醒。

GIF 的原有独立播放语义不变；SVG 的有限时间线不使用 GPUI 会持续循环的多帧 `Img` 路径。


## 显示、尺寸与滚动

- 保留 #81 的固有/作者请求尺寸、列宽适配和透明成功路径：不放大、不强加背景或圆角；图片绝对定位在其实际比例盒内。小徽章的外盒按图片本身宽度参与行布局，高图保持原有尺寸语义，不恢复旧的 640px / 比例钳制。
- Ready 图片不强制显示 alt 图注；alt 保留在文档文本中。**非 Ready 状态一律不绘制图片 UI**：未请求、排队、Loading、Failed/Refused、Unavailable 与 Released 都没有图片、alt/诊断标签、占位面板或背景。非 Ready（包括 Released）不预留图片尺寸/外边距/gap；淘汰后只保留原图尺寸元数据供恢复网格计算，不生成文档布局盒。失败原因只作为内部状态用于测试与调度。
- 需求由实际布局交集收集：Ready 上报图片几何，非 Ready（包括 Released）通过不增加子项/gap 的探针上报插入点和可用列宽。visible 优先于 near-visible，构建 table cell 不等于可见；实际测得离开范围时取消无用工作。虚拟列表缺少本帧测量不等于已离屏：已准入、几何仍未知的 Loading 请求保持有界需求，直到实际测得离开范围或 scope 失效。初次纯图片块仍高度归零，不能以正面积交集作为初次加载前提。
- 不再为 pending 状态预留 140px；图片完成后对该顶层块调用真实 `ListState::remeasure_items`，以**锚定当前滚动位置**的方式重测，不重排版整篇文档，不插入假 AST 项，不把文末 inset 挤出测量范围，不因图片落地产生全文档高频重绘。在 `MarkdownLayout::Flow`（自然高度）下改为请求一次重绘并由 `on_layout_changed` 通知宿主，宿主的页面容器随之增长。
- 跨块选择/复制不受影响：图片块的 alt 仍在 `DocText` 中，图片本身不注册字形选区；选区坐标来自段落真实排版，不因图片尺寸变化而错位。滚动条继续由共享 `Motion`/`Bar` 读取同一个真实 `ListState`。
- 同一纯图片段落（包括软换行间隔的链接徽章）保留为可换行的 `ImageRow`，空行另起段落；极长图片段落按最多 64 张图片切成有界组，跨片段居中 wrapper 最多保留 256 层。居中容器中的段落/图片行使用占满可用列宽的作用域，保留普通标题与正文的字号/间距，渲染前仍按原段落拆成独立虚拟项，不让整篇 Profile 成为一个无界行。混合文字和图片的段落仍将图片提取到文字后，未声称支持文字行盒中原位置嵌图。
- **表格单元格图片**：AST 在 cell 内没有块列表，图片不能像段落图片那样移到块后。单元格记录 `CellImage`（alt、URL、`width`/`height` 提示、alt 在 cell 文本中的字节范围、包住图片自己的链接）；`cell_parts` 把单元格拆成「文本段 / 图片」的阅读顺序，`DocText` 与渲染器遍历同一份拆分，因此一个 cell 可能登记多个单元而复制文本仍是原来的 cell 文本（alt 保留原位）。
  - 渲染复用同一 `ImageBoard`/`images::image_element` 状态与预算：仓库相对路径仍按固定 commit 解析，外链走同一条无凭据通路，SVG 同样经有界光栅化与有限时间线；图片按 cell 内容宽等比缩放（以逻辑尺寸为准，不用超采样像素网格），比 cell 窄时按列对齐落在内左/居中/内右。
  - 非 Ready（包括 Released）的 cell 图片不生成可见子元素、布局盒或额外行/gap，但插入点探针保留需求；落地时重测表格顶层块。恢复仅由新的实际需求 epoch 触发，不因每帧 `want` 重载。表头、边框与作者写的图注行照常保留。
  - 可见的表格 SVG 通过 `painted_when_visible` 上报 `painted`，与普通图片一样只有在真实可见交集时才起播并保持有限动画。
  - 图片绘制区域登记为该单元的真实命中目标，从图片起手的拖选从图片所在 cell 开始，而不是从文档起点；`[![alt](img)](link)` 的归属链接在解析时记录在 `CellImage::link`（空 alt 图片不能靠字节包含关系区分相邻链接），点击图片会像点击那段文字一样报告 `ReaderAction::Link`，而拖选经过图片会取消这次点击。

## 生命周期与取消

- 一个 reader 的每次文档代次有一个 `ImageScope`；`show`/换 provider/换预算推进代次并取消旧 scope，旧完成一律丢弃。reader 的 pump 在 GPUI 执行器上以轻量节拍（20ms）驱动，**有工作才运行，结清即停**，不做跨线程唤醒，不做永久后台轮询，不在 render 中发 I/O。
- 文档替换、账户变化、窗口隐藏/关闭都会取消无用工作；provider 内部对每个 scope 一个取消源，Drop 时取消全部。取消的 scope 不再向 sink 发布完成。
- 两个 reader 显示同一图片时，各自有独立的 board、请求、取消和保留预算。即使宿主共享一个 provider，`ImageScope.reader` 也把工作分开（专项测试覆盖 `cancel` 只取消一个 scope）。
- 授权失败/过期在数据层按 `StaleContext` 报告；reader 不因此保留旧授权图片，不制定私有内容移除或授权失效保留政策，不新增磁盘缓存。

## 支持边界（明确未覆盖）

- 段落中的 inline 图片被提取为段落后的图片块；纯图片段落保留为 `ImageRow`。表格单元格中的图片同样不在文字行内原位置：cell 内的文本段与图片按阅读顺序各占一行（`flex_col`），不在一行文字中间绘制。
- 静态 SVG 经有界光栅化支持（典型徽章、统计卡片、Star History 折线图与常见文本/渐变/滤镜/裁剪）；**`<foreignObject>`、`<script>`、`on*` 事件、SMIL 动画、`@import` 样式、XML DTD/实体展开、外部 `<image href="https://...">` / 本地路径、外部网络/文件字体、WOFF2 与字体集合**均不支持或按边界拦截/不抓取；AVIF、BMP、TIFF、ICO 仍为 `Unsupported`。
- 注意区别**公开样本响应可达性**与**离线解码验证**：公开徽章与 Star History SVG 响应形态已实地核查（见 `docs/devlog/2026-11-10-reader-svg.md`；`github-readme-stats` 公开端点当时返回部署暂停文本而非 SVG），但所有自动化回归均基于确定性本地 SVG fixture，不依赖外部网络。
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
3. **外部图片与徽章重验证**：外部图片按 URL 与既有图片表示建立可变条目。新鲜期内本地命中；过期后仅在响应明确给出尚未结束的 `stale-while-revalidate` 且无 `no-cache`/`no-store`/`must-revalidate`/`proxy-revalidate` 禁止时先发布旧像素，并在后台重验证。没有该许可则等待。计入 Age、Date apparent age 与响应耗时，不另加 TTL；旧版未记录这套 age 语义的条目保留原字节和 validator，先验证一次后再恢复 fresh/SWR 快速命中。304 必须仍对应本地同字节/validator/政策表示，否则一次不带旧 validator 的重取；可选 SWR 旧像素在解码排队/处理期间到期，或旧内容 malformed/unsupported/超出解码预算时，只要原始授权、取消和总截止仍允许，跳过旧像素并沿原总截止继续重验证；真正撤权仍终止。相同字节验证成功直接确认仍存在的像素和时间线；若 SWR 到期已撤回像素，则结束旧请求并按实际需求从已验证缓存恢复绘制，迟到的旧 terminal 不影响新请求计数。新 200 替换原始字节与元数据；`no-store`（包括失败响应/重定向）删除旧条目。SWR 错误仅保留至原许可到期，届时清掉可见副本；账户、授权和 clear 的失效票据可立即撤回已发布图。
4. **缺失降级与错误处理**：缺失图片直接以 `ImageFailure::NotFound` 返回，其余图片和正文文档正常解码并渲染；`CredentialRejected` / `AccessDenied` 时安全降级。失败只影响该图片是否绘制（不绘制），不影响正文排版与其它图片。
5. **正式 Shell 接线**：后续由协调者安排正式 Shell 离线接线任务，统一在 `RepositoryPage` 创建 `RepositoryImageService` 时将账户管理层获取的 `CacheStore` 与当前激活账户的 `CacheScope` 注入。

## 性能发布协议与接线检查点（2026-11-12）

`ImageSink::for_request` 分配本 board 内唯一请求 id；`ImageEvent` 同时带 scope、request、单调 phase 与 terminal。Reader 仅接受当前请求的下一个阶段，首个 Ready 不归还在途计数；正常终结或取消只归还一次。有限 SVG 先交付正确初始帧与已准备的最终备用图，随后每批至多两帧；更高 DPI、隐藏、取消及迟到事件不重启同一文档已结束的时间线。生产路径不编码中间 PNG。空间网格先与单帧可接受网格核对，预算不足优先减少时间采样，不能以分批发布为由降低接受的 SVG 清晰度。

解码许可随已排队的像素事件保留到 UI 消费，限制生产者跑在隐藏/繁忙 UI 前面继续堆积大缓冲；取消 sink 会移除该请求的排队事件并拒绝迟到发布。缓存 I/O 与 decode 的许可都移入实际 blocking worker，超时只结束 await，不会提前释放仍在运行的 worker 的配额。commit、directory、同源 bytes 及 blob 的在途合并只活在当前 `RepositoryImageService` 实例（已捕获 session）内；最后一个订阅退出才取消共享读，失败/取消不留永久条目。resolution 与 bytes 的 write ticket 都在相应远程工作前取得。

GPUI 的 `ImageSource::Render` 绕过 asset cache，`paint_image` 借用已有 BGRA 上传；`RenderImage` Arc 释放不会自动清除 atlas。Reader 在动画结束、替换、淘汰、文档/预算替换、隐藏和实体释放时显式安排 window 侧 `drop_image`；临时 retirement 列表去重，最终图继续保留，旧 tiles 随前台回收释放。64 MiB 是 reader 的 retained pixel 起点，不是整个进程或 GPU 驱动的 RSS 上限；原始响应、解码/旋转/重采样工作面、等待 UI 消费的至多两个 decode 作业和一个前台回收间隔的旧资源都属于另列的瞬时成本。实际测量及限制见[性能日志](devlog/2026-11-12-reader-image-performance.md)。

已在授权的 table/silent checkpoint `945698ddfe271979d3d95c2f2b3c7d6f16628983` 上连接普通行、table cell、Repository 与 Flow 的 prepaint 需求。测量包装转发原 layout id，不增加布局子项；静默探针不产生选择/链接命中区。Flow 根盒使用自然高度、禁止被固定滚动宿主压缩，viewport 取外层真实 mask；普通 Reader 使用自身与 mask 的交集。`options_for` 携带每张图的实际列宽与设备比例，逻辑尺寸继续来自原始 header/作者提示。

静态 raster 从原始字节按实际显示网格重采样，保留 EXIF 和 straight BGRA；透明边缘先以 16-bit premultiplied 通道滤波，再恢复 straight alpha，避免暗边。私有 `crates/image-resize` 仅提供两个非泛型像素内核入口，内部使用锁定的 `fast_image_resize 5.1.4` Lanczos3；dev/test 仅将这两个内核包设为 opt-level 3，使泛型实例化落在优化边界内。应用、原始 image decoder 与验证 runner 配置保持原样。加宽/DPI 增大时补齐网格；布局稳定后若宽度缩小至少 25%，回收明显过大的静态网格，小幅抖动不反复解码。SVG 与 GIF/WebP 时间线不进入这条缩小路径。Released 在 scope/DPI/恢复切换时保留原图尺寸元数据，布局仍归零，并允许一个新的需求尝试；稳定 viewport 不运行回收/重载循环。联合验证和原生验收边界见性能日志，PR #86 与性能工作均未据此视为负责人验收。

## 运行与验证

```sh
node scripts/verify.mjs -- cargo run --locked --example reader_images
node scripts/verify.mjs -- cargo run --locked --example reader_image_performance
node scripts/verify.mjs -- cargo test --locked --test reader_images
node scripts/verify.mjs -- cargo check --locked --all-targets
node scripts/verify.mjs -- cargo clippy --locked --all-targets -- -D warnings
```

原图压力预览 `reader_image_performance` 与联合测试共用九张生成的 2559×1520 原始 PNG、正文与真实账户/cache/provider/decoder fixture，不含实时网络或真实凭据。支持 Rows/Table × Reader/Flow、默认 64 MiB 与 retain-four 压力开关；可滚动返回、resize 或跨 DPI 拖动。关闭窗口在可执行文件旁写 `.metrics.json`，只含累计阶段计数、原图数量及 Windows 进程 working set/peak/private commit；明确不测独立显存。测试平台进程内存与原生 GPU 窗口的报告不能相互替代。

预览场景：Healthy（默认，相对/根路径/公开 HTTPS/延迟图片）、Extremes（宽图与高图）、Failure（404/文本文件/file: 拒绝）、Oversize（编码超限与像素超限）、Long document（长文按需加载）、Switch（旧图片在途时切文档）。底部 action 栏显示当前文档与最近的 reader 事件；fixture 未建模的请求会显示为配置错误而不是假装权限结果。
