# 开发日志：头像加载链路普通磁盘缓存适配（avatar-cache）

- 日期：2026-11-01
- 任务：avatar-cache
- 分支：feature/avatar-cache
- 基线：9c405942654237b95cceb10c0f8543ee1a6f06a5

## 任务背景与目标

正式 Shell 与侧边栏的头像（包括账户 footer、仓库 namespace 以及各页面 credited 用户头像）已具备内存解码缓存与 initials fallback，但此前没有磁盘缓存，冷启动重启或断网时无法展示此前已加载过的头像。

本任务目标：
1. 实现 `src/data/avatar_cache.rs`，基于 `CacheStore` 与当前查看账户的 `CacheScope` 提供头像持久化与条件请求适配。
2. 扩展 `src/app/avatar.rs` 的 `AvatarFetcher`，支持显式可选注入 `AvatarDiskCache` 以及携带响应元数据的 `AvatarSource`。
3. 处理 HTTP 304、no-store、敏感 URL 签名脱敏、损坏或超大文件 initials 兜底、以及账户退休后的隔离。
4. 保证磁盘 I/O 不在 UI 线程执行。

## 应用的相关教训

- **L007（安全判定与连接方解析器一致）**：使用 `reqwest::Url` 解析与清洗头像 URL，识别包含敏感参数（`token`, `sig`, `signature`, `auth` 等）并统一脱敏查询字符串，避免敏感凭据或临时签名存入可读键或日志。
- **L011（裁切与圆形呈现）**：现有圆形头像、首字母 tint 颜色及零面积空 mask 滚动绘制防护保持不变，不影响原有 UI 渲染行为。

## 实现细节

1. **`src/data/avatar_cache.rs`**：
   - 定义 `AvatarDiskCache` 结构体，包裹 `CacheStore` 与 `CacheScope`。
   - `avatar_cache_key` 使用 `AvatarKeyLike` 提取稳定的 `subject`（如 `github.com#1`）与经过 `sanitize_avatar_url` 清洗的 URL，生成 canonical `CacheKey`。
   - 提供同步阻塞方法：`read_sync`, `put_sync`, `revalidate_sync`, `invalidate_sync`，在后台 blocking worker 中执行。
   - `revalidate_sync` 在收到 304 时检查本地是否仍有匹配的有效表示，若有则更新其 `verified_at_ms`，否则不生成空数据。
2. **`src/app/avatar.rs`**：
   - 增加 `AvatarResponseMetadata` 与 `AvatarSource` trait，为头像 HTTP 请求支持条件头（`If-None-Match`, `If-Modified-Since`）。
   - 为生产客户端 `ReqwestAvatars` 实现 `AvatarSource`，同时保留原 `AvatarBytes` 兼容路径。
   - `AvatarFetcher` 增加可选注入方法 `with_cache` 与 `with_source`。
   - 在 `fetch` 过程中：
     - 若注入了 `AvatarDiskCache`，先在 `tokio::task::spawn_blocking` 中读取已缓存快照。
     - 联网请求携带已有 ETag / Last-Modified。
     - 若返回 304，续期本地快照并解码。
     - 若返回 200，检查 `Cache-Control: no-store`，若允许存储则写入磁盘缓存，若 no-store 则主动失效旧缓存。
     - 若断网或网络出错，回退到已读取的磁盘缓存快照并解码。
     - 无论来源是网络还是磁盘，均调用 `decode_image` 执行完整的 image 库像素解码校验，校验失败终态回退到 initials。
3. **确定性测试 `src/data/avatar_cache/tests.rs`**：
   - `avatar_populates_cache_and_reopen_hits_offline`：网络填充 → 重新打开 CacheStore/Fetcher → 断网离线命中并成功解码。
   - `avatar_same_url_updates_on_new_image_and_304_revalidates`：验证 304 续期及同 URL 换图时原子更新。
   - `avatar_no_store_invalidates_old_cache_and_is_not_persisted`：no-store 响应清除旧缓存且不持久化新图。
   - `corrupt_or_oversized_payload_falls_back_and_does_not_cache`：损坏/非图片字节及超 2 MiB 负载不会解码成功，安全 fallback。
   - `sensitive_url_query_parameters_are_sanitized`：敏感 URL 参数在 CacheKey 中脱敏。
   - `retired_account_scope_prevents_read_and_write`：账户退休后拒绝读取和回写。

## 验证记录

## Codex Review 修复反馈 (Round 1)

第一轮代码审查提出三项 P2 建议，已全部处理并补充回归测试：
1. **网络请求前捕获写入票据 (begin_write)**：在发送 HTTP 请求前调用 `begin_write_sync` 捕获 `CacheWrite` 票据，并在写入时使用 `commit_write_sync`。若下载期间发生缓存失效、清理或更新的写入，原有票据自动被判定为 `Stale`，彻底杜绝慢响应覆盖新数据或回填已清理缓存。
2. **校验图片解码后再写入磁盘**：在 `put` 进磁盘前先执行 `decode_image` 完整图像解码校验；若为损坏/非图片内容，拒绝入库，避免坏数据覆盖有效缓存或存储坏 validator。补充了 `corrupt_or_oversized_payload_falls_back_and_does_not_cache` 对磁盘无写入的断言。
3. **遵守离线回退的 Cache-Control 限制**：增加 `is_snapshot_reusable_offline` 检查，若缓存标有 `no-cache` 或 `must-revalidate`，离线网络不可用时拒绝回退旧图，安全退回 initials fallback。新增 `no_cache_and_must_revalidate_prohibit_stale_offline_reuse` 测试验证。

所有 Cargo 检查通过 `node scripts/verify.mjs -- cargo ...` 执行：
- `cargo fmt --all -- --check`：通过。
- `cargo check --locked --all-targets`：通过。
- `cargo clippy --locked --all-targets -- -D warnings`：通过（zero warnings）。
- `cargo test --locked`：全量测试套件通过（44 passed）。
- `cargo build --locked`：通过。
- `git diff --check`：无空白或格式异常。
