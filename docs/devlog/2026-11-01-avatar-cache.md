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

## Codex Review 修复反馈

### Round 1

第一轮代码审查提出三项 P2 建议，已全部处理并补充回归测试：
1. **网络请求前捕获写入票据 (begin_write)**：在发送 HTTP 请求前调用 `begin_write_sync` 捕获 `CacheWrite` 票据，并在写入时使用 `commit_write_sync`。若下载期间发生缓存失效、清理或更新的写入，原有票据自动被判定为 `Stale`，彻底杜绝慢响应覆盖新数据或回填已清理缓存。
2. **校验图片解码后再写入磁盘**：在 `put` 进磁盘前先执行 `decode_image` 完整图像解码校验；若为损坏/非图片内容，拒绝入库，避免坏数据覆盖有效缓存或存储坏 validator。补充了 `corrupt_or_oversized_payload_falls_back_and_does_not_cache` 对磁盘无写入的断言。
3. **遵守离线回退的 Cache-Control 限制**：增加 `is_snapshot_reusable_offline` 检查，若缓存标有 `no-cache` 或 `must-revalidate`，离线网络不可用时拒绝回退旧图，安全退回 initials fallback。新增 `no_cache_and_must_revalidate_prohibit_stale_offline_reuse` 测试验证。

### Round 2

1. **304 只续期条件请求所依据的表示**：移除 `revalidate_sync`（它会重新读取当前条目，把更新写入者的 body 与旧 ETag 拼在一起），改为 `commit_revalidation_sync(ticket, original_snapshot, …)`，复用请求前的 ticket 和请求时读取的快照；若期间有更新写入或失效，`CacheStore::put` 以 `Stale` 拒绝发布。

### Round 3

1. **先取 ticket 再读快照**：`begin_write_sync` 与 `read_sync` 同一 blocking 任务内按此顺序执行，使 clear/invalidate 落在两者之间时旧快照无法再借新 ticket 复活。
2. **no-store 也走带 ticket 的提交**：删掉无 ticket 的 `invalidate_sync` 分支，改由 `commit_write_sync` 交给 `CacheStore::put` 在票据校验后处理 no-store 删除，避免旧响应删掉新写入者已发布的条目和 slot。

### Round 4

1. **启用磁盘缓存时必须有响应元数据**：无 `AvatarSource` 时不再持久化任何东西（`has_metadata` 为假即跳过全部磁盘写入），避免 `ReqwestAvatars::load` 丢弃的响应头导致 no-store/no-cache 被忽略。

### Round 5

1. **回退前重核授权**：304 分支与离线回退分支在返回缓存字节前，都在 blocking worker 上重新 `read_sync`（`CacheLease::snapshot` 的授权复查），账户退休/凭据拒绝后不再把旧图片排进 inbox。
2. **合并所有 Cache-Control 字段**：抽出 `collect_cache_control` 用 `get_all` 拼接全部字段值，后续 `no-store`/`no-cache` 不会因先出现的 `max-age` 而丢失。

### Round 6

1. **缺失 Cache-Control 必须是 `None`**：`collect_cache_control` 在没有任何字段时返回 `None`，否则 304 会用空字符串覆盖原 `no-cache` 策略。新增 `cache_control_is_absent_only_when_no_field_is_present`（直接覆盖生产 header 提取路径）与 `a_304_without_cache_control_preserves_the_stored_policy`（存储策略跨 304 保持）。

### Round 7

1. **no-store 与图片校验解耦**：存储策略只由响应头决定。`no-store` 响应即使在 body 为空/损坏时也用同一票据提交，让 `CacheStore::put` 删除旧表示；损坏 body 仍然不入库也不渲染，返回 initials。新增 `no_store_with_an_unusable_body_still_invalidates_the_stored_avatar`。

### Round 8

1. **超大响应保留策略头**：生产 `ReqwestAvatars::load_detailed` 在 body 超限时不再 `Err(())` 丢弃状态/头，而是返回状态与元数据、丢弃 body，使 `no-store` 仍能通过同一票据失效旧缓存。新增 `an_oversized_streamed_body_keeps_its_cache_control`（真实 socket）与 `oversized_no_store_body_invalidates_the_stored_avatar`。
2. **离线回退改用当前快照**：304 与网络失败分支都不再直接渲染请求前捕获的快照，而是在 blocking worker 上重读当前条目（同时复核授权），并按当前条目的 `Cache-Control` 策略决定是否回退。新增 `offline_fallback_uses_the_current_snapshot_not_the_pre_request_one` 覆盖“请求在途时被更新写入者替换为 no-cache”的场景。

### Round 9

1. **304 变陈旧时仍须检查复用策略**：304 分支现在追踪 `commit_revalidation_sync` 是否返回 `Stored`；若被更新写入者判为 `Stale`（例如被替换为 `no-cache` 的 v2），则对重读到的当前条目按 `is_snapshot_reusable_offline` 判定，不允许时回退 initials。新增 `a_stale_304_does_not_return_a_superseded_no_cache_entry`。

### 基线同步

main 推进至 `c44f34d`（合并 #58/#62/#63，与本任务并行）。本分支已 `rebase` 到该基线上，冲突零，重跑 `cargo check/clippy/fmt/test` 全绿（1007 passed 主二进制）。

### Round 10

1. **成功续期绑定被提交的表示**：304 分支不再在提交成功后再做一次独立 `read_sync`（那会在提交与读取之间给更新写入者留窗）。`committed == Stored` 时直接解码提交用的请求前快照（票据已证明 scope 有效、无更新写入者）；未提交时重读当前条目并按 `is_snapshot_reusable_offline` 判定。

所有 Cargo 检查通过 `node scripts/verify.mjs -- cargo ...` 执行：
- `cargo fmt --all -- --check`：通过。
- `cargo check --locked --all-targets`：通过。
- `cargo clippy --locked --all-targets -- -D warnings`：通过（zero warnings）。
- `cargo test --locked`：全量测试套件通过。
- `cargo build --locked`：通过。
- `git diff --check`：无空白或格式异常。
