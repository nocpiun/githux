# README 静态 SVG 支持 (`reader-svg`)

- 日期：2026-11-10（Asia/Shanghai）。任务 ID `reader-svg`，分支 `fix/reader-svg`，base `0c619070be103048f9aa033f2f57520372cbebb2`。PR 链接待创建后补充。
- 目标与状态：实现中（本地代码与文档已完成，待运行验证与提交 PR）。修复用户问题 5：Markdown 阅读器中的静态 SVG 徽章、GitHub 统计卡片与 Star History 图表经现有授权图片管线与缓存路径光栅化显示，而非报错 `unsupported picture format`。

## 改动与决定

### 1. 光栅化库选型与依赖

- 选用 `resvg = "0.46.0"`（`default-features = false`，仅开启 `text`、`raster-images`），与 `Cargo.lock` 中 `gpui-pre 0.3.10` 已锁定的 `resvg 0.46.0` / `usvg 0.46.0` / `tiny-skia 0.11.4` 完全一致，未引入版本分叉。
- 关闭 `system-fonts` 与 `memmap-fonts`，把应用已打包的 `Githux UI` Regular/Bold（Noto Sans + Noto Sans SC）与 `Google Sans Code` 注入进程内懒加载的 `usvg::fontdb::Database`，并将通用字体族映射到应用内置字体；500/600 字重由 fontdb 选最接近的常规/粗体面。SVG 解码不读取磁盘字体目录、不使用外部资源目录（`resources_dir = None`）。

### 2. 公开样本形态核查与确定性测试边界

2026-11-10 实地核查公开 SVG 响应特征（仅用于确认真实响应形状，不作为自动化解码通过的依赖）：
- Shields.io 徽章（`https://img.shields.io/badge/build-passing-brightgreen`、`.../license-MIT-blue?style=for-the-badge`）：返回 `Content-Type: image/svg+xml;charset=utf-8`，带 `width`/`height`、`linearGradient`、`clipPath`、`filter`/`feGaussianBlur`、`shape-rendering="crispEdges"`、`<text transform="scale(.1)" textLength=...>` 与 `Verdana,Geneva,DejaVu Sans,sans-serif`。
- Star History 图表（`https://api.star-history.com/svg?repos=torvalds/linux&type=Date`）：返回 `width="800" height="533.333"`、`<style>` `@font-face`（`data:application/font-woff;base64,...`）、`feTurbulence` + `feDisplacementMap` 滤镜、路径/文本及内嵌 `data:image/jpeg;base64,...` `<image>`。
- `github-readme-stats.vercel.app` 公开示例当时返回 `DEPLOYMENT_PAUSED` 纯文本，因此统计卡片形态按经典 SVG `<?xml version="1.0"?>` + `viewBox` + `<style>` CSS 类 + `<text>` + `<circle stroke-dasharray>` 构造本地确定性 fixture。

### 3. 安全、预算与缓存兼容性

- **保持 `DecodedImage` 契约不变**：`src/content/images/decode.rs` 对 SVG 负载在 `ImageLimits` 预算检查后以最大 2x（受 `max_dimension` / `max_pixels` / `max_decoded_bytes` 钳制）光栅化为 PNG，并保留 SVG 固有 / `viewBox` 宽高（`width`、`height`）。外部 `DecodedImage`、`ImageFormat` 与所有 UI 渲染路径无需新增 `Svg` 分支。
- **有界性与拒绝语义**：
  - 响应字节上限检查先于一切 XML/SVG 解析；
  - 禁用 DTD/实体展开（`allow_dtd: false`）、节点上限 `50_000`（超限报 `TooLarge`）、嵌套深度上限 `256`（超限报 `Malformed`）；
  - 显式拒绝 `<script>`、`on*` 事件属性、`<foreignObject>`、`@import` 样式与 SMIL 动画元素（`<animate>`/`<animateTransform>`/`<animateMotion>`/`<set>`），报 `ImageFailure::Unsupported`；
  - 零尺寸、无固有尺寸/`viewBox`、非 UTF-8 或畸形 XML 报 `ImageFailure::Malformed`；
  - 逻辑尺寸与像素数先于 pixmap 分配校验（超限报 `TooManyPixels` / `TooLarge`）；编码后 PNG 字节受 `max_response_bytes` 约束。
- **无外部 I/O**：`options.resources_dir = None`、`resolve_string` 恒返回 `None`、`resolve_data` 仅允许有界（≤2 MiB）内嵌栅格 data URI（不允许嵌套 SVG data URI）。测试中用本地未连接文件与本地未 accept 的 `TcpListener` 验证解码过程不读本地文件、不发起网络连接。
- **网络与缓存路径**：`src/network/reader_images.rs` 将外部请求的 `Accept` 头补充 `image/svg+xml`；`RepositoryImageService` 沿用原有固定 commit 解析、授权复核、`no-store` 失效、私有仓库隔离与 `CredentialRejected` 隔离。

