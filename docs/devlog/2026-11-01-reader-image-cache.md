# 2026-11-01 开发日志：README 图片持久缓存适配 (reader-image-cache)

## 任务背景与目标

为 Githux 的 README 图片阅读服务提供基于 `CacheStore` / `CacheScope` 的持久缓存能力：
1. **固定 commit 仓库相对图片**：在线读取后写入持久对象缓存；在重启 store 或断网（离线）后仍可读取，不同分支的新图按对应 blob SHA 隔离不串。
2. **外部动态徽章与资产**：识别 HTTP `Cache-Control`、`ETag` 与 `Last-Modified`；处于新鲜期直接复用，过期触发条件请求（带 `If-None-Match`/`If-Modified-Since`）；收到 304 Not Modified 复用本地字节并更新 `verified_at`；收到 `no-store` 失效旧条目并不再持久化。
3. **安全与隔离边界**：必须验证调用方传入的 `scope.account` 与 `session.context().auth().account` 一致；私有仓库 gate 严防越权，凭据拒绝（CredentialRejected）立即隔离私有缓存；外部请求严格无凭据无代理，不破坏已有 DNS/URL/重定向边界。
4. **非阻塞与容错**：单张图片 404、损坏或过大不影响兄弟图片与 README 正文的正常排版渲染；旧构造器 `RepositoryImageService::new` 保持 100% 向后兼容。

## 关键代码变动

- **`src/data/reader_image_cache.rs`（新增）**：
  - `ReaderImageCache`：封装面向 `RepositoryImageService` 的持久读写契约。
  - `ExternalFreshness`：计算外部资源是否处于 `max-age` 新鲜期。
  - 固定 blob SHA 使用命名空间 `reader-image-bytes-v1`；外部 URL 使用 `external-image` mutable key。
  - 304 更新验证时间、`no-store` 失效、私有属性注入。
- **`src/data/reader_images.rs`**：
  - 增加 `RepositoryImageService::with_cache` 构造器，支持注入 `Option<(CacheStore, CacheScope, bool)>`，校验账户匹配；保持原 `new` 向后兼容。
  - `repository_bytes` 优先查询持久缓存；若命中且未超限则直接解码；网络读取成功后异步入库。
  - `external_bytes` 优先查询持久缓存；未过期直接复用，过期时发送带条件头的请求并在 304 时 revalidate。
- **`src/network/reader_images.rs` 与 `src/network/reader_images_tests.rs`**：
  - `ExternalImageBytes` trait 拓展 `load_with_metadata` 方法，接收 `ValidationHeaders` 并返回 `ExternalImageResponse`。
  - `ReqwestExternalImages` 实现 304 识别与 ETag/Last-Modified/Cache-Control 头提取，保留手动逐跳重定向与 local DNS 拦截。
  - 单元测试验证 If-None-Match 头部发送与 304 Not Modified 响应处理。
- **`src/data/reader_images_tests.rs` 与 `src/bin_support/reader_images_fixture.rs`**：
  - 更新测试与 fixture 的 memory 外部图片源，支持条件请求及 metadata 提取。
  - 新增测试：
    - `fixed_revision_images_persist_across_reopened_store_and_service`（在线读→drop→离线重启读通过）
    - `external_badges_honor_freshness_revalidation_and_no_store`（304 续期及 no-store 失效）
    - `branch_switch_or_other_repo_does_not_mix_cached_images`（不同仓库/分支 blob 隔离）
    - `private_cache_isolation_and_quarantine_rejection`（凭据拒绝后私有图片读取被隔离）
    - `missing_images_fail_without_blocking_other_images_or_context`（坏图/缺图诚实降级，不阻塞好图）

## 遵守与应用教训

- **L002（模拟验证写清边界）**：明确说明本次交付针对 `RepositoryImageService` 与数据层持久缓存契约，正式 Shell 尚未挂载离线阅读；所有测试边界、离线行为及真实 socket 验证均独立记录。
- **L005（取消语义）**：保留了现有的 `CancellationToken` 及超时处理，无论持久读写还是网络请求均受取消信号保护。
- **L007（安全判定与 URL 解析）**：外部请求及条件请求保持使用同一个 `reqwest::Url` 解析器，重定向每一跳均核验。
- **L010（fixture 支持反复操作）**：更新 `MemoryExternalImages` 保证同一 URL 多次请求、条件请求及反复切换场景稳定应答。
- **L014（可选内容缺失不是错误卡片）**：缺图或 404 返回 `ImageFailure::NotFound`，由阅读器渲染为诚实图注或占位，不阻塞正文排版，不产生全屏错误。

## 审查修复轮次

针对 Codex 审查提出的两项反馈完成修复：
1. **持久化固定 commit/path 至 blob 的解析映射**：
   - 增加 `ReaderImageCache::read_resolved_blob` 和 `put_resolved_blob`，使用 `reader-image-resolution` mutable key 保存 commit/path 到 blob SHA 的映射关系；
   - 在 `repository_bytes` 中离线先查询该解析关系，命中后直接读取已缓存 blob 字节，使完全断网/重启服务后无需重新请求 GitHub 树/提交即可展示固定图片；
   - 更新测试 `fixed_revision_images_persist_across_reopened_store_and_service` 为第二轮完全零网络响应断言。
2. **在网络请求发起前领取写票据（Write Ticket）**：
   - `ReaderImageCache` 增加 `begin_write_external` 与 `with_ticket` 写入方法；
   - 在 `RepositoryImageService::external_bytes` 及 `repository_bytes` 中，在 `await` 远程响应之前先领取 ticket，入库时传入该 ticket；
   - 保证 `CacheStore` 的并发代次和 `clear()` 失效机制正常运作，阻止并发旧结果覆盖较新快照。
3. **HTTP 缓存新鲜度优化与 404 代次防护**：
   - 支持 `must-revalidate` 在未过期（`max-age` 新鲜期内）直接复用缓存，仅对 `no-cache` 或过期条目强制网络验证；
   - 支持解析 upstream `Age` header，将响应已存在的时间纳入新鲜期计算，并独立保存以确保满足 `verified_at_ms >= fetched_at_ms` 存储不变量，防止 CDN 延迟交付导致本地误判为完整满期；
   - 在 HTTP 响应头提取阶段使用 `headers.get_all(CACHE_CONTROL)` 合并所有 Cache-Control 值，避免分散指令遗漏 `no-store`；
   - 404 失效（`invalidate_external_image_with_ticket`）改用在请求发起时领取的 write ticket 执行带有 no-store 语义的条件写入，防止慢速/过期的 404 响应误删后续并发请求刚刚缓存的较新图片。

## 验证结果

所有 Cargo 验证均严格通过 `node scripts/verify.mjs -- cargo ...` 运行：
- `cargo test --locked --test reader_images`：624 项测试全部通过。
- `cargo test --locked --bin githux network::reader_images::tests`：全部通过。
- `cargo test --locked --bin githux data::reader_images::tests`：全部通过。
- `cargo check --locked --all-targets`：通过。
- `cargo clippy --locked --all-targets -- -D warnings`：通过（零警告）。
- `cargo fmt --all -- --check`：通过。
