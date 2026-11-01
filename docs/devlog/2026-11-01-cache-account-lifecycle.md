# 开发日志：cache-account-lifecycle

## 结果

把真实 `AccountService` 与已合并 `CacheStore` 授权门禁连接起来：新增显式 `open_with_cache`、与当前账户 incarnation/read context 绑定的 `AccountCache`、替换/拒绝/移除生命周期、启动对账与恢复重试。旧 `open` 保持兼容；正式 Shell 尚未注入，离线 UI 不在本任务交付范围。

跨 DB 采用 retire/quarantine-before-catalog-commit 加启动对账，不声称两个数据库有原子事务。补充 `CacheStore::live_scopes` 只读枚举和进程内 suspension generation：当门禁写入结果未知时，不仅删除服务 map entry，连此前分发的旧 scope/lease/writer 也 fail closed。持久授权状态不因这个本地屏障重置。

## 验证

所有 Cargo 调用通过 `node scripts/verify.mjs -- cargo ...`，测试只使用临时目录、合成 bytes、假 PAT 和 mock transport。没有读取真实凭据、访问 GitHub、修改 GUI 或其他任务 worktree。

- 最终 `cargo test --locked account_cache` 9 项通过：run `bd7a4317-8d24-403f-b44c-514b98d8b3cd`。
- `cargo test --locked account` 通过：run `8545d81a-7020-4061-8132-edb39ba1af67`。
- `cargo test --locked storage::cache::tests` 通过：run `2a5e5eea-6b75-4bf7-84f6-04ac212c79a3`。
- 一次多过滤器命令被 Cargo 拒绝且未运行测试（run `4037e0b2-b71f-4c77-894d-2ecbd5dc7d77`），随后用合法的单过滤器分别验证；没有计为通过证据。
- 首次编译完成前发现 API/所有权问题，直接读取原日志修复；没有用重复全量构建筛选输出。
- 最终完整验证：`cargo test --locked`（run `c844db77-2c8c-4b4b-bffc-f723eef0d220`）、`cargo check --locked --all-targets`（`d1431b4e-9580-4b57-80cc-98d7f4fdf59b`）、`cargo clippy --locked --all-targets -- -D warnings`（`f7330707-b55a-4040-ab86-fcce3af22a28`）、`cargo build --locked`（`7abae586-4ec6-44b7-b294-29b4c4ebe50c`）全部通过。`cargo fmt --all -- --check` 通过（`a3120d3e-10e1-4105-bea5-863baaa4c440`）。最后将两个 lifecycle 辅助方法收紧为 `pub(crate)`，因此重跑了 test/clippy；check/build 已由 clippy 的 all-target 编译及此前 build 覆盖，无其他语义变更。

实际故障注入覆盖：cache scope UPDATE trigger 失败使替换不保存且旧 clone/lease/writer 封禁；account DELETE trigger 失败使替换已隔离或移除已退休后目录未提交；通过只改临时 DB 模拟“目录删除已提交、cache 仍 live”的崩溃后状态并在真实 `open_with_cache` 对账；Windows 独占文件句柄及 Unix 对象目录权限造成物理删除失败，修复后 `recover` 成功。测试用真多线程 runtime 和 blocking 边界探针固定旧拒绝在替换之后才取得提交锁。没有执行真实进程强杀，SQLite 自身的崩溃原子性沿用 cache-core/账户存储既有验证边界。

## 应用的既有教训

- L002：区分离线组合测试、mock 网络与真实 GUI/PAT；明确 Shell 和真实窗口未验证。
- L006：未新增 crate 级 warning 豁免，沿用现有 data/cache 模块边界。
- L008：迟到拒绝使用多线程 runtime 和真实提交锁交错，非 current-thread 顺序 await。
- L012/L013：本任务只交付生命周期和稳定 core 注入接口，不把独立适配器说成正式 Shell 离线能力；组合 owner 后续负责装配。

## 可复用候选（供协调者，不直接改共享 LESSONS）

跨 DB 安全操作失败时，撤销内存里的 map entry 不足以封禁此前 clone 出去的 `{store, scope}`；需要在共享 store 内设置只能拒绝、不能授权的本地代次屏障，并用旧 lease/writer 验证。该教训是否加入共享 LESSONS 由协调者决定。

## Codex 第一轮反馈与修复

PR #61 对 head `4b3832463b93132237237e7fadfb0d3873adc94f` 的 [P1 评论](https://github.com/nocpiun/githux/pull/61#discussion_r4436200521) 指出：门禁失败撤回 scope 后，直接重试替换会把 `None` 当成禁用缓存，可能未经隔离提交新 PAT。该问题成立，无需产品裁决，合并权限保持 worker。

修复为 cache-enabled 服务在 map 缺 scope 时也按持久账户身份取得分区，并强制应用 `PatReplaced`；同类移除重试也强制 `retire`。回归扩展 `cache_gate_failure_aborts_replacement_and_preserves_old_credential`：故障解除后不调用 `recover()` 直接重试，旧 bytes 仍隔离，需逐仓库证明才恢复。聚焦测试通过：run `92fa3988-1732-4c69-b661-c18a0ef403d2`。一次等待共享 mutex 超过宿主 15 分钟工具时限，未执行测试，随后以同一参数取得上述记录。第二轮全量验证通过：`cargo test --locked`（`6ac7c4d0-ffd5-42e3-8a5d-c381c5fac356`）、`cargo clippy --locked --all-targets -- -D warnings`（`b29fada8-f369-4d1a-a552-8dae810f44a7`）、`cargo build --locked`（`f2412e4f-bdbd-4ea1-b35f-9a7b8c8e4f61`）；格式化后通过最终 fmt/diff 核验。

