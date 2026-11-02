# 2026-11-02：修复图片并发测试的离线 fixture

任务：`reader-image-fixture`；执行者：基层 Agent；基线：`97108a68f5eec8e36ed6be966f157ced22d99648`。交付范围仅测试文件及本日志，生产图片读取、错误映射、缓存与网络实现不变。当前状态：实现和 Windows 本地验证完成，待 PR 当前 head 的审查与跨平台 CI；最终证据保存在 PR 与任务交接。

## 根因与原始证据

两次 main 的 Ubuntu code-check 都在严格的 `Err(ImageFailure::NotFound)` 断言上得到 `Err(Network)`：

- [`a4ea4ba` / run 41367461395 / job 125030604423](https://github.com/nocpiun/githux/actions/runs/41367461395/job/125030604423)：`githux` target，`src/data/reader_images_tests.rs:1123`。
- [`a041fac` / run 41380458397 / job 125069959188](https://github.com/nocpiun/githux/actions/runs/41380458397/job/125069959188)：`repository` target，同一源码/断言。该 run 的 `githux` 与先前 target 中同名测试通过，不能把一次通过解释成稳定。

原测试启动 missing/good 两张图片，却给 `Controlled::steps` 全局 FIFO 预塞 `commit, commit, tree, tree, blob`。真实 `RepositoryImageService::start` 为图片各自 spawn 任务，`snapshot` 与 `directory` 在 await 前查询共享 `ResolutionCache`，成功后填入缓存。请求是否重复以及后续请求的到达次序都取决于另一任务是否已填缓存。少一次 commit/tree 或请求交错后，FIFO 会给 tree 请求返回 commit JSON，或给 blob 请求返回 tree JSON；解析错误由既有生产映射转为 `Network`。缺图的树查找根本没有机会稳定到达 `NotFound`。

执行路径核对：`BackgroundExecutor::new` 使用专用线程上的 current-thread Tokio runtime，而非 multi-thread 网络 runtime；它会驱动多个独立任务；每次认证读取在 `RepoService::read_authenticated` 中以 `spawn_blocking` 读取凭据，图片解码也使用 blocking worker。JSON 解析自身同步执行，交错来自这些实际的 await 边界。回归验证在 multi-thread 测试 runtime 上调用实际 `start`，并用请求进入信号证明两个独立图片任务同时未完成；没有把单线程 `join!` 当作并发证据。全程离线，不涉及 GitHub 网络波动。

## 修复与验收

为 `Controlled` 增加显式启用的 URL fixture：按规范化完整 URL 匹配 GET 请求，响应可重复读取；未知 URL（包括未建模查询参数）或方法明确 panic，不能降级成产品 404。gate 仅控制某个 URL 的请求放行，响应始终由请求身份选择。其他既有 FIFO gate/cancel 用例保留原语义。

- 原同名回归分别以 missing 先启动、good 先启动。两个 commit 请求都进入 gate 后，先放行第二个，再让两个 tree 请求同时在途，反向放行。严格断言缺图为 `NotFound`、好图解码为 6×3，并验证账户上下文有效和实际请求数为两个 commit、两个 tree、一个 blob。
- 缓存命中回归让 good 停在 blob gate，再启动 missing。missing 必须在 good 尚未完成时返回 `NotFound`，并且总请求数只有一个 commit、一个 tree、一个 blob；放行后 good 成功。随后再读 good，只有 blob 请求增加，验证 fixture 可重复使用与服务上下文未受缺图影响。

## 验证

所有 Cargo 命令均通过 `node scripts/verify.mjs -- cargo ...`，持有共享 Windows mutex，run ID 与日志保存在本 worktree 被忽略的 `.agents/task/runs/`。

修复前证据：

- 原样 `cargo test --locked --bin githux missing_images_fail_without_blocking_other_images_or_context`，run `b0f373f7-4576-47e2-94e5-eb0e27de530d`，单次通过；这仅说明原错误依赖调度，不作为修复证据。
- 确定性诊断：保留原 FIFO 与并发双图，通过真实 `service.snapshot(COMMIT)` 先填共享缓存，再启动双图，确保不再发出第二个 commit 请求，剩余的“just in case”commit JSON 被后续 tree 请求错用。相同命令 run `ad097e32-9fbf-45c0-82ed-d48d14c3aec2`，exit 101；严格断言实际 `Err(Network)`、预期 `Err(NotFound)`，与两次 Linux CI 一致。没有随机重试。诊断改动已撤下；最终回归以 URL fixture 和显式 gate 验证两种启动顺序与缓存减少请求。

修复后聚焦检查：

| Cargo 参数 | run ID | 实际结果 |
| --- | --- | --- |
| `test --locked --bin githux --test repository missing_image` | `1cc60535-3569-4be5-81f8-64ca7a04b154` | 两个 target 各 2 项通过；包括原同名测试的两个启动顺序和缓存命中时的并发回归 |
| `test --locked --bin githux --test repository data::reader_images::tests` | `b8a9b38e-a257-4150-9553-2585b886fd37` | 两个 target 各 20 项通过，包含原有有序取消/超时/账户切换测试与未知请求拒绝 |
| `test --locked --test reader_images` | `8ca823f7-d154-480d-8b8e-ec7fd3baf61a` | 664 项通过 |

最终本地检查（源码与上述聚焦检查一致；fetch 后 main 仍为 `97108a6`，未为筛日志重复编译）：

| Cargo 参数 | run ID | 结果 |
| --- | --- | --- |
| `fmt --all -- --check` | `5813a8ee-cf47-49c0-9909-7d30ab4581d4` | 通过 |
| `check --locked --all-targets` | `947db08f-ee75-4c74-bee8-f046485c4a5e` | 通过 |
| `clippy --locked --all-targets -- -D warnings` | `51cc5b11-f728-44a8-b6ed-a58d26d5f381` | 通过，零警告 |
| `build --locked` | `51bae4f7-c0ab-4768-9088-c2d49c63c236` | 通过 |
| `test --locked` | `c8f01eb6-2a48-4004-951d-3762c61eb2ed` | 3,099 项通过，0 失败/忽略 |

完整测试覆盖 `githux` 1,019、home 95、profile 279、reader 250、reader_images 664、repository 748、repository_overview 44 项。`git diff --check` 通过。所有运行各自排队取得共享 mutex，没有并发重编译。Linux 和四平台结果以实际 PR/main Actions 为准，Windows 本地通过不替代 Linux CI；本任务没有 GUI、真实网络或离线页面产品行为变更。

## 当前 main 组合验证

初轮 [PR #64](https://github.com/nocpiun/githux/pull/64) 的 `8401fbb71c80facf1b8d302eb41f3c081ccae649` 已取得 [Codex clean 摘要](https://github.com/nocpiun/githux/pull/64#issuecomment-6570038743)及 [CI run 41395704155](https://github.com/nocpiun/githux/actions/runs/41395704155) 成功，含 Ubuntu code-check、四平台构建与独立 `Check`。等待器 run `f4ab058b-4a78-41e2-9614-14618c525dae` 在下载日志期间按退避恢复 EOF/TLS 临时错误，核验所有执行 job 的 checkout 都是 `b946ddf`，父节点对应原基线 `97108a6` 与本轮 head。

最终复核时 main 已合入账户缓存生命周期 PR #61，前进至 `9e48f4429a10378b163ce1115a66b38bce45c588`，因此程序正确返回 `base_changed` / exit 13，而非允许合并。本分支无冲突 rebase 到该基线；账户/存储变更会影响图片测试使用的真实 session 路径，故重新执行聚焦检查与完整 fmt/check/clippy/build/test，属于新基线组合验证，不是盲目重跑以求转绿。组合验证记录如下：

| Cargo 参数 | run ID | 结果 |
| --- | --- | --- |
| `test --locked --bin githux --test repository missing_image` | `c053e71b-c286-4155-aebf-6556339f1529` | 两个 target 各 2 项通过 |
| `test --locked --bin githux --test repository data::reader_images::tests` | `d775dc8d-9c52-48ba-851d-583c177b554d` | 两个 target 各 20 项通过 |
| `fmt --all -- --check` | `dda690ae-aa0c-464c-a5c9-77865359a9ef` | 通过 |
| `check --locked --all-targets` | `66766020-9f35-49e6-b0ef-db71859d879e` | 通过 |
| `clippy --locked --all-targets -- -D warnings` | `d9f7d648-54a0-4189-9558-7c0f9c98d381` | 通过，零警告 |
| `build --locked` | `2e0dae0a-f116-44cd-998c-d04f87ff3629` | 通过 |
| `test --locked` | `0f3d38a7-b206-4d62-b400-cc7bfb748f3a` | 3,174 项通过，0 失败/忽略 |

随后最终 fetch 发现 main 又前进至 `44678e6dfafdd7e0956cbd2a41ef61bbaaad440f`（Settings PR #56）。该增量只有独立 Settings 的 UI/example/fixture 和文档，不改图片读取、账户数据路径或本任务文件，分支再次无冲突 rebase。保留 `9e48f44` 上已通过的完整 Windows 组合检查，针对新基线补跑两个 target 的全部 20 项图片服务测试与 fmt；最终组合的全目标 check/clippy/test 和四平台 build 必须由新 head 的完整 CI 重新验证，不能沿用初轮旧 head 放行。`test --locked --bin githux --test repository data::reader_images::tests` 的 run `1debfe85-80b2-49c2-9989-95408a0c34a6` 两个 target 各 20 项通过，`fmt --all -- --check` 的 run `dd82681b-0cd2-46b5-8860-aa0bb1f12468` 通过。远端最终证据留 PR 评论和交接。

## 2026-11-03 恢复与最新 main

原范围内阻塞的 Feed description FIFO fixture 已由独立任务修复（PR #65），本任务不再等待产品或重复编辑 Feed。恢复时由协调者把上轮自定义交付字段原样保留在 `progress.additional_delivery_metadata`，顶层恢复为规范字段；`arbitration_ever_required: true` / `merge_owner: coordinator` 保持不变，故本任务即便全部门槛通过也只能报告 ready，不自行合并。

同步 `b23ece505abb19a9f8b5484a654b99065f5b3445`（含 page-cache 与 Feed fixture）后，分支无冲突 rebase 为 `25ad2168f4384af1b248c24900f25575cfff5b27`。增量未改图片测试或生产图片错误语义，但改了 `page_reads.rs`；补跑受影响的真实 session 图片服务测试和格式检查：

| Cargo 参数 | run ID | 结果 |
| --- | --- | --- |
| `test --locked --bin githux --test repository data::reader_images::tests` | `16ac17b1-5109-41da-82e3-95e4290a7f98` | 两个 target 各 20 项通过 |
| `fmt --all -- --check` | `4692eabd-026e-4f1d-afa3-0a98c4724438` | 通过 |

保留 `9e48f44` 上已完成的 Windows 全目标 check/clippy/build/全测与 `44678e6` 上的完整图片测试记录，不机械重跑无关实现；最新组合的全目标 Linux 检查与四平台构建由本 head 的完整 CI 重新验证。最终 head、CI、review 和 waiter 证据以 PR 评论与交付记录为准。

## 教训与后续

适用 L002、L008、L010。L010 的同一根因也会出现在并发服务测试：按调用次数预留重复响应仍不能建模可变请求数。本日志作为补充证据候选供共享 LESSONS 唯一写入者归并，不修改共享 LESSONS 或扩大生产范围。无待裁决事项；后续按当前 head 审查、最新 main 组合 CI、squash 默认消息和合并后 main run 完成交付。
