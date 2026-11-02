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

第二条候选（本轮新增证据）：跨任务契约缺口应先写出“在旧实现上确实会失败”的回归再等依赖，ready 后直接用同一回归验收，避免用恒真测试冒充修复；同时把“失败来自待接核心”与“本任务代码缺陷”分开记录。本轮 3 项撤权回归先失败（`ba15ae5b`）、组合后转为通过（`8a84442a`），可作为以后跨 task 依赖交付的槏板。

## 验证

所有 Cargo 命令经过 `node scripts/verify.mjs -- cargo ...`，共享 Windows mutex，独立 target；本任务进程设 `CARGO_INCREMENTAL=0`、dev/test debug=0。开始磁盘约 54 GiB，可用量持续检查高于 8 GiB。

第一轮最终 fmt/check/clippy/build 已通过。完整 test 的主程序 964 项及 Home/Profile/reader/image 等先行 targets 通过（含全部 13 项缓存组合/交错测试与原有 Feed 自动分页回归）；repository target 的既有 `chunked_and_no_length_stop_before_eof_on_success_and_errors` 一次返回 `Network` 而非 `ResponseTooLarge`，完整调用退出 101。直接读取失败日志，单独重跑该用例通过，再补跑尚未执行的 repository_overview 全部通过，没有改动通用 transport。

本地首轮还修正了两处新测试 fixture 问题：缺少 Arc import、仓库 JSON 缺少 html_url；针对性重测通过。所有失败原始记录留在私有 runner runs 内，未上传日志。

交付前安全复查补上当前有效会话 HTTP 401 的 `CredentialRejected` 持久事件（包括 Feed 作者依赖）；模糊 403 继续允许旧快照。缓存 blocking worker 复用既有 credential_commit，把 session 检查与缓存副作用相对 PAT/selection 提交串行，避免旧 401 隔离新凭据。

修正后的 14 项缓存聚焦回归、fmt/check/clippy/build 均通过（runner：`e9b1d55e`、`538c1abf`、`808bfbaa`、`d1a5a103`、`1dbab990`）。第二次完整 test（`251dd932`）通过主程序 965 项、Home 95 项、Profile 279 项、reader 250 项、reader_images 634 项；repository 706 项通过，两个既有 localhost bounded transport 用例同样收到 Network 而不是 ResponseTooLarge，调用退出 101。随后同一源码用 `cargo test --locked --test repository -- --test-threads=1`（`0971f642`）完整通过 708 项，支持并发 socket 测试不稳定的判断；未改动通用 transport 或放宽断言。最后 `cargo test --locked --test repository_overview`（`343e9ef7`）44 项全部通过。默认并发的两次完整 test 失败记录保留；所有测试 target 均取得最终源码的通过结果，repository 用串行方式核验。后续不再无变更重复测试。

## PR 审查第一轮

PR #59 的 `43e3fbb` 已通过 CI、四平台构建与 Check。Codex 提出两项成立反馈：明确仓库撤权不会使 repository=None 聚合快照失效（P1），以及 TooLarge/Capacity 丢弃成功在线值（P2）。P1 需要跨任务缓存核心契约：现有 Home DTO 没有数字仓库 ID，核心没有聚合依赖/全局撤权 revision 接口，且本批核心扩展由 cache-account-lifecycle 独占。已登记永久 `true/coordinator`，证据与选项见 [裁决评论](https://github.com/nocpiun/githux/pull/59#issuecomment-6559390694)。

P2 已独立本地修复：仅 TooLarge/Capacity 允许返回未缓存的成功 Network 值；旧时间快照保留，Stale/AccessDenied 不降级。新增单条限制与 pinned sibling 容量压力用例；`e5b6f1d2` 的 15 项缓存回归全部通过，格式化及全 targets clippy（`c4099c49`，-D warnings）通过。按混合反馈流程本地验证后交协调者，未擅自推送新轮次；其余结果以任务交接为准。