## Codex 第二轮反馈与修复

PR #61 对 head `3fd636fa5db6bcd6d9baf0117a8a960b85ca90cb` 的 [P1 评论](https://github.com/nocpiun/githux/pull/61#discussion_r4436443355) 指出替换已 quarantine、尚未提交 registry 时旧 session 可以完成 repository proof。问题成立，无需产品裁决。

新增 `RepoService::begin_revalidation(context, repository)` 与 async `permission_confirmed(context, ticket)`：后者和替换共用提交锁，并在 blocking worker 内复核 registry epoch、cache 绑定与 store ticket。测试先在替换窗口构造旧 proof，用真多线程 runtime 暂停其提交、让替换先完成，再确认旧 proof 为 `Stale` 且 bytes 仍隔离；run `09aac8de-e701-4864-87e6-3a7e5f3aa66a` 通过。现有正常 revalidation 回归改走该服务入口，不把 core ticket 直接确认当作安全组装。最终 lifecycle 10 项通过（`cb5a4a68-9302-484c-99b8-5f6abb408c03`），全量 960 项通过（`8c9aade8-7bb8-4f82-ba59-316328da811c`），clippy（`69b9b170-c43f-44ce-b28f-9ced8f093226`）与 build（`9b1e0610-60a8-4194-bb27-18a3e37c3ffd`）通过。一次全量运行中未修改的 transport bounded 用例返回 Network 而非 ResponseTooLarge（`6629b76e-edd2-4208-a179-6f3c817cc4f2`），其单独重跑通过（`bc21f38e-dc2f-412c-a787-6497988bbb16`），最终全量通过；不把这波动作当作本任务逻辑验证。

## Codex 第三轮、CI 与基线

PR #61 对 `46b3a18ba24fa26d5acd655c56fa265ea9f15ce7` 的审查提出三项均可直接修复的问题：退休失败的 orphan 在重加前仍须退休；失败的明确拒绝须在 recover 时重放，不能被重新发 scope 覆盖；已持久退休的分区应允许 remove 直接重试。相应测试 `blocked_orphan_is_retired_before_readd_gets_a_fresh_partition`、`failed_rejection_is_replayed_before_recovery_reissues_scope` 与扩展的 `retirement_before_catalog_failure_is_fail_closed_and_recovers_on_reopen` 覆盖实际顺序，最终 lifecycle 12 项通过（run `8dc4b7c5-3297-4dd8-9525-6ff9cc597c92`）。

同一 head 的 Linux CI 在 Unix-only 测试分支发现 `cache_config` 局部变量遮蔽函数（Windows 不编译该分支），已改用已有 config 值。main 前进至 `ee0e9f9a8ca97b87e98b85cad46b6b2a8feedd8c`（reader-image-cache），已合并并保留其独立注册/实现；组合检查通过：全量 test `7a4cf22d-0e12-4cc4-ab05-4de56526fb4b`、all-target check `36844887-9fde-47d8-b7bb-05118c02dd7b`、clippy `ccd3c1db-6bcc-4861-96c6-72bd2fec33d3`、build `931bcdbf-eccc-40fe-a49c-3869369bc985`、fmt check `fbac683c-a0fd-4780-afad-e3706dceae39`。所有反馈均无需裁决，merge_owner 仍为 worker。

## Codex 第四轮恢复修复

对 `19f59052bb46b018dca884acabbd1c8350ff5109` 的两条 P1 均成立：re-added 的 blocked orphan 在 recover 中仍须先退休；map 为空不等于已持久退休，remove 重试必须查询 live scope，只有不存在 live 才视为已退休。另将 `recover()` 改为无论凭据 cleanup 是否失败都执行安全对账，再返回原清理结果。

回归新增 re-add 后 recover、退休失败→remove 重试→凭据删除失败→重加的组合。最终 lifecycle 14 项通过（`d96f1076-911f-4978-ab42-acbc595a7258`）；未确认的 orphan retirement 保持 blocked，正常已退休分区不阻碍 remove/re-add。全量 test `a1c3543b-6998-4da6-9214-a225e1eb3271`、clippy `34f0abcb-38a7-4116-813f-ad08c17d676e`、build `1b767fde-afab-4b81-810a-8f8251e3832a`、fmt check `814a092f-b113-482b-867e-3912a7bee161` 通过。反馈均可直接修复，无裁决。

## 剩余边界

- `src/app/accounts` 和正式 Shell 仍调用旧 `open`，尚未创建/注入 app 级 CacheStore；需唯一后续组装 owner。
- `credential_rejected` 只接受已分类的明确拒绝，尚不替代 `read_authenticated` 的最终组装分类；网络/限流/模糊拒绝不生成该事件。
- 无 GUI、真实 GitHub 或真实用户 PAT 验证；跨平台结果以 PR CI 为准。
