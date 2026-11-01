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

## 剩余边界

- `src/app/accounts` 和正式 Shell 仍调用旧 `open`，尚未创建/注入 app 级 CacheStore；需唯一后续组装 owner。
- `credential_rejected` 只接受已分类的明确拒绝，尚不替代 `read_authenticated` 的最终组装分类；网络/限流/模糊拒绝不生成该事件。
- 无 GUI、真实 GitHub 或真实用户 PAT 验证；跨平台结果以 PR CI 为准。