## P1 核心依赖与组合验证（已消费）

批准方案 A 先落地为页面侧回归，不修改共享核心：新增 [page_cache 撤权回归](D:/githux-page-cache/src/data/page_cache/revocation_tests.rs) 与 [在途迟到回填回归](D:/githux-page-cache/src/data/page_read_tests/page_cache_revocation_tests.rs)，只使用既有 `revoke_repository(scope, Some(R))` 接口：完整/Partial 聚合、旧 lease、在途写、同账户其他仓库 S、其他账户、撤权后新在线聚合与 PAT 替换 proof 隔离。旧核心上 3 项断言按预期失败（旧 lease 仍可读、迟到结果仍发布为成功 Network、被撤权账户聚合仍可读，`ba15ae5b`：16 通过/3 失败），确认回归确实覆盖缺口而非恒真。

依赖 ready 后按协调者授权消费已提交核心小 commit `5306d0c322151d7287264c6d55684379b24f0428`（`fix(cache): invalidate account aggregate on repository revocation`）：范围仅 `src/storage/cache/mod.rs`、`src/storage/cache/additional_tests.rs`、`docs/cache-core.md`，已用 `git show --stat` 核验；未合入整个 lifecycle 分支。基线先 rebase 到最新 `origin/main`，再 `git cherry-pick -x 5306d0c3`。main 在本轮又推进到 `a041fac`（头像缓存 #60），再次 rebase 时只有 `src/data/mod.rs` 的模块注册行冲突，按两侧注册共存解决（`avatar_cache` 与 `page_cache` 都保留），无代码冲突；本分支 cherry-pick 提交为 `105a133`。全部本地 P2 与撤权回归保留。

组合结果（`8a84442a`，真实核心 + 页面适配器）：19 项 page_cache 聚焦测试全部通过，原来失败的 3 项撤权回归由失败转为通过，P2 TooLarge/Capacity 与 Stale/AccessDenied 不吞的用例仍通过；核心 `storage::cache::tests` 的 3 项新增撤权测试也在完整测试中通过。页面侧无需新公共 API。

最终检查（同一组合树，全部经 `node scripts/verify.mjs`，共享 mutex）：`fmt --all` `8e6acaf5`、`fmt --all -- --check` `51a5718c`、`check --locked --all-targets` `88e474e4`、`clippy --locked --all-targets -- -D warnings` `9804d007`、`build --locked` `cbae2e06`、`test --locked` `938d06d0`（1031/95/279/250/676/760/44 全部通过，0 failed），对应推送 head `b54ed10` 与 main `a041fac` 组合。此前在 main `c44f34d` 基线也跑过同一组检查（`228908b7`/`91c02791`/`d4ad42ea`/`bfd584a3`/`8182c4d7`/`c7fc3afe`，1017 项主程序通过）；中途 clippy 曾发现新回归里的 `&cached.scope()` 多余借用（`e576d68f` 退出 101），已按提示改为 `cached.scope()` 并重跑。基线变化后主程序测试数由 1017 变为 1031（含 #60 头像缓存用例），全部通过。

验证边界：以上是数据层核心 + 页面适配器组合，mock/localhost 与临时 CacheStore，无真实 PAT、外部写产物或原生 GUI；正式 Shell 离线接线仍未完成。已复制到 UI 的字节无法由 storage 撤回，页面发布前复核仍属后续组装。

## PR 审查第二轮（新 head `add0705`）

Codex 对本轮修复提出 1 项新的 P2：`repository_descriptions` 把整批坐标向量塞进缓存 request 身份，超过 `CacheKey` 的 4096 字节上限时，合法的大批次在线读取会在查缓存前就以 `CacheError::InvalidInput` 失败。这是真实缺陷，已修复：完整有序坐标向量与 `max_lookups` 序列化后生成固定长度的双 64 位 FNV-1a 摘要。复查时发现仅取参与查找的前几个坐标会漏掉结果映射的未解析尾部；最终保留完整输入、顺序、拼写和预算身份，防止尾部不同的批次串用快照。摘要不承担授权或内容完整性验证。

