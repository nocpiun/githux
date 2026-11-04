# 业务缓存存储核心

状态：cache-core 提供可复用存储与持久授权门槛。**尚无正式 Shell 离线读取路径**，未接入 AccountService 事件生产、RepoService、页面、头像或 README 图片下载。账户事件接入和 DTO/网络新鲜度策略由后续任务完成，不能把本接口存在当作应用已离线可用。

实现：[CacheStore](../src/storage/cache/mod.rs)、[键](../src/storage/cache/keys.rs)、[来源与完整性](../src/domain/cache.rs)。公开方法是阻塞 I/O，调用方须在现有后台 executor 的 `spawn_blocking` 中执行；不新增 runtime，不在 render/事件线程调用。

## 已确认政策 C1–C5

- C1：账户仍保存、没有已知授权失败时，断网冷启动允许按最后已知授权读私有缓存。返回 bytes + fetched/verified 时间、DTO 版本、完整性和验证器；消费者标明历史快照、离线和上次同步，不能据此声称当前权限或最新版本。
- C2：`retire` 持久退休分区后立即拒绝所有读写。`recover` 可稍后删除退休内容并清理文件，open 自动尝试恢复。重加同 host + 数字账户 ID 获得新 UUID partition；其他账户不受影响。
- C3：`PatReplaced` 推进持久授权 epoch，隔离所有旧私有 scope。新凭据逐仓库确认访问权后才能 `permission_confirmed`，`GET /user` 不构成仓库权限证明。账户级私有聚合数据使用 `repository=None` 的独立门槛，不能随某个仓库重验一起恢复旧值。2026-11-04 补充授权：成功发布新凭据后可废弃可重取的旧聚合，建立只接收新凭据新结果的空聚合代次；不保留/解锁旧 Home/Profile/Feed 快照。两 DB 恢复顺序见[账户生命周期](cache-account-lifecycle.md)。
- C4：`CredentialRejected` 持久拒绝私有读取，重启或 `saved_account` 不解除；接受替换凭据后仍逐仓库重验。Offline/Timeout/ServerFailure/RateLimited/AmbiguousDenial 不改变最后已知许可。明确仓库撤权使用 `revoke_repository` 清理该仓库全部条目并拒绝私有读取；模糊 403/404 不足以调用它。撤销某具体仓库 R 时，同事务还使该账户 `repository=None` 的旧聚合快照（Complete/Partial）全部失效并推进聚合门禁 revision：既有聚合 lease、在途写 ticket 和旧重验 proof 均不能回填或重新打开旧数据，重启不复活；聚合门禁的 `allowed/epoch` 保持不变，因此在原本允许且未受 PAT 替换/凭据拒绝隔离的 scope 下，撤权后由当前凭据完成的新在线聚合读取仍可建立新快照。其他账户、同账户其他仓库及其在途写入不受影响。
- C5：普通可淘汰缓存，没有 pin/永久保留或离线下载保证。读取 lease 仅阻止容量淘汰，`snapshot()` 每次检查授权；退休、撤权、clear 可以立即使 lease 失效。已复制到调用方的字节无法由 storage 撤回，消费者必须继续复核现有 Account/RepositoryReadContext 与页面代次并清掉旧画面。

这些是产品决定；下述容量与 schema 是可调整的工程选择。

## 目录、版本和恢复

默认 `dirs::cache_dir()/Githux/business-v1/`，独立 `cache.sqlite3`、`cache.lock`、`objects/<UUID>`。测试用 `CacheConfig::at(absolute_path)` 指定专用临时目录。不打开 accounts.sqlite3、settings、系统凭据；不得把应用 data 根目录作为配置目录。UUID 文件名由 store 分配，输入键/URL/仓库路径从不拼文件路径。对象名严格规范 UUID，读取拒绝非普通文件和符号链接，清理只 unlink 规范 UUID/UUID.tmp，不递归或删除未知文件。

独占 OS 文件租约排除另一 store/进程；同一 store 可 clone，内部 mutex 序列化 SQLite 与文件发布。业务键包括 host + 稳定数字账户 ID + 随机 partition + repository/resource + 完整 request representation。固定对象 `Commit/Tree/Blob + SHA` 和 mutable resource/request 属于不同键空间；SHA 不绕过授权，存储不会因 SHA 相同跨账户共享。SHA 是消费者指定的对象身份；本层不把任意 payload 当 Git wire object 计算其 Git SHA，消费者仍负责 domain 身份与 DTO 语义核验。

数据库 schema v1，DTO 的 `representation_version` 独立。v0 只接管空库，拒绝未知非空库、高版本、quick_check 失败；保留原文件，**不自动重建授权账本**。损坏账本不能按“可丢弃缓存”删除重建后隐式恢复私有访问。若未来用户手动删除整个缓存目录，正式集成必须从账户事件/恢复策略重建安全门槛；本任务不提供该产品操作。

