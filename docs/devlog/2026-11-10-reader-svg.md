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
| `cargo test --locked --test reader_images` | 通过，782 passed | `cb02cf16-95d7-4d03-9524-f5ab969696fa` |
| `cargo test --locked`（全部测试目标） | 通过：1230 / 98 / 594 / 288 / 254 / 782 / 880 / 44，0 failed | `e742c5f5-074b-44a6-90c8-cfc368a47f52` |
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

## 运行与限制

- 构建产物：`target/debug/githux.exe`（`cargo build --locked`）。预览入口沿用 `cargo run --example reader_images`，但本任务未修改预览 fixture（归 `reader-image-layout`），因此预览仍只展示原有 PNG 场景；SVG 的生产路径证据来自数据层/provider 回归，不是外部网络可达性证明。
- 不声称原生视觉验收：本任务无 UI 改动，照片/徽章的实际排版、去灰底与拉伸由 `reader-image-layout` 负责并经负责人原生窗口验收。
