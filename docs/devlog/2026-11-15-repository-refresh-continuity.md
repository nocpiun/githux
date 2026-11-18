# Repository refresh continuity

- 日期：2026-11-15；任务：`repository-refresh-continuity`；分支：`fix/repository-refresh-continuity`；起点：`812e5fc6b6389ea7c67c60f17a8c1b8ae53cfc15`。
- 目标：保留缓存首屏速度，在同账户/仓库/ref 的在线重验期间保持可读内容，按固定提交协调发布替换；账户、授权与 ref 的隔离边界不变。

## 起点与最小复现

实际读取路径为 `RepositoryPage::activate` 同时启动 `Job::Snapshot` 与在线 `Job::Repository`。快照作业只读本地；在线作业依次读取摘要 → 分支 → revision，再分别读取根树、README 定位与正文。并非“先缓存结束再开始网络”。

`apply_revision` 已对同分支、同 commit 保留树/reader，但不同 commit 的分支立即替换 `BranchState`、将树置空、清掉 reader，然后才启动后续读取，导致等待新内容期间绘制空树与 Loading。手动 `refresh_repository` 更早清空摘要、overview、树、reader 和授权槽，随后又从缓存重建。`app::pages::repository_page`/`Shell::sync_pages` 没有因普通缓存/网络结果重绑 host；host 只在账户授权上下文变化时重建。

最小回归 `continuity_changed_commit_keeps_cached_content_until_replacement_is_ready` 复用真实缓存服务和共享仓库 fixture：在线预热、重开服务、挂起网络并确认缓存树与 README 已渲染；将同一 `main` 指向 fixture 的第二个 commit，只挂起该 commit 的根树，检查等待中的实际页面与 reader 实体；再放行，逐次 drain/render 检查树和 reader 没有空白中间态。

## 基线失败证据

Runner `6e66d345-943b-4c2d-ad73-aa090d7993a7`，命令 `node scripts/verify.mjs -- cargo test --locked --test repository continuity_changed_commit -- --nocapture`：1 个回归失败、1013 个过滤。失败断言为新根树仍挂起时 reader 应为 `Showing`，实际为 `Loading`；测试运行 0.91s。构建槽等待 212ms；首次依赖/目标构建与运行合计 846.8s。此时生产源码仍为基线。

## 实现

- `src/ui/repository/refresh.rs` 通过已有 `CachedRepositorySession`/`RepositorySession`、同一取消令牌与固定 `RepoSnapshot` 在后台准备根树、已展开目录、README 定位和当前正文。只加载保存的展开目录与选中路径所需祖先，不递归下载仓库。
- `apply_revision` 遇到同 ref 新 commit 时保留显示中的 `BranchState`/reader，将新目标存为 `replacement`；完成后 `Body::Replacement` 在一个前台更新中发布新树、README、正文或真实终态。所有中间读取不修改已显示 commit 或把旧正文标成新版本。
- 替换仍核对原页面 generation、授权上下文、carried token、branch request、候选 commit 与 selection request。用户准备期间改变选择时重新准备最新路径；明确切 ref、账户、授权、取消会退休候选。失败保留旧快照与来源，使用已有 compact warning/Retry 样式提示。
- 手动 Refresh 推进读取身份、取消旧工作，但保留有效摘要、树、reader 和 overview。保留 reader 后重新绑定回调身份，不替换实体、滚动或选区。overview 保留显示 binding，单独用各端点请求代次取消旧读取。无需改变共享 overview API，也未修改并行任务负责的 `repository_overview/summary.rs` 或 `repository/view.rs::tab_strip`。
- 刷新不再丢弃仍在显示的授权令牌；撤权期间旧资源不能被新请求重新授权。重新激活、重绑和数字仓库身份变化仍清空旧数据。
- 渲染只增加同作用域的紧凑刷新错误入口，没有修改主题、布局间距或已有链接命中区域，因而无需主题文档变更。

## 运行与验收

