# 账户目录与重启恢复

更新：2026-10-25。R2 的账户持久化服务已实现，尚未接入 GPUI/app/UI；不代表完整登录产品或 R2 完成。账户数据库是用户持久数据，未来清业务缓存不得删除它、其恢复记录或系统凭据。

## 依赖与位置

本任务按派发决定采用 `rusqlite = 0.37.0` + `bundled`，内置 `libsqlite3-sys 0.35.0` / SQLite 3.50.2。核验了 [官方版本 README](https://github.com/rusqlite/rusqlite/blob/v0.37.0/README.md)、[Cargo manifest](https://github.com/rusqlite/rusqlite/blob/v0.37.0/Cargo.toml) 和 [原生 CI 矩阵](https://github.com/rusqlite/rusqlite/blob/v0.37.0/.github/workflows/main.yml)：edition 2021，MSRV 政策是发布时 stable，bundled 经 `cc` 编译 C 源码并使用预生成绑定，不需要系统 SQLite 或构建时 bindgen。上游矩阵包含 Windows MSVC/Linux/macOS；本项目固定 Rust 1.98.1 的实际四平台结果以本 PR CI 为准，不由上游矩阵代替。没有升级 GPUI/Ely/reqwest 或已有锁定包。

`AccountDatabaseConfig::application_data()` 使用 `dirs 7.0.0` 的 [data_local_dir](https://docs.rs/dirs/7.0.0/dirs/fn.data_local_dir.html)，在其下建立 `Githux/accounts.sqlite3`。Windows 为 LocalAppData，macOS 为 Application Support，Linux 为 XDG data 或其系统约定回退。找不到目录返回 `PathUnavailable`，不使用 cwd。测试通过 `AccountDatabaseConfig::at(absolute_path)` 注入临时目录。

`accounts.sqlite3.lock` 是 OS advisory 独占租约文件，不含秘密，关闭释放锁但不删除文件。第二个服务/进程返回 `InUse`，禁止多个旧内存快照覆盖数据库；不提供多进程共享账户目录。数据库本身仍遵循 SQLite 锁与事务语义。

## 数据与恢复状态

实现分布在 `src/storage/accounts.rs`、`src/data/accounts.rs`、`src/auth/mod.rs` 与 `src/domain/account.rs`。

SQLite v1 只有账户顺序、HostId + 十进制 u64 数字账户 ID、显示资料（login/name/avatar URL/profile URL）、CredentialRef 的非秘密 revision、明确 active 选择、`user_version` 和待清理凭据引用。u64 使用规范十进制 TEXT 避免 SQLite 有符号整数截断。不保存 PAT、权限判定、AuthEpoch、selection generation、头像文件或业务缓存。PAT 只通过已有 CredentialStore；Windows 后端继续使用 Credential Manager，其他平台仍是 `Unsupported`，没有明文回退。

恢复不联网。身份来自过去 `GET /user` 验证后保存的稳定 ID，不把 login 用作身份，也不把本地存在当作当前授权有效。每次新建注册表重新分配运行时 epoch/selection；上下文只在所属服务实例内比较，调用方不得跨服务保留旧请求。

`AccountSummary.authorization` 是运行时状态：

| 状态 | 含义 |
| --- | --- |
| `PendingVerification` | 恢复记录且凭据可读，当前授权尚未核验 |
| `MissingCredential` | 目录仍存在，但系统凭据缺失 |
| `CredentialUnavailable` | 系统凭据读取失败或平台 Unsupported |
| `Verified` | 本次运行的 GET /user 已成功核验匹配身份 |
| `VerificationFailed(error)` | 本次重验失败；保存目录不会被自动删除 |

`revalidate(key)` 读取该引用并重新请求 GET /user，要求稳定 key 一致。返回前检查引用与 epoch；旧成功、旧错误均为 `StaleContext`。重验结果推进 runtime epoch，使更早请求失效；显示资料刷新留到显式 login/replace。RepoService 保持原读取语义，可以对恢复账户发起真实请求，但读取成功本身不把整个授权状态改为 Verified。

## 跨存储提交与失败契约

SQLite 与系统凭据不是一个事务。添加/替换采用独立 UUID revision 凭据槽，永不先覆盖旧 PAT：

1. SQLite 原子保存新槽的 cleanup 引用，成功后才允许写秘密。
2. CredentialStore 写新槽。即使平台返回失败也可能已写入，因此仍保留引用直到删除成功。
3. 单个 SQLite 事务发布完整目录与 active，移除新槽 cleanup，将旧槽登记为 cleanup。事务失败保留原目录、原 PAT 和原 runtime epoch。
4. 发布内存注册表，再删除退休/失败槽；删除成功后才从 cleanup 中确认移除。

进程在任意阶段退出：发布前只留下可清理的新槽，发布后目录引用新槽且旧槽仍可清理。下一次 open 尝试一次 cleanup；失败可通过 `recovery_pending().await` 观察，`recover().await` 显式重试，不无限自动重试。无 live 记录的秘密仍有数据库引用和重试入口；格式非法、指向 live 槽的 cleanup 视为损坏，不能按其删除活跃秘密。恢复记录不含 token。

移除先在单个 SQLite 事务删除账户、修正活动选择并登记旧引用，然后发布内存，最后删凭据。清理失败仍保持已移除，不会在重启复活；返回 `AccountError::CleanupPending`，恢复记录保留。当前不存在私有业务缓存，本功能不决定其保留/删除政策。

| 返回 | 调用方处理 |
| --- | --- |
| `Ok(SignedIn/Replaced)` | 凭据、metadata 已提交且无待处理清理失败 |
| `LoginError::CredentialSave` | 新凭据保存失败；目录保持原状，调用 recovery_pending 检查清理 |
| `LoginError::Storage` | 未发布新目录；原账户保持，不能显示保存成功 |
| `LoginError::CleanupPending(summary)` | 新账户/替换**已经提交**，退休秘密尚待清理；显示该明确部分成功状态，不盲目重放 |
| `AccountError::CleanupPending` | 选择/移除已提交，但仍有 journalled 清理；读取 summaries 确认当前目录 |
| `IdentityMismatch` / `NotFound` / `StaleContext` | 不提交；身份错误不得变成添加另一账户 |

## 执行、一致性、取消与关闭

open/migration、SQLite、凭据 save/load/delete、恢复清理都通过 `spawn_blocking`。写操作使用共享互斥提交入口，网络核验不占提交锁；操作按后台取得提交锁的顺序线性化。查询 summaries/active_context 只短暂读内存，不等待磁盘锁。单次 SQLite busy 等待默认 500ms，可配置并上限 5s；OS 凭据 API 自身没有本层可取消的 timeout。

替换捕获账户 incarnation，提交时复查；remove → re-add 后旧替换不能覆盖新账户。并发替换同一个仍存活的账户可依序提交，各自分配新 epoch。A → B → A 继续推进 selection，替换/移除/重验使旧成功与旧错误失效。持久化选择须用 `AccountService::set_active().await`；旧的同步 `RepoService::set_active` 在持久化服务返回 `PersistenceRequired`（内存测试服务保持原行为），不能绕过落盘。可写 registry handle 仅测试可用。

`login_cancellable(pat, cancel)` 在等网络许可与 GET /user 时可取消；提交前观察取消则不写入。提交一旦投递到 blocking worker，就运行至完成，丢弃等待 future 不会截断跨存储步骤；取消并不是撤销已经提交的操作。结果丢失时重新读取目录和 recovery_pending，不宣称已回滚。`close().await` 立即拒绝未来提交，等待已进入提交区的操作完成并在后台关闭数据库；尚未取得锁的提交返回 Closed。网络结果在关闭后也被丢弃。调用方还须管理网络 executor 的任务并关闭它。未显式 close 的兜底 Drop 将数据库关闭移到独立线程，租约释放可能稍晚，需立即重开时必须 await close。

v0 空库到 v1 采用事务建表并设置 `user_version=1`；非空未知 v0、损坏、非法记录、更高 schema 明确失败，保留文件，不删除重建。显式只读配置不能构建可写 AccountService；storage 只读打开可供诊断但写入报 ReadOnly。实际 SQLite 只读/锁冲突按类型返回。参考 [SQLite busy_timeout](https://www.sqlite.org/pragma.html#pragma_busy_timeout)、[user_version](https://www.sqlite.org/pragma.html#pragma_user_version) 与 [原子提交](https://www.sqlite.org/atomiccommit.html)。

## 后续登录 UI 的调用示例

以下在现有 BackgroundExecutor 的 Tokio 任务中调用；不要在 GPUI render/事件线程 block_on。GPUI 侧通过通道接收结果后更新实体。

```rust
let accounts = AccountService::open(
    AccountDatabaseConfig::application_data()?,
    credential_store, client, executor.clone(),
).await?;
let saved = accounts.summaries(); // 离线也可列出，读取 authorization 状态
let active = accounts.active_context();
if accounts.recovery_pending().await? {
    // 展示凭据清理待完成；用户/调用方可显式 accounts.recover().await
}
let outcome = accounts.login_cancellable(pat, cancellation).await;
// 必须区分 Ok、未提交错误与 CleanupPending(summary)，不笼统提示成功。
accounts.set_active(saved_key.clone()).await?;
let authorization = accounts.revalidate(saved_key.clone()).await;
let replacement = accounts.replace(saved_key.clone(), new_pat).await;
let removal = accounts.remove_saved(saved_key).await;
accounts.close().await?;
```

测试只使用临时目录、fake 凭据和模拟 GET /user；未做真实 GitHub PAT 联调，没有新增真实用户凭据操作。跨平台编译不是非 Windows 凭据产品支持，UI/app 尚未调用这些接口。