`scopes` 保存 active/rejected/retired、持久随机 epoch 与 restricted；`gates` 保存每仓库许可、epoch 与撤权 revision；`slots` 保存最近写 ticket 代次；`entries` 原子保存 payload/对象引用、完整元数据、字节数和 CRC-64/ECMA-182。CRC 用于检测意外损坏，不防本机攻击者篡改，既不授权也不作内容寻址去重。`repository=None` 是现有账户聚合门禁：具体仓库撤权时保守地使该门槛下所有旧聚合快照失效并推进其 revision，这是已确认取舍，允许损失部分本与撤销仓库无关的普通缓存，但不额外请求仓库数字 ID、不新增授权注册表、也不永久封禁新聚合在线读取。

大内容：create_new 临时文件 → write/sync_all → rename UUID 对象 → SQLite 事务发布。Unix 同步目录；Windows 掉电后的目录项持久性不额外保证，丢失对象返回 Corrupt miss。元数据发布失败回滚旧表示，残留孤儿下次 put/open/recover 删除。SQLite 使用 rollback journal、FULL synchronous；无半条表示，数据库引用丢失对象只作为 miss，不当空内容。

单条对象缺失、尺寸/CRC/元数据错误在 read 时返回带原因 miss 并删除该索引；兄弟项仍可读。schema/SQL/I/O 错误明确返回，error 文本不包含 SQL 参数、键、内容、URL 或凭据。`clear` 仅删除 entries/slots，保留授权门槛与退休记录，旧 writer 失效；随后后台调用 `recover` 回收文件与增量 vacuum。retire/revoke/clear 的成功表示逻辑提交成功；文件删除另行重试，不把清理失败假装成事务回滚。

## 最小调用流程

以下代码放在现有后台 blocking worker，外层继续检查 RepositoryReadContext，不重新加载凭据：

```rust
use crate::storage::cache::{AccessEvent, CacheConfig, CacheKey, CacheRead, CacheStore, ObjectKind};
use crate::domain::cache::{CacheCompleteness, CacheMetadata};

let cache = CacheStore::open(CacheConfig::application_cache()?)?;
// 只能对真实账户目录中仍保存的账户调用。重启不代表已联网验证。
let scope = cache.saved_account(saved_account_key)?;
let branch = CacheKey::mutable(
    Some(repository_numeric_id), true, "ref-head", "ref=main;api=2022-11-28",
    "github-ref-json-v1", 1,
)?;
let blob = CacheKey::fixed(
    repository_numeric_id, true, ObjectKind::Blob, blob_sha,
    "raw-bytes", 1,
)?;

// 在网络请求之前领取 ticket；刷新/写入后的新请求重新领取。
let ticket = cache.begin_write(&scope, &blob)?;
// await 现有 RepositorySession，检查其 context 后再切回 blocking worker。
cache.put(ticket, &response_bytes, CacheMetadata {
    representation_version: 1,
    fetched_at_ms, verified_at_ms,
    completeness: CacheCompleteness::Complete,
    etag, last_modified, cache_control,
})?;
match cache.read(&scope, &blob)? {
    CacheRead::Hit(lease) => {
        if let Some(snapshot) = lease.snapshot()? {
            // 消费者再解码版本化 DTO/原始 bytes；标明时间及固定 commit。
            // lease 可保留到这次读取/消费结束，不能永久持有作 pin。
        }
    }
    CacheRead::Miss(reason) => { /* 未缓存/损坏/版本不匹配，不是服务端空值 */ }
}
```

mutable request identity 必须包含影响表示的查询/分页范围/媒体类型/API 版本等。`TabId`、PAT、CredentialRef、AuthEpoch、Authorization、cookie 和签名下载 URL 不属于键。CacheKey 的 Debug 隐藏 request；任意业务字符串并非秘密识别器，调用方仍必须遵守输入契约。resource/request 有大小与控制字符限制，构造后不可变且不提供 Deserialize 绕过构造器。

账户生产者后续按明确事件调用：

```rust
cache.access_event(&scope, AccessEvent::PatReplaced)?;
let proof = cache.begin_revalidation(&scope, Some(repo_id))?;
// 使用当前凭据对该仓库作权限核验，并复核现有服务 context。
cache.permission_confirmed(proof)?;
cache.access_event(&scope, AccessEvent::CredentialRejected)?;
cache.revoke_repository(&scope, Some(repo_id))?; // 明确撤权证据；同时使该账户聚合失效
cache.retire(&scope)?; // 账户移除先提交逻辑拒绝；随后处理 recover
cache.recover()?;
```