新增 [description_tests](D:/githux-page-cache/src/data/page_cache/description_tests.rs)：200 条长坐标（旧身份 >4 KiB，测试内显式断言该前提）在 `max_lookups = 1` 下在线成功、只发 1 次查找请求、离线可读、不同批次仍为 Miss、Home 聚合键仍在上限内。初版聚焦 20 项 page_cache 测试通过（`3c541665`）；最终补上首项相同而尾部不同、顺序变化、零预算及完整结果映射的断言，并明确测试撤权后关闭/重开 CacheStore 不会复活 Complete/Partial。main 又推进到 `97108a6`（#57 全局列表），修复前先同步该基线，无冲突。修复过程中先遇到两处本地问题：`parameters` 需包成 `Value::String`（编译错误 `d7a14f11`）、请求计数断言漏算登录请求（`57300929` 失败后改为比较增量），均已修正。

本轮最终组合检查（main `97108a6` + 独立核心依赖 + 页面修复）：聚焦 page_cache 20 项通过（`0b46ef86`），fmt check `3081582a`、全 targets check `094617ee`、全 targets clippy `3e2adf0d`（-D warnings）、build `95c83b6c`、完整 test `122f6f5a` 全部通过；完整测试 1041/95/279/250/686/770/44，0 failed。所有 Cargo 均使用共享 verify runner。本轮重新执行是因完整批次身份修正、重启撤权断言及 main #57 基线变化；通过后不重复验证未变化源码。

## 账户 lifecycle 合入后的最终组合

在 `3cdb95f` 推送后，main 合入 #61 到 `9e48f44`。按已批准依赖策略同步该已合入基线，自动合并无冲突；核心原修复与 main 相同，`git diff origin/main -- src/storage/cache` 为空，PR 最终差异不再携带核心文件，也未引入 lifecycle 未提交分支。因此重新运行组合验证，覆盖账户生命周期字段/构造与页面会话、撤权和描述批次路径；此前检查结果不代替这个新基线的证据。

旧 head `3cdb95f` 已收到 clean Code Review（[收据](https://github.com/nocpiun/githux/pull/59#issuecomment-6570372827)），但其 CI `41397803713` 在既有 reader-images `missing_images_fail_without_blocking_other_images_or_context` 用例收到 `Network` 而非 `NotFound`，1055 通过/1 失败，四平台构建全过。直接读取原始 CI 日志保留此失败；未修改其他任务所有的 reader-images 源文件，也不将旧 head 审查或 CI 作为本次新组合的放行证据。

#61 组合最终记录：聚焦 20 项（`aaa76d66`），fmt check `e796a209`、全 targets check `6c47e616`、clippy `a28eb2f1`（-D warnings）、build `b45829cd`、完整 test `90a9abc8` 全部通过，完整测试 1063/95/279/250/708/792/44，0 failed。此轮包含旧 CI 失败的既有 reader-images 用例，未改变其实现或断言。

随后 main 合入 `44678e6`（#56 Settings 独立预览）。已同步且无冲突；核对 `src/data/domain/github/network/storage/app`、`src/main.rs` 和正式 `src/ui/mod.rs` 相对 #61 组合均未变化，Settings 模块仅由 `examples/settings.rs` 通过 path 引入。因此保留刚通过的正式应用检查，补验 Settings example 的 check/test（`faf37a75` / `fa780fbf`，1075 项通过，0 failed），最终 CI 负责新 head 与最新 main 的完整组合；不因独立预览新增而机械重跑相同应用测试。

## 未接入与剩余边界

正式 Shell 离线呈现和账户聚合重新授权证明没有在本任务接线。无真实 PAT、外部写操作或原生 GUI 验收。后续唯一组装点及调用方职责见主题文档；缓存数据层完成不代表离线产品已交付。