## 文件改动

- `Cargo.toml`、`Cargo.lock`：添加直接依赖 `resvg = { version = "0.46.0", default-features = false, features = ["text", "raster-images"] }`。
- `src/content/images/decode.rs`、`src/content/images/mod.rs`：实现有界静态 SVG 光栅化与 XML/特性安全校验，更新文档注释。
- `src/content/images/tests.rs`：保留原有栅格、预算与失败回归，更新 SVG 测试覆盖徽章、统计卡片、Star History 图表、XML 头/`viewBox`/BOM、畸形/DTD、超限、禁用特性及本地文件/网络不可达证明。
- `src/network/reader_images.rs`：外部请求 `Accept` 头加入 `image/svg+xml`。
- `src/data/reader_images_tests.rs`：新增端到端 provider + 持久化缓存 + 离线重开 + `CredentialRejected` 撤权隔离 SVG 回归。
- `docs/reader-images.md`、`docs/devlog/2026-11-10-reader-svg.md`：更新当前支持能力与边界文档。

## 验证

全部通过 `node scripts/verify.mjs` 串行执行（共享 Windows Cargo mutex），过程记录在 `.agents/task/runs/`：

| 命令 | 结果 | 运行记录 |
| --- | --- | --- |
| `cargo test --locked --test reader_images` | 通过，786 passed（最终代码） | `0bf680e2-33d6-428f-a108-417e84e79338` 内同一二进制 |
| `cargo test --locked`（全部测试目标，最终代码） | 通过：1234 / 98 / 598 / 292 / 258 / 786 / 884 / 44，0 failed | `0bf680e2-33d6-428f-a108-417e84e79338` |
| `cargo fmt --all -- --check` | 通过 | `5b557d86-bdcd-44e7-a8bc-313e1893b5cf` |
| `cargo clippy --locked --all-targets -- -D warnings` | 通过，零警告 | `d8e035d4-bfee-40e4-b66c-7ee67576c2ec` |
| `cargo check --locked --all-targets` | 通过 | `86595c65-1aac-4c75-aea2-67da6e8a8e46` |
| `cargo build --locked` | 通过，产物 `target/debug/githux.exe` | `f19b19cd-21ed-4906-81c1-19bcfc4563ff` |

新增/更新的回归测试：

- `content::images::tests::formats_are_detected_from_content_not_extension`：SVG/BOM/XML 声明/DOCTYPE 识别与 RSS 负例。
- `content::images::tests::static_svg_badge_stats_card_and_chart_rasterize_to_png_with_dimensions`：shields.io 形态徽章（gradient/clipPath/text/scale/textLength）、`<?xml?>` + `viewBox` 统计卡片（CSS 类/文本/环）、Star History 形态图表（filter/turbulence/displacement/内嵌 PNG data URI/路径/文本）均产出有效 PNG 并保留固有宽高，并断言实际绘制像素颜色。
- `content::images::tests::svg_unsupported_active_and_foreign_features_are_rejected_honestly`：`<script>`、`<foreignObject>`、SMIL `<animate>`、`on*`、`@import` 均返回 `Unsupported`。
- `content::images::tests::svg_malformed_and_dtd_payloads_are_rejected`：截断标签、零尺寸、无固有尺寸、DTD/实体、非 UTF-8 均返回 `Malformed`。
- `content::images::tests::svg_oversized_dimensions_pixels_and_node_counts_are_bounded`：超单边/像素/解码字节预算与深度超限均在分配前拒绝（深度超限在 XML 解析前由有界扫描先行拒绝，避免深度树导致栈溢出）。
- `content::images::tests::svg_external_file_and_network_references_cannot_read_local_files_or_fetch_network`：`<image href="file:///…">`、本地路径、`http://127.0.0.1…`、`https://127.0.0.1…` 均不被读取/抓取（渲染结果只有背景色，本地 `TcpListener` 无连接）。
- `data::reader_images::tests::svg_badge_stats_card_and_chart_load_through_provider_and_cache`：真实 `RepositoryImageService` + 持久化 `CacheStore` 路径下，外部徽章/统计卡与仓库内图表 SVG 经 provider 解码为 PNG；缓存重开离线读取仍成功；`CredentialRejected` 后私有仓库 SVG 读取返回 `StaleContext`。
- 既有 782 项 `reader_images` 回归（授权、取消、超时、预算、缓存新鲜度/304/no-store、私有隔离）继续通过。

同时把 `fixed_revision_images_persist_across_reopened_store_and_service` 与新增缓存测试的重开逻辑改为等待后台写入释放缓存锁（`Fixture::reopen_cache_store`），修复全量并行测试下 `CacheStore` 重开偶发 `InUse` 的既有竞态。

