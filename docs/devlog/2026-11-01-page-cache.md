# page-cache 开发记录

任务：为 Home/Feed、Profile 与仓库摘要 extras 提供持久数据层，分支 `feature/page-cache`，起点 `9c405942654237b95cceb10c0f8543ee1a6f06a5`。契约见 [page-cache](../page-cache.md)。本任务不修改 UI、正式 host、账户生命周期或 cache-core。

## 实现

新增可注入的 `CachedPageReadSession`，复用现有 PageReadSession/RepositorySession 的授权、selection、并发 permit 和取消守卫。版本化响应集在独立 CacheStore 内原子保存，离线通过同一 GitHub adapter 和 Feed policy 重放。Home open/involves、Feed 三类活动/作者与关注关系/源页 cursor、Profile pins/Popular/README 根目录约束均沿用已有路径。

返回来源、旧时间、完整性与明确 Miss。Complete 和 Partial 分开保留，离线优先 Complete；局部失败不会冒充空 Feed 或丢失字段错误。Home/Feed/Profile 组合使用账户聚合 gate，单 repo proof 不解锁。extras 用数字 repo gate。README 的 Missing 与读取失败分别保留。

REST validators 随各响应保存，但目前按整个组合无条件刷新，不用某一页的 ETag 代表聚合。意外 304 无条件重取一次，不能造空；任一依赖 no-store（包括错误）都使同请求旧 Complete/Partial 失效。请求前 writer tickets 防旧刷新覆盖新内容。请求凭据与 cookies 不持久化；GraphQL error message、未使用的 download_url 不保存。

## 教训应用

- L002：验收使用真实 page adapters + 临时 CacheStore + mock/localhost，报告不扩张为真实 GitHub/PAT/原生 GUI 验收。
- L007：继续用 GitHubClient/reqwest URL origin 判断，缓存层不新增 URL 认证解析器，不保存或重放 Authorization。
- L008：旧刷新/账户切换用受控 transport 的真正异步交错测试，不用顺序调用假装在途覆盖。
- L012/L013：服务能力与正式 Shell 消费能力分开，后续由协调者指派唯一 Shell 接线执行者。
- L014：README 未缓存、确认缺失、树不完整和读取失败保持不同状态，可选缺失不升级为 UI 故障。本任务不改可见组件。
- L015：保留现有 Feed 自动旧页触发算法；缓存只包裹读取，既有过滤与分页回归继续运行。

未修改共享 LESSONS。候选新教训：聚合 no-store 不能只在成功 outcome 上处理；后续依赖失败时也要使此前可复用快照失效。已用 Feed following 503 + no-store 建立回归证据，待验证后供协调者评估。

## 验证

所有 Cargo 命令经过 `node scripts/verify.mjs -- cargo ...`，共享 Windows mutex，独立 target；本任务进程设 `CARGO_INCREMENTAL=0`、dev/test debug=0。开始磁盘约 54 GiB，可用量持续检查高于 8 GiB。

第一轮最终 fmt/check/clippy/build 已通过。完整 test 的主程序 964 项及 Home/Profile/reader/image 等先行 targets 通过（含全部 13 项缓存组合/交错测试与原有 Feed 自动分页回归）；repository target 的既有 `chunked_and_no_length_stop_before_eof_on_success_and_errors` 一次返回 `Network` 而非 `ResponseTooLarge`，完整调用退出 101。直接读取失败日志，单独重跑该用例通过，再补跑尚未执行的 repository_overview 全部通过，没有改动通用 transport。

本地首轮还修正了两处新测试 fixture 问题：缺少 Arc import、仓库 JSON 缺少 html_url；针对性重测通过。所有失败原始记录留在私有 runner runs 内，未上传日志。

交付前安全复查补上当前有效会话 HTTP 401 的 `CredentialRejected` 持久事件（包括 Feed 作者依赖）；模糊 403 继续允许旧快照。缓存 blocking worker 复用既有 credential_commit，把 session 检查与缓存副作用相对 PAT/selection 提交串行，避免旧 401 隔离新凭据。

修正后的 14 项缓存聚焦回归、fmt/check/clippy/build 均通过（runner：`e9b1d55e`、`538c1abf`、`808bfbaa`、`d1a5a103`、`1dbab990`）。第二次完整 test（`251dd932`）通过主程序 965 项、Home 95 项、Profile 279 项、reader 250 项、reader_images 634 项；repository 706 项通过，两个既有 localhost bounded transport 用例同样收到 Network 而不是 ResponseTooLarge，调用退出 101。随后同一源码用 `cargo test --locked --test repository -- --test-threads=1`（`0971f642`）完整通过 708 项，支持并发 socket 测试不稳定的判断；未改动通用 transport 或放宽断言。最后 `cargo test --locked --test repository_overview`（`343e9ef7`）44 项全部通过。默认并发的两次完整 test 失败记录保留；所有测试 target 均取得最终源码的通过结果，repository 用串行方式核验。后续不再无变更重复测试。

## 未接入与剩余边界

正式 Shell 离线呈现和账户聚合重新授权证明没有在本任务接线。无真实 PAT、外部写操作或原生 GUI 验收。后续唯一组装点及调用方职责见主题文档；缓存数据层完成不代表离线产品已交付。
