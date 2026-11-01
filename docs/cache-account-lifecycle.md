# 账户生命周期与私有缓存

状态：`AccountService` 已能显式绑定应用级 `CacheStore`，保存、替换、明确拒绝、移除和重启对账已连接既有持久门禁。**正式 Shell 尚未调用 `open_with_cache` 或注入缓存消费者；本文不是正式离线 UI 已交付的声明。** 存储格式和 C1–C5 仍见 [cache-core](cache-core.md) 与[缓存策略](cache-strategy.md)。

## 组装与消费契约

旧的 `AccountService::open` 保持纯在线/无缓存行为。需要缓存时，应用只在启动组装处打开一次 `CacheStore`，然后调用：

```rust
let accounts = AccountService::open_with_cache(
    account_config, credential_store, client, executor, cache_store,
).await?;
```

所有打开和对账 I/O 都在 blocking worker；缓存对账失败不使账户 DB 打开失败。此时 `recovery_pending()` 为真，缓存绑定被拒绝，普通账户保存/选择/在线读取仍可使用；不能把它当作有效空缓存。应用级 `CacheStore::open` 本身失败时由组装处保留无缓存服务。

- 仓库/页面消费者使用 `RepoService::cache(session.context())`。只有该 `RepositoryReadContext` 仍是当前服务实例、账户、授权 epoch 和 selection 时才返回绑定。
- 账户级消费者使用 `AccountService::cache(&current_summary)`。整个 summary（含 epoch/运行时状态）必须仍匹配，避免 remove/re-add 后旧 summary 取得新分区。
- 返回的 `AccountCache` 提供同一个 `CacheStore` clone 与已保存账户的 `CacheScope`，也可 `into_parts()`。消费者仍须校验 `scope.account() == session.context().auth().account`，保留现有 session、context、资源代次、取消与 lease 复核。
- 消费者不得自行创建分区、打开另一 DB、绕过现有服务或把旧句柄当成持续许可。`None` 表示无可用私有缓存，不是空内容。

替换后消费者先通过 `RepoService::begin_revalidation(session.context(), Some(repo_id))` 取得 ticket，使用**当前 session 固定的新凭据**成功读取该仓库后，再检查 context 并 `await RepoService::permission_confirmed(session.context(), ticket)`。完成证明与账户替换共用提交锁，写门禁前再次核对 context，旧 session 在替换提交窗口内也不能解锁新 cache epoch。不得绕过该服务入口直接发布 core proof。`GET /user` 和 `revalidate()` 不会解锁仓库。账户级私有聚合使用 `repository=None` 的独立证明，不随任一仓库证明解锁。

## 跨 DB 顺序与崩溃窗口

账户目录与缓存 DB 不共享事务。所有生命周期提交沿用 `credential_commit` 串行化，采用以下持久顺序：

1. **保存**：先对账并完成新身份可能存在的旧分区退休，再按既有独立 revision 协议提交目录/凭据并 provision scope。对账或退休未确认时不保存该新身份，避免重启后把旧分区误认给新账户。remove/re-add 通过退休记录获得新 UUID partition；新保存不会重置已有 quarantine。
2. **替换 PAT**：先核对账户 incarnation，再持久 `PatReplaced`，之后才能保存新槽并提交目录。旧私有内容先进入逐仓库隔离。若随后凭据/目录步骤失败，旧凭据和旧目录继续权威，但缓存保持隔离，不能因回滚重新放开。
3. **明确凭据拒绝**：`RepoService::credential_rejected(context)` 只接收已分类的明确拒绝，blocking 前后都复核原 context，并在提交锁内再次复核；迟到 401 返回 `StaleContext`，不能污染替换后的凭据或 remove/re-add 账户。网络、超时、5xx、限流、模糊 403/404 不调用它。
4. **移除**：先持久 `retire`，再提交目录删除与凭据 cleanup journal。退休成功立即拒绝读、写和旧 lease；目录提交失败可留下“账户仍保存但缓存仍退休”的保守状态，之后重试移除或重启对账。目录已删除而进程在退休前崩溃的反向窗口由启动对账覆盖。
5. **启动/恢复**：先枚举 live scopes，退休所有不在持久账户目录中的分区，再为保存账户发绑定，最后回收退休 entries/slots 和孤儿对象。目录不存在账户时不能 provision。`recover()` 重试凭据 cleanup、目录对账与物理回收，不自动无限重试。

门禁 SQL 失败时账户替换不继续。由于结果可能未知，本地会撤回 scope map，并推进 `CacheStore` 的进程内 suspension generation；已经 clone 的旧 store/scope、旧 writer 和 lease 也立即拒绝。只有之后按持久目录和门禁重新对账才会发新 scope；suspension 不是授权证明，不持久化、不绕过 C1–C5。map 中没有 scope 不等于禁用缓存：故障后直接重试替换/移除也必须先按持久账户身份恢复该分区并应用 quarantine/retire，不能绕过门禁；已经持久退休的分区直接满足移除前提，不重开新分区。退休在本地封禁旧 clone 后才执行持久校验，因此重复移除或退休 SQL 失败也不会留下已启用的内存路由。

退休失败的 orphan 会保持阻断，重加前必须成功持久退休并取得新 UUID partition，不能因 SQL 故障已解除而直接采用旧分区。scope 枚举失败也是对账未完成的封闭屏障，不是空账本；新登录在准备阶段被拒绝而不提交目录，已有账户继续可用，恢复后重试。明确拒绝若持久化失败，恢复时先重放该事件，成功后才可能发绑定；新的已验证 PAT 通过 `PatReplaced` 明确取代该待恢复事件，不会顺带解除仓库门禁。

物理对象删除失败不回滚逻辑退休，其他账户不受影响，`recovery_pending()` 保持可观察。释放文件/修复目录后调用 `recover()`；成功后重试回收并清除 pending。账户功能不因缓存清理失败而被删除或伪装成清理成功。

## 实现与验证边界

实现位于 `src/data/account_cache.rs`、`src/data/accounts.rs`、AccountService/RepoService 构造字段，以及 cache core 的只读 `live_scopes` 与本地 suspension 防线。没有改账户 schema、cache schema、Shell、repository/page 消费者或 GitHub 网络分类。

离线确定性测试使用真实 `AccountService`、临时 SQLite 账户目录、真实 `CacheStore`、内存假凭据与 mock `/user`，覆盖正常重启、替换旧 lease/writer、逐仓库证明、账户聚合独立门槛、移除重加、跨账户隔离、迟到拒绝、门禁与账户 SQL 故障、目录已删而 scope 未退休的对账，以及 Windows 文件占用/Unix 目录权限导致的物理清理失败。未访问 GitHub 或真实 PAT。GUI、正式 Shell 注入、真实平台凭据与端到端离线阅读均未在本任务验证。