两次全量运行中出现与本改动无关的抖动：`app::pages::home::tests::authentication_failure_cancellation_and_uncached_pages_are_not_historical_success`（`e742c5f5` 同一套件通过，单独重跑通过，随后全量再次全绿）与 `network::transport::bounded_tests::chunked_and_no_length_stop_before_eof_on_success_and_errors`（`926780ed`，本机同时出现 GitHub TLS 握手失败与 loopback 请求 `error sending request`；单独重跑通过，最终全量 `9b0b60fa` 全绿）。按 L002 记录为负载/网络环境相关的既有测试抖动，不声称已修复也不归因于本任务。

## 审查修复轮次

第一轮 Codex 审查（head `35ed2fa4`，review 5838968518）提出 2 条 P2，均核实成立：

1. **超采样 PNG 未按实际像素网格计费**：`DecodedImage::decoded_bytes()` 原来只用逻辑 `width`/`height`，而 2x 光栅化后的 PNG 像素是 4 倍，reader 的保留预算/淘汰会低估内存。现在 `decoded_bytes()` 对 `ImageFormat::Png` 读取 PNG IHDR 的实际宽高再计算 `w*h*4`（普通 PNG 的两者相等），并新增断言「徽章 SVG 的 decoded_bytes 等于实际 PNG 网格大小且大于逻辑尺寸」。`DecodedImage` 字段未变，UI 仍用 `width`/`height` 做比例布局。
2. **非 ASCII 属性名切片 panic**：事件属性检查原来用 `name.len() >= 2 && name[..2].eq_ignore_ascii_case("on")`，遇到合法多字节属性名（如 `中`）会在非字符边界 panic，生产路径会被转成 `Network`。现在改用 `name.as_bytes()` 检查前两个字节，并新增回归 `svg_non_ascii_attribute_names_do_not_panic_the_decoder`。

第二轮 Codex 审查（head `91d24b8e`，review 5839212482）提出 2 条 P1，均核实成立：

1. **深度预扫描把图形元素当成隐式 void**：原实现把 `rect`/`path`/`image` 等当作自闭合跳过计数，但 XML 没有隐式 void 元素。`<rect>` 连续嵌套 300 层时预扫描只报 1 层，进入 roxmltree 的递归 tokenizer 后仍可能栈溢出。现在只豁免真正的 `/>` 自闭合写法；新增 `svg_xml_nesting_counts_every_non_self_closing_element`（300 层 `<rect>` 在解析前返回 `Malformed`）。
2. **内嵌栅格只限制压缩字节、未限制解码像素**：`<image href="data:image/png;base64,...">` 的 2 MiB 编码上限拦不住高压缩的超大画布（如 8192×8192 解码后 256 MiB），而 `usvg` 只包装字节、由 `resvg` 在渲染时才解码。现在 `svg_usvg_options(limits)` 在 `resolve_data` 中用 `image::ImageReader::into_dimensions()` 只读文件头，按 `max_dimension` / `max_pixels` / `max_decoded_bytes` 校验后才交给 `usvg`；超限的内嵌图被跳过。新增 `svg_embedded_raster_pixels_are_bounded_before_resvg_decodes_them`（9000×9000 声明画布 + 极小 payload 只渲染背景色）。

第一轮修复的校验（PNG 实际像素网格计费、非 ASCII 属性名不切片 panic）在本轮继续通过。

第三轮 Codex 审查（head `827a2e4`，review 5839445655）提出 1 条 P1，核实成立：

1. **深度预扫描不识别引号内的 `>`**：`<g a="/>">` 的 `/` 与 `>` 都在属性值里，但简单扫描把它当自闭合，20,000 层嵌套仍然只报 1 层并进入递归 XML 解析器。现在 `count_svg_nesting_depth` 是一个小型非递归扫描器：跳过 `<!-- -->`、`<![CDATA[ ]]>`、`<? ?>` 和 `<!DOCTYPE ...>`（含内部子集括号与引号），在开始标签内跟踪 `'`/`"` 引号状态，只在引号外把 `>` 当标签结束、并要求结束前最后一个非空白字节是 `/` 才判自闭合。新增 `svg_depth_preflight_ignores_tag_like_text_in_quotes_comments_and_cdata`：300 层 `<g a="/>">` 在解析前 `Malformed`，而注释/CDATA/引号中包含 `<g>`、`>`、`/>` 的合法小文档正常渲染。

## 运行与限制

- 构建产物：`target/debug/githux.exe`（`cargo build --locked`）。预览入口沿用 `cargo run --example reader_images`，但本任务未修改预览 fixture（归 `reader-image-layout`），因此预览仍只展示原有 PNG 场景；SVG 的生产路径证据来自数据层/provider 回归，不是外部网络可达性证明。
- 不声称原生视觉验收：本任务无 UI 改动，照片/徽章的实际排版、去灰底与拉伸由 `reader-image-layout` 负责并经负责人原生窗口验收。