ticket 绑定当前 store 实例、partition、持久 auth epoch、repository revocation revision、最新 key writer revision；更早结果返回 Stale，不能恢复门槛。跨重启不能复用 ticket。`saved_account` 不是“新 token 已通过”的通知。`start_fresh_aggregate` 是内部生命周期入口，只在新凭据目录/journal 已提交后调用；它原子清空旧聚合与写槽、推进聚合 revision，再允许新结果，拒绝 rejected scope。普通消费者不得调用它。账户目录与缓存不共享事务，账户生产者未来必须安排崩溃可恢复的事件顺序/对账；本交付不声称账户移除已在正式应用触发上述调用。

## 已显示副本的权限失效接口

```rust
// 请求开始前捕获，随后台 reply 一起传递，并保留到显示结束。
let authorization = binding.store().authorization(binding.scope(), None);
// None 表示账户聚合；仓库独立资源用 Some(repository_id)。
// 有绑定却无法捕获票据时保守拒绝发布，不把缺失票据当授权。
if authorization.as_ref().is_some_and(|token| !token.is_current()) {
    // 清掉在线/历史以及衍生副本，或丢弃后台迟到回复。
}
```

`CacheAuthorization` 可 Clone/Debug/Eq。`CacheStore::authorization(scope, repository)` 只取得独立于数据库 State 的内存锁；`CacheAuthorization::is_current()` 仅三次原子读取，无锁无 I/O。`is_authorization_current(&token)` 为兼容便利方法。票据不授予读取权限，仍须保留 session/context/cancel/资源与页面 generation、持久 gate/lease 的既有校验。

PAT 替换、明确凭据拒绝、退休、suspend 使该分区全部旧票据失效；仓库 R 明确撤权使 R 与该账户聚合 None 失效，未撤权仓库和其他账户不受影响；clear 使全部票据失效。普通刷新/写入/淘汰不推进权限票据，不需要为票据 pin 一个 entry。失效票据永远不恢复，新 gate/新证明只能供新请求捕获新票据。

在线与历史结果都须在后台完成和 UI apply 前复核同一张请求前票据，显示期间由已有 drain 复核并清除失效数据；不能只在后台检查一次，也不能用新票据为旧 reply 背书。`CachedPageReadSession`、`list_repos_cached` 还在 adapter 返回前验证自己的票据；UI 仍需保留自己的票据覆盖返回后到真正发布的窗口。旧 Profile AggregateGateStamp/CacheLease 语义仍兼容，但不再需要每页永久 80ms 磁盘轮询作为共享失效机制。Home/Profile/Repository 的组件消费由各原执行者完成。

## 表示与策略边界

Complete + 零 bytes 是有效空内容，Partial + 零 bytes 保留原因，无条目是 NotCached，损坏是 Corrupt；网络失败不能写成 Complete empty。put 接受调用方选择的完整性，不自行决定“新 Partial 是否可覆盖旧 Complete”；数据消费者按资源语义选择离线默认快照。

所有响应入口传 Cache-Control，大小写不敏感识别 no-store：不写该次 bytes，且使此前同键表示失效；no-cache 允许存储但后续使用须由消费者重验。`invalidate(scope,key)` 在成功业务写入或明确资源失效后同时删除条目和 writer slot，阻止更早请求回填；同仓库其他条目保留。验证器只随完整键对应表示保存。本层无 HTTP 请求、TTL 或 304 协议：消费者收到 304 先读取仍存在且 validator 匹配的原表示，再以同 key ticket 原子发布 bytes 与新 verified_at；没有匹配表示则无条件重取，绝不能生成空 bytes。当前没有公开的跨 key 复制或跳过 gate API。

## 容量与测量

初始默认总内容 256 MiB，单条最多 16 MiB，64 KiB 及以下入 SQLite，最多 8192 条，SQLite 文件硬上限 64 MiB。按读取时间 LRU，跳过活跃 lease，超预算写入返回 Capacity 并回滚；不会突破内容预算永久保留活跃值。新请求 slot 最多保留 max_entries 个，淘汰 slot 只使极旧在途结果 Stale。门槛/退休账本不按 LRU 删除；数据库页上限同样限制其增长，SQLITE_FULL 明确失败，不能为节省空间放开授权。

put 前清理已无引用对象，所以正常运行对象磁盘峰值是内容预算加最多一个 max_entry_bytes（以及 SQLite/journal、文件系统开销）；失败后孤儿同样可恢复。unknown 非 store 文件不计入可管理预算也不删除。数据库是额外预算，logical usage 不等于物理文件，recover 每次增量 vacuum 256 页；回收可分多次调用。该设计不提供账户配额或命中率保证。

测量测试 `measure_size_thresholds_and_bounded_eviction`：64 次写入交替 4/64/128/512 KiB，4 MiB 内容预算，对比三个入库分界。具体本机数据及验证边界见[开发日志](devlog/2026-11-01-cache-core.md)。这些合成大小用于选取保守起点；真实仓库/图片分布和缓存命中率留给消费者接入后测量。