正式应用启动：`node scripts/verify.mjs -- cargo run --locked --bin githux`。负责人可打开已缓存仓库，检查缓存首屏、自动重验、手动 Refresh、文件/分支切换与当前滚动/选区；正式应用继续使用其已有账户与缓存路径。

确定性离线复现/回归：`node scripts/verify.mjs -- cargo test --locked --test repository continuity_ -- --nocapture`。用例在可控传输层门闩下断言中间渲染状态，不以最后出现内容证明不存在空白帧。

## 验证记录

- 首轮修复边界检查：`node scripts/verify.mjs -- cargo test --locked --test repository repository_modules::repository:: -- --nocapture`，runner `a0e6edc0-616b-498b-8d28-892983a3f657`，111 通过、0 失败；包含原先失败的空白回归与既有 repository/navigation/extras/star/offline 检查。构建槽等待 473.4s；实际编译/运行 190.0s（测试本身 6.31s）。
- 首轮通过后自查补齐两项边界，因此最终检查重新运行：数字仓库身份改变时推进整页 generation，拒绝前一个仓库已排队的分支列表；替换 README 路径改变时把新祖先目录纳入展开意图，确保实际选中行可见。各自新增确定性回归。
- 中途 runner `933bce53-e40c-40b7-be91-dcca3439345b` 在等待构建槽时取消，以集中剩余场景；该次没有启动 Cargo，不视为检查结果。
- 最终仓库边界：runner `8db4c166-ed1c-4778-82b5-9339d7f5b65c`，113 通过、0 失败；槽等待 183ms，实际编译/运行 138.6s，测试本身 6.44s。包括同 commit 的 reader/overview 实体、滚动与选区保留，新 commit 准备期间逐次实际渲染、固定 commit 一致性，正文失败、最新选择、删除路径、README 移动、数字仓库身份、账户/ref 切换与刷新期间撤权。
- `cargo clippy --locked --bin githux --test repository -- -D warnings`：runner `5f8b36f5-3182-4d98-afc5-0799c8aba3cd` 通过；槽等待 191ms，实际执行 194.0s。
- `cargo build --locked --bin githux`：runner `1dc4697f-4b2f-45c2-b776-19339440bdc0` 通过；槽等待 155.4s，实际执行 225.3s。
- `cargo fmt --all -- --check`：runner `02178731-f0db-4742-a9a2-c69f3784a8d1` 通过；不占构建槽，实际执行 24.4s。`git diff --check` 与 `git diff --cached --check` 通过。
- 所有 Cargo 经共享 runner，重检查串行持有构建 mutex。最终通过后未重复运行未改变的测试；远端最新 head 的 CI/Codex 证据留在 PR 与任务交付状态。

## PR #89 第一轮审查

Codex 对 `37305ac` 提出两项 P2，均符合既有刷新/失败约定，可直接修复；无待裁决事项，保留 `arbitration_ever_required: false / merge_owner: worker`。

