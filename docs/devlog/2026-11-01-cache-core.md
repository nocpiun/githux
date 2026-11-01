# 2026-11-01：cache-core

任务分支 feature/cache-core，基线 main `997f6e947fced45ca00b728fcea4756957f36061`。按已批准 C1–C5 交付存储/政策，不修改账户 schema、运行时、RepoService、Shell 页面或 Cargo 依赖。接口与整合示例见 [cache-core](../cache-core.md)，本次同步缓存策略中的旧待定政策。

## 实现与边界

一个 CacheStore，独占文件租约 + mutex。SQLite 独立于账户/设置，持久 scope epoch、逐仓库 gate、退休分区与最新写 ticket；内容是可淘汰 entries/UUID 对象文件。换 PAT 隔离、拒绝凭据跨重启隐藏、逐仓库重验、明确撤权清理和移除后新分区，固定 SHA 一样检查门槛。Complete/Partial/空/miss 区分，显式 DTO 版本、时间与验证器，no-store 使同键旧表示失效。有界尺寸与 CRC-64 检测意外损坏，不把 CRC 当密码学证明或 Git SHA 验证。

大对象 sync/rename 后事务发布；失败保留旧 metadata，孤儿在 put/open/recover 清理。clear 保留门槛；retire/revoke/clear 的物理文件清理可稍后 recover。损坏/未来 schema 不自动重建授权账本。接口全部 blocking；调用方复用现有后台 worker，页面发布前仍复核既有授权/会话代次。

**没有正式 Shell 离线路径，也没有账户事件生产者整合**。本次无 GUI/native preview、真实 PAT、实网 API 或真实用户缓存删除。后续生产者需实现账户目录与缓存的崩溃恢复顺序/对账，后续消费者负责版本化业务 DTO、304/TTL/Partial 默认选择、在线策略和 UI 离线标识。这些不能从存储测试推导完成。

## 验证

测试在产品现有 bin 的 storage::cache 模块内，不增加第二个全应用 test target。覆盖临时目录重启隔离、持久隔离/逐仓库恢复、退休/重加、模糊错误不撤权、明确撤权、no-store、空与部分结果、真实双线程交错、旧 store ticket、撤权 ticket、回滚和孤儿恢复、子进程在未提交事务中 `process::exit(23)`、损坏/缺失/敌意路径、容量保护、清缓存保留独立账户/设置文件。Unix CI 专门执行 symlink 分支；本机 Windows 没有把 cfg 跳过冒充该项实测。

所有 Cargo 调用经 `node scripts/verify.mjs -- cargo ...`，runner 的 JSON/log 留 `.agents/task/runs`，不上传。首次聚焦检查发现零长度 SQLite substr 表示丢失，改为显式保留空 blob；同时校正了超大损坏尺寸在启动容量回收后表现为 NotCached 的断言。测试能发现真实空值边界，不把有效空内容当损坏。

本地最终检查均在 2026-11-01 稳定代码上通过（固定 Rust 1.98.1，Windows）：

| 检查 | run_id | 结果 |
| --- | --- | --- |
| `cargo fmt --all -- --check` | `a5941953-3bfd-408d-aea1-df1d852fd39a` | 0 |
| `cargo check --locked --all-targets` | `60682e37-0960-4f92-92a2-d106a3c6fc36` | 0 |
| `cargo clippy --locked --all-targets -- -D warnings` | `8b32bdba-1ed0-4421-9d52-6e3fb8016ffc` | 0 |
| `cargo test --locked` | `da352a1b-fc47-4c5a-a44d-e5b488d57c3a` | 0；全部 suite 通过 |
| `cargo build --locked` | `c6521bec-09e4-4955-9b58-72ca443ed23b` | 0 |

聚焦缓存测试修复后 `0015076a-c19e-4146-9812-0d7852546fb7` 为 17/17；首次 `0e6bc895-0e71-4c8c-ba45-9be5ae06a956` 的 15/17 已记录实际失败。全量测试中 storage::cache 用例也再次通过；全量输出中的 704 个过滤项不是该聚焦命令的覆盖范围，其他 suite 的实际通过见 run。clippy 修复的是未接入公开重导出和条件折叠风格，不改变运行行为。

CI 四平台、Linux symlink 路径和 PR 审查仍以实际远端结果为准，不由本机 Windows 推断。桌面预览没有 cache 界面可验收；本任务为存储/政策，不增加 UI 或完整离线路径。

## 合成测量与默认

本机 Windows、固定 Rust 1.98.1、临时目录，64 次交替写入 4/64/128/512 KiB，4 MiB 内容预算。稳定版聚焦测试记录：

| inline 阈值 | 逻辑内容 | 留存条数 | 对象文件 bytes | SQLite bytes | 总写入/回收毫秒 |
| --- | ---: | ---: | ---: | ---: | ---: |
| 64 KiB | 4,149,248 | 21 | 3,801,088 | 524,288 | 1,083 |
| 128 KiB | 4,149,248 | 21 | 3,145,728 | 1,310,720 | 795 |
| 256 KiB | 4,149,248 | 21 | 3,145,728 | 1,310,720 | 812 |

三个阈值均收敛至预算；更大的 inline 在这个合成负载写入更快，但 SQLite 占用增加。选择保守 64 KiB 作为初始分界，把大正文放文件；总内容 256 MiB 约可容纳此合成 4 MiB 工作集的 64 倍，单项 16 MiB、8192 条和独立 DB 64 MiB ceiling 进一步约束开销。不是实际用户仓库数量/命中率保证，也未比较 128/512 MiB 的真实负载命中率。容量和阈值以后根据真实仓库/图片接入测量调整，不作为新的用户选项。

没有新增需合入 LESSONS 的已证实跨任务规则；复用 L002 的验证边界、L007 的安全解析原则（文件身份只规范 UUID，不手拆 URL）及 L008 的真实线程交错测试。LESSONS 由本批 Home 执行者协调维护。
