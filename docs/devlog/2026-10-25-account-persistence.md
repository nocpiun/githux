# 账户目录与活动选择持久化

日期：2026-10-25。任务：`account-persistence`，分支 `feature/account-persistence`，基线 `32ec5acbbe970bbba7f8dc766ef914e5299bea27`。交付：PR，First Release milestone #1。

## 交付范围

R2 的下一段服务基础：账户目录、CredentialRef、明确活动选择保存到独立 SQLite 用户数据库；重新创建服务可以离线枚举账户并恢复选择。尚未接入 UI/app，没有登录表单、业务缓存、头像文件或私有内容清理政策，也不宣称 R2 完成。

按任务选型核验 rusqlite 官方 README、manifest 与 CI，固定 `rusqlite 0.37.0` / `libsqlite3-sys 0.35.0` / bundled SQLite 3.50.2；项目 Rust 1.98.1 本地编译通过。Cargo.lock 只新增五个 SQLite 相关包，dirs/uuid/tempfile 使用已有锁定版本，没有升级 GPUI/Ely 或其他原有包。官方来源和目录约定见 [账户持久化](../account-persistence.md)。

## 实现与契约

- `src/storage/accounts.rs`：绝对可注入路径，默认系统本地应用数据目录；v1 事务迁移、schema/integrity 校验、规范 u64 字符串、显式 active、恢复引用；busy 等待默认 500ms、上限 5s。独占 OS lease 防止另一服务持旧内存快照覆盖；不自动删除/重建未知或损坏数据库。
- `src/data/accounts.rs`：open、添加/替换、选择、移除、恢复清理、revalidate、close。SQLite 与凭据操作通过 blocking workers，共享串行提交锁，UI 只读短暂内存快照。提交过程中不持 registry 锁做 IO。
- 新凭据采用 UUID revision 独立槽：先 SQL 登记清理引用，再写系统凭据，最后 SQL 原子发布新目录并登记退休槽。失败保留旧 PAT 与内存状态，中断后仍有清理路径。提交成功但 cleanup 失败返回明确部分结果，不能把所有 Err 理解为未提交。
- 恢复授权状态 PendingVerification / MissingCredential / CredentialUnavailable；凭据存在不等于已验证。重验核对稳定 ID，旧成功/旧错误校验 epoch、引用和 selection。替换另外核对 incarnation 防止 remove/re-add 后旧替换落到新记录。
- PAT 不进入 SQL、journal、WAL 或普通序列化；非 Windows 后端保持 Unsupported，无明文回退。
- 取消网络验证不提交；blocking commit 投递后完成，即使等待方取消。close 拒绝新提交并等待正在执行的提交、在后台关库。未显式 close 的 Drop 只投递独立线程清理，需立即重开应 await close。

完整状态和调用示例见 [账户持久化](../account-persistence.md)。RepoService 公共读取接口保留；同步 set_active 在持久化服务明确返回 PersistenceRequired，调用方改用 AccountService async 选择，避免虚假落盘成功。内存构造与可写 registry handle 限测试使用，生产只使用 open。

## 本地验证

```sh
cargo fmt --all -- --check
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --locked
git diff --check
```

上述本地 Windows 检查通过；全量 155 passed / 0 failed（原有 144 + 本轮 11）。四平台 Debug CI 的结果由 PR 当前 head 检查确认，本地结果不代替它。

新增验证覆盖：

| 场景 | 实际证据 |
| --- | --- |
| A 模拟 GET /user 保存两账户，B 恢复选择，移除后 C 不复活 | 临时 SQLite + fake 凭据；恢复状态待核验，重验成功推进代次；扫描目录所有文件不含 fake PAT |
| 新秘密写入后 metadata 失败 | fake store 在 save 后用第二个真实 SQLite connection BEGIN IMMEDIATE 锁住数据库；提交 Busy；旧 PAT/epoch 保持，新引用有持久 cleanup；重开后显式恢复 |
| 平台 save 返回错误但已写秘密 | fake store 先写再报错，删除也注入失败；保留 cleanup，之后恢复 |
| 移除后 secret 删除失败 | 返回 CleanupPending，重启目录仍为空，cleanup 可重试 |
| 目录写入被拒绝 | 真实 SQLite trigger RAISE(ABORT)；选择/移除的 SQL 事务回滚，内存与原凭据保持 |
| 中断在 stage + save 后 | 直接构造该真实持久中间状态并关库；重开清理 secret，不添加账户 |
| 缺凭据、Unsupported、401、换错身份 | 保存记录保持，状态明确，不新增别的账户 |
| migration / future schema / corrupt / read-only / busy | 临时真文件与真实写锁；未来版本/损坏原文件字节保持；有界返回 |
| 取消/关闭/重复 open | 验证前取消无记录；关闭后拒绝写；第二实例 InUse |
| 并发与 runtime | 多线程并发 replace/remove/add/select 与重启快照相符；暂停 blocking save，current-thread 网络 runtime 仍可推进；取消等待方不截断提交，close 等待后重开可恢复 |

首轮 149/150 通过，schema 用例重开时遇 InUse；原因是测试依赖兜底异步 Drop 立即释放租约，修正为显式 close 后通过。Clippy 指出部分成功错误过大，summary 改为 Box；未屏蔽该检查。

## 首轮审查修复

PR #18 的 `8bd8b5b` 收到一条成立的 P2：读取捕获旧 CredentialRef 后，替换可能删除旧槽，blocking load 返回 None/error 时原代码提前 `?` 返回，未核对上下文，可能把旧槽错误显示为新凭据问题。现对 blocking load 的成功、缺失、错误及 worker 失败统一先核对 runtime context/closed，再返回结果。新增 paused-load 真并发测试分别覆盖 None 和 Corrupted，并确认当前槽缺失仍返回真实 CredentialStore 错误。相关用例、全量 155 项、check/clippy/build 通过；无待裁决事项。

## 验证边界

新增测试只使用 fake store 和临时目录，没有使用真实 PAT/gh token/Cookie，没有访问真实 GitHub，没有新增真实 Credential Manager 测试。全量运行包含仓库原有自清理 Credential Manager 用例，本任务未改该用例。生产端到端 UI 重启恢复尚未验证。SQLite 四平台编译以本 PR CI 为准，非 Windows 凭据功能仍不支持。

审查/CI 最终链接与合并事实记录在 PR 和任务交接，不为补录已合并状态新增未经检查提交。PR16 不属于本任务，保持 open。