1. [失败资源重试](https://github.com/nocpiun/githux/pull/89#discussion_r4577492837)：原地刷新保留 `ContentFailed`，而同 commit 捷径仅为 cache-first 失败清理状态，导致先前在线文件读取失败后整页刷新不再重读。回归通过共享 `Scene::FileFailure` 的真实服务响应建立失败，随后执行 Refresh，并包含已失败目录的恢复。
2. [分支 loading 释放](https://github.com/nocpiun/githux/pull/89#discussion_r4577492842)：手动 Refresh 提前设置 `branches_loading`，摘要失败后不会有 `apply_branches` 来释放它，独立 Retry branches 因而被一直挡住。回归注入摘要 404（不回退为缓存成功、不当作明确撤权），检查旧内容保留且分支重试可继续。

两条新增回归在未修生产路径上失败（runner `f7922e47-54c1-4c01-a92f-d7290b685dea`，0 通过/2 失败，测试 0.37s；槽等待 92.9s，实际编译/执行 91.5s）。修复将失败资源的清理放在同 commit 重验成功后：目录失败从任意来源清除，reader 的失败终态转为可重试 Loading，再按当前选择读取；此前已成功显示的 reader/tree 不动。在摘要仍挂起或失败时保留诚实的失败终态，避免制造无在途读取的 Loading。摘要错误路径清掉分支 loading 标记，让独立枚举重试继续。

本轮仓库边界 runner `51262267-3074-4709-a2e1-f8ab2f928946`：115 通过、0 失败；槽等待 198ms，实际编译/执行 138.0s（测试 6.44s）。回归进一步在网络挂起时确认保留失败终态，再等待真实新 revision provenance 发布，确认文件/目录重试成功。

本轮其余检查均通过：clippy（同生产 bin + repository test 目标，`-D warnings`）runner `36b1ba8c-7d48-4313-865c-280a4ab2f908`，槽等待 197.7s / 执行 13.9s；生产 build runner `e3187e19-9d40-4caa-885d-dba89dbcbdf0`，槽等待 211ms / 执行 40.5s；fmt check runner `22a41477-0780-47a5-ae18-bb242d3d2c1c`，不占槽 / 执行 26.3s。差异检查通过。重新运行的原因是上述两项生产修复与对应回归新增。

## CI 的既有 Profile 测试边界

`96d9dc5` 的 Codex 审查 clean（[收据](https://github.com/nocpiun/githux/pull/89#issuecomment-6891312410)），四平台构建和 PR Check 通过，但 [CI 43691590108](https://github.com/nocpiun/githux/actions/runs/43691590108) 的 bin 全量测试为 1402 通过、2 失败、3 忽略。失败均在此前未修改的 `src/app/pages/profile.rs` 测试模块；仓库连续刷新与隔离用例在 CI 通过。第一轮 CI 43689989481 也在其中的 partial/calendar 用例失败，而 alias 用例当时通过。

聚焦诊断 `cargo test --locked --bin githux app::pages::profile::tests:: -- --nocapture`（runner `5d8fbc8a-f7ea-4d09-89fe-61e81781887b`）在原测试代码上 28 通过；这只是 Windows 聚焦结果，不能冒充 Ubuntu CI 已通过。源码与 CI 状态确认的测试依赖：

- alias 用例未阻塞 Basic 网络，却必须观察到 Basic 缓存标记；网络先到时按正确的发布顺序抑制缓存，测试等不到标记，兄弟 work 仍被自己挂起。夹具增加独立 Basic/兄弟门闩，先确认缓存首屏，再只放行 Basic，确认它已在线更新且兄弟仍在途，最后放行兄弟，实际加强既定顺序覆盖。
- partial/calendar 用例的最终失败状态是 `Error(NotCached)`、work 全空；一次按 README 异步布局测得的坐标点击没有证明 Retry action 被送达。将此缓存主机用例的重试放在当前 `ProfileRequest` 的 `ProfileAction::RetryContributions` 边界，保留最终新鲜日历断言；真实 warning 标记点击仍由既有 UI 用例 `historical_and_partial_provenance_badges_and_section_retries_work` 覆盖。本轮合并运行 Profile 主机与 UI 边界，不把主机 action 调用声称为原生鼠标验收。

这些改动仅在 `#[cfg(test)] mod tests` 中；Profile 产品代码、公共 API、布局及账户/缓存规则没有改变。该 CI 修复不重新运行未改动的仓库 115 项；扩大到 Profile 相关检查是由真实 CI 失败引起的。

修复后的 `cargo test --locked --bin githux profile::`（runner `9309fa5d-3fbb-4612-9f79-c21927294360`）92 通过、0 失败，含主机与 UI 用例。槽等待 1200.6s，实际编译/执行 138.6s（测试本身 1.90s）；不把共享槽等待计作测试运行。

`cargo clippy --locked --bin githux --tests -- -D warnings`（runner `b21e4c3e-e818-43f1-b40a-a604ba6a7bc2`）通过，槽等待 149.3s / 执行 118.0s；fmt check（runner `ef7f9ace-73af-4dd5-aa77-de757a47f2a0`）通过，不占槽 / 执行 25.5s。差异检查通过。仅测试模块变化不重做本地生产构建；生产代码与 `96d9dc5` 相同，最新提交仍按完整远端 CI 与 Codex 流程核验。

## PR #89 第三轮审查：取消的正文必须结算

Codex 对 `61fa7c9` 提出 [P2：刷新失败后取消的正文永久 Loading](https://github.com/nocpiun/githux/pull/89#discussion_r4577709453)，成立且可直接修复，无待裁决。持有 `src/main.rs` 的真实 blob 请求后 Refresh 会取消其旧代次；若摘要或 ref 返回 404，原实现只记录页级错误，正文没有任何请求拥有其 Loading。

两条独立传输门闩回归在未修路径上均失败（runner `1204919f-8450-4c5f-8e10-4ff8d1be46b5`，0 通过/2 失败，测试 0.65s；槽等待 24.6s / 实际编译执行 90.8s）。修复在摘要、revision、替换准备和根树错误路径共用结算：保留快照存在、正文仍 Loading 且没有当前正文请求时，发布可 Retry 的终态；成功显示的 reader 或刷新后新开始的正文请求不被覆盖。现有 generation/selection request 校验继续拒绝被取消的旧回复。回归还验证正文可独立重试恢复，并加入“刷新后另开文件”的新请求仍能完成的保留场景。

在本轮准备期间，`61fa7c9` 的 [完整 CI 43695709389](https://github.com/nocpiun/githux/actions/runs/43695709389) 已通过（四平台构建与全量 code-check 均通过），确认 Profile 夹具修复通过远端验证。该收据属于父提交，不代替本轮新 head 的 CI/review。

本轮本地检查全部通过：仓库 118 项（runner `d574fbc3-f27a-403d-9311-b9e1d025702f`，槽等待 707.5s / 编译执行 146.0s，测试 8.97s）；clippy 生产 bin + repository test，`-D warnings`（`bc03ff85-8faa-48c9-a45b-97aea4f78ae4`，等待 610.9s / 执行 15.9s）；生产 build（`95378309-97cf-46da-bd19-9604b12fed56`，等待 120.3s / 执行 47.0s）；fmt check（`cae0ae70-738c-4285-a8cb-f759352819d8`，不占槽 / 执行 23.3s）与差异检查。重新检查由本轮实际生产错误路径修复与 3 项边界新增触发。

## PR #89 第四轮审查：统一结算取消的局部读取

Codex 对 `f9bf491` 提出 [P2：摘要失败后取消的 TOP 分组永久 Loading](https://github.com/nocpiun/githux/pull/89#discussion_r4577843399)，成立且可直接修复，无待裁决。沿同一取消所有权检查文件树后，也复现了被取消的展开目录没有局部终态的问题。三条新门闩用例在修复前失败，上一轮两条正文用例仍通过（runner `f9c0f6af-1a1c-4c4b-9d78-6076ee23a19d`，2 通过/3 失败，测试 1.01s；槽等待 455.6s / 编译执行 91.5s）。

上游失败现在统一结算没有当前请求负责的正文、已知且缺数据的根树/展开目录、TOP 分组；成功值、已有错误和刷新后新发起的读取均保留。分组的当前 work 在接受其最终回复时清除，避免把已完成的缓存预读当成仍在读取；预读 MISS 晚于摘要失败结束，也会进入可 Retry 终态，真实历史值则继续展示。补充用例覆盖 scoped Retry 恢复、有效兄弟组与 Reader 不变、新分组请求继续运行、缓存预读 MISS 晚到，以及账户 epoch 未变的仓库撤权在宿主动作发布前阻断 Reader 旧动作。没有修改 `repository_overview/summary.rs`、`tab_strip` 或共享分组 API。

最终仓库边界整组 124 项通过（runner `55af66ad-0ad9-4018-a868-50bfe7a7adf1`，槽等待 0.2s / 编译执行 96.9s，测试 7.83s）。首次本轮检查 `98779454-9c5e-46f7-bd46-44461527c22b` 止于新增测试的显式导入错误，没有运行用例；修正导入后才记录本次通过。clippy 生产 bin + repository test、`-D warnings` 通过（`edfd19c7-1e62-4368-802c-43da4b6ea2e5`，等待 1261.5s / 执行 20.5s）；生产 build 通过（`1c522081-9dc0-4d2f-943c-c1eed6206e31`，等待 7.5s / 执行 54.0s）；fmt check 通过（`c0a568cd-5c80-4a63-8fe0-f3643c1ab06f`，不占槽 / 执行 20.8s）。最新 head 的 CI/review 按 PR 当前提交收据核对，父提交的 CI 不能代替；负责人原生视觉验收仍单独 pending。可复用的取消所有权教训已纳入 [L029](../../LESSONS.md#l029取消刷新工作后必须交接加载态的所有权)。

交付前同步 `main` 的 `5a3eab3`（PR #90，仅工具/文档）：保留其 L028，将本任务条目编为 L029。333 个 Rust/Cargo 输入文件在同步前后的归一化 SHA-256 同为 `184505fd129e9ee5c4a82cc688339a236b5cf0be89c925738bc38597f0097c38`，因此沿用同内容的 124 项 Rust 边界与 lint/build/fmt 收据；同步后的派发组合测试 20 项通过（84.98s）。新的 CI 仍需验证当前 head 与该基线的实际组合，不沿用旧 CI。

## 2026-11-18：负责人本地 release 构建反馈复核

负责人反馈本 worktree 本地无法编译；协调者观察到其它任务的 release exe 已产出，而本任务没有 `target/release/githux.exe`。此前的 Debug 构建、Debug CI 与 GPUI 用例不作为 release 构建成功的证据。

先在未修改的 `43d5eb1` 复核：fmt check 通过；`cargo build --locked --release --bin githux` 确实失败（runner `84a53be6-fd7e-44bf-8412-dd333e5b1663`，槽等待 0.193s / 执行 139.848s，exit 101、未超时），错误为依赖 `rustybuzz` 读取标准库 `core` 时 `only metadata stub found for rlib dependency core`，尚未编译到 `githux`。同一原始 HEAD 的独立 `cargo check --locked --release --all-targets` 通过（`d79d9828-f447-46b8-8b0b-2953e9a5e547`，等待 0.202s / 执行 467.826s），包含 examples 的 release 类型检查。诊断 wrapper 的第三个 Node 子进程曾在 Cargo 启动前异常退出；独立检查的 runner 收据才计入结果。

按负责人要求执行 merge，同步到 `main` 的 `4e75ad8`（#93；当次 fetch 尚无 #94），保留本任务 L029。默认 GPG 程序无法执行，备用程序也不能连接现有 keyboxd；仅对本任务的本地提交使用单次 `commit.gpgsign=false`，没有修改全局配置或密钥库。随后在合并后的代码上继续完整 release 构建并记录编译命令。终端和直接调用均为 Rust 1.98.1 / x64 MSVC，同一 sysroot，未发现 Rust/Cargo 环境覆写；标准库独立 `.rmeta` 文件存在。依赖元数据错误没有在本次重编译中再次出现，仍不将其推断为已证实的应用源码缺陷或特定缓存损坏原因。

合并后的完整 `cargo build --locked --release --bin githux -vv` 通过并产出 release exe（`9704a87d-0b46-461b-a997-28e0e4acfdd5`，等待 0.181s / 执行 989.191s，未超时）。本机这轮完整优化编译实际超过 600 秒缺省工具时限；负责人原次失败没有完整日志，不能据此断言同一原因。本轮 Cargo 工具调用显式使用 3600 秒预算，长命令在同项目的持续检查终端中完成。

合并后的 release 全目标检查通过（`d8c0adb8-1ce8-438a-9707-bab06b3e80db`）。随后默认并发的全目标 clippy 在主程序测试目标发生编译器 OOM（`cfec6274-4d41-4379-9d6f-ffbc220f2863`，`memory allocation of 2097152 bytes failed`，没有 lint 诊断）。机器有 32 个逻辑核、约 32 GiB 物理内存，失败后可用提交内存约 5 GiB；共享 mutex 只串行 Cargo 命令，不限制一个 Cargo 内的多目标并发。剩余本地验证使用命令级 `--jobs 1` 控制峰值，不修改全局或项目默认配置。该 OOM 是独立的已确认失败，不据此倒推首次标准库元数据错误的原因。

最终在合并后的同一应用源码上完成以下复核（只有本节日志为待提交改动；所有 Cargo 保留共享 mutex）：

| 检查 | runner | 构建槽等待 / 实际执行 |
| --- | --- | --- |
| fmt check | `0c88ec35-9153-4a1e-bdc5-1acfcd8030d5` | 通过 |
| release all-targets check（含 examples） | `d8c0adb8-1ce8-438a-9707-bab06b3e80db` | 通过 |
| all-targets clippy，`-D warnings`，`--jobs 1` | `3baf4947-225e-496c-ab96-9055af561d25` | 1791.246s / 70.150s |
| debug bin build，`--jobs 1` | `e1b80ffa-a485-4d71-9740-bb0c36cfcdec` | 23.355s / 52.390s |
| release bin build，`--jobs 1` | `290d0718-5cff-4752-a865-a379d1cbfbc7` | 54.871s / 7.015s |
| `--test repository continuity_`，`--jobs 1` | `f12de51b-720e-4237-af5c-dc0920b84923` | 305.458s / 70.635s；21 项全过，测试 2.98s |

两种配置均执行 `prepare-windows-runtime.ps1 -Destination target/<profile>` 并最终复核成功，各有 27 个经过固定包校验的 Runtime/PRI/许可证/manifest 文件，包含 `DeploymentAgent.exe` 与 `RestartAgent.exe`。PE 校验确认两个 `githux.exe` 都是 x64 Windows GUI 子系统；debug 为 128,370,176 bytes，release 为 99,632,640 bytes。产物及文件摘要保存在本任务忽略的验证记录中，不上传二进制或原始日志。可直接从工作树运行 `target/debug/githux.exe` 或 `target/release/githux.exe`，无需再等待一次优化编译；保留 exe 同目录的运行时文件。

构建复核阶段没有修改 Cargo.lock、依赖、工具链、运行时脚本或仓库刷新实现；该阶段源代码变化来自按要求同步的 #93。构建和暂存成功不代替负责人对真实缓存刷新连续性的 Windows 视觉验收，PR #89 继续保持 `pending`，不合并。新提交仍单独等待最新 head 的 CI/Codex。

### 本轮审查：切分支中断 Refresh 的分组后继

`ce52766` 的审查提出 [P2](https://github.com/nocpiun/githux/pull/89#discussion_r4611509374)：Refresh 已取消分组读取，用户在摘要完成前切分支会取消摘要并推进 branch request，摘要回复被拒绝，未显示的分组没有任何后继。两条真实传输门闩回归及一条迟到缓存预读 MISS 的发布边界回归在修复前全部失败（`82ce3b82-5f5c-4f76-8d48-f5cae310a05e`，0 通过/3 失败；槽等待 204.789s / 编译执行 94.346s，测试 0.63s）。该意见成立且可直接修复，无待裁决。

分支导航现在清除已取消摘要/枚举的加载标记，并只结算无当前工作负责的 TOP Loading。中断原因保留到下一次仓库刷新，覆盖本地缓存预读晚于导航完成才返回 MISS 的时序；实际历史值和在途新请求继续保持，局部 Retry 仍使用当前仓库绑定。补充用例验证分支已成功切换、兄弟值与 overview 实体保留、分组 Retry 恢复且不重建新分支 Reader。此修复仅涉及仓库状态/回归和主题文档，不改并行组件的样式。

修复后仓库整组 127 项通过（`13cf50ae-23f8-4323-b0ad-c6ecb8cba4a7`，等待 3.703s / 编译执行 152.326s，测试 8.04s）；release all-targets check 通过（`68d04a4d-f00c-408f-a13c-b412335638d0`，97.149s / 114.296s）；严格 all-target clippy 通过（`59cf1f94-878b-4f48-a9ff-ac7b45e8a400`，199.068s / 54.882s）；debug/release 构建通过（`9a7c7d30-d51e-449d-867c-85f112cfa449`，123.949s / 58.521s；`49c717a3-4ba0-4715-b4eb-4bee5e760b1d`，162.911s / 224.001s）；两套 Runtime 复核通过，fmt check `b7eff223-3439-4621-8ad9-33395a8d21b5` 通过。重建后的 debug/release exe 均为 x64 GUI，各有 27 个 Runtime 文件。

### 最终基线组合：同步 main #95

收尾 fetch 发现 `main` 新增 `78d59400112225a069332e1c6c2dfef4d79b5f3b`（#95，Home/Profile 测试稳定化），已合入为 `73fdda9f14a72a6545150364830aa4d4b5fcfb7f`。Profile 的旧夹具修复与 main 新版冲突，选择已合入 main 的完整测试实现；合并前确认 `#[cfg(test)] mod tests` 之前的生产代码前缀完全一致，Home 改动也只在 `#[cfg(test)]` 模块。最终 PR 对 Profile 不再有独立差异。仓库刷新源码与 127 项边界收据对应输入不变；补跑受影响的页面组合和全目标配置，并再次重建两套产物。

以下全部成功；时间按共享槽等待 / 实际执行分列，单位秒：

| 检查 | run ID | 等待 / 执行 |
| --- | --- | --- |
| fmt check | `0f83cb4e-7324-4c19-b0ad-c02cf2ff9431` | 0 / 21.380 |
| Home/Profile 页面组合 66 项（测试 5.75s） | `4993377f-4cda-499d-9df8-c5c1e367cccc` | 339.254 / 128.628 |
| release all-targets check | `10747d1d-50b0-4803-a63c-6c3bf9aefbb0` | 264.643 / 86.589 |
| strict all-target clippy | `b78cf6d2-b3fc-438d-b996-a37ccb7175ad` | 911.689 / 44.806 |
| debug bin build | `ccfa47a9-4c9e-4fba-91e1-d48c5f530a64` | 198.371 / 23.557 |
| release bin build | `b76d177f-afc7-49e3-9e59-53cb0e1f662d` | 226.786 / 220.835 |

两套 SDK 暂存与末次文件复核均通过，各 27 个文件。最终 `target/debug/githux.exe` 为 128,371,712 bytes，`target/release/githux.exe` 为 99,632,128 bytes；二者 PE 均为 x64、Windows GUI。末次 fetch 的 main 仍为 `78d5940`。以上验证代码组合之后仅补此日志，不改变生产或测试输入；最新文档提交仍须重新等待对应 head 的 CI/Codex。没有记录应用实际启动或负责人视觉验收成功，继续保持 `pending`，不合并 PR #89。

## 可复用教训

缓存先行的测试需要检查“在线 revision 已返回，但新树/文档未准备好”的中间绘制状态。仅断言网络放行前有缓存、全部完成后有最新内容，会漏掉两个断言之间的清空/重建。保留显示资源与更新读取代次是两个操作；保留旧画面时也必须保留其 carried authorization，而不是用新请求的令牌替换它。

## 验证边界

使用临时 SQLite/CacheStore、合成账户与可控 transport 的 GPUI 组合测试，不依赖 GitHub 实网或真实 PAT。原生视觉验收由负责人完成，PR 保持 `pending`，不合并。
