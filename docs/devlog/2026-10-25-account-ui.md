# 真实登录与 GPUI 桥接预览

任务：`account-ui`，分支 `feature/account-ui`。目标是把已有的账户服务接进一个**可运行的 Ely 预览**，让负责人能亲自输入 PAT、看到真实已认证账户与完整可见仓库读取结果。

## 结论

- 新增 `cargo run -- --account-preview` 可选入口：系统标题栏、独立窗口，不接触现有 Shell、sidebar、tabs 与其他 Agent 的布局改动。
- 复用 `AccountService::open`、应用数据目录 SQLite、平台凭据、`GitHubClient`、`BackgroundExecutor` 与 `RepoService`，**没有**新增 HTTP 客户端、SQLite 路径或账户服务。
- 账户添加、选择、替换、重新验证与仓库读取均经过后台执行；UI 只在 GPUI 线程接收并应用结果。
- **真实 PAT 验收未做。** 负责人需自行在预览里输入 token；本轮没有任何真实凭据读取或真实 GitHub 请求。

## 实现

| 文件 | 内容 |
| --- | --- |
| `src/app/accounts/mod.rs` | GPUI 桥接：掩码输入、操作 generation、结果应用、上下文复核、取消、关闭 |
| `src/app/accounts/runtime.rs` | 生产/测试组装：应用数据目录、平台凭据、真实传输、执行器、收尾 |
| `src/app/accounts/tests.rs` | 8 项真实 GPUI 视图 + 临时 SQLite + 合成传输集成测试 |
| `src/ui/accounts/{mod,view}.rs` | 可复用账户/仓库预览视图，无 I/O |
| `src/app/mod.rs` | 读取 `--account-preview`，默认 Shell 路径不变 |
| `src/ui/mod.rs` | 仅新增 `pub mod accounts;` |

## 设计取舍

1. **PAT 生命周期短**：输入实体在提交时整体重建，连同撤销历史一起丢弃；token 只作为 `Pat` 进入后台任务，不进 Presentation、状态消息、reply 或 Debug。
2. **两道回写校验**：单调 generation 负责“同一界面上的新旧请求”；`RepositoryReadContext` 负责“同一个账户授权上下文”。仓库读取在请求前捕获上下文，接收前用 `is_current` 复核。
3. **账户变更不套用仓库上下文检查**：登录与替换本身推进 epoch，属预期结果；否则会把成功误判成过期。
4. **不用轮询等待通道**：后台只入队，前台按 50ms 节拍取出。GPUI 测试调度器会把跨线程唤醒判为不确定，这个结构同时满足确定性测试与“render 内无 I/O”。
5. **部分成功如实显示**：`CleanupPending`、`Partial`、取消、SSO、限流、缺凭据分别有独立文案；没有一条路径把失败渲染成“成功空列表”。

## 验证证据

```text
cargo fmt --all                 # 已格式化
cargo check --locked --all-targets        → ok
cargo clippy --locked --all-targets -- -D warnings → ok
cargo test --locked                       → 205 passed（其中本任务 16 项）
cargo build --locked                      → ok
```

8 项预览测试覆盖：登录后仓库进入视图；掩码 + 提交后清空 + Debug 不含 token；Tab 到提交按钮；重复提交不推进；替换/切换/重验证/重启恢复（恢复后为 `PendingVerification`）；401/限流不冒充空列表；Partial 保留已取数据；取消后旧 generation 不覆盖新结果；请求期间切账户丢弃旧仓库结果并结束加载态；视图关闭后不再发布。

真实窗口：`githux.exe --account-preview` 在本机启动并保持运行后被手动关闭；无参数启动仍进入原 Shell。两者都不代表真实 GitHub 联调。

## 第 2 轮：审查修复

Codex 首轮提出两条 P2，均属可直接修复范围：

1. **取消后账户显示可能与存储不一致**：取消清空 `operation` 后，reply 永久被丢弃，而已提交的跨存储变更仍会写入 SQLite 与注册表。修复：后台任务结束时压入 `Settled(generation)`，前台对账服务权威的账户列表与当前账户；被取消操作的 reply 仍丢弃，也不采用它的仓库结果。回归测试 `cancelling_after_a_committed_mutation_reconciles_saved_accounts`。
2. **重新验证不参与取消**：`AccountService::revalidate` 没有可取消变体，内部总会提交运行时核验并推进 epoch。这里不假装网络已取消，也不在 data 层外伪造取消语义：取消只停止发布，任务照常收尾并触发上面的对账，界面不显示被取消操作的成功。回归测试 `cancelling_revalidation_does_not_publish_its_late_success`。若后续需要真正可中断的 revalidate，应在 data 层新增 `revalidate_cancellable`，不在 UI 层模拟。

对账信号的投递方式也调整过：`oneshot` 完成会在网络线程唤醒前台任务，GPUI 测试调度器把它判为非确定性；改回“后台只入队、前台按 50ms 节拍取用”，与首轮实现一致。

## 第 3 轮：审查修复

Codex 第二轮提出第 3 条 P2：账户变更以错误返回时，`repo_service` 的旧结果仍显示为 Complete。`AccountService::mutate` 在 `CleanupPending` 之前已经提交了目录与活动选择，所以错误 reply 同样推进了权威账户状态。

修复：`apply` 的错误分支对**所有非仓库读取**操作都作废仓库展示，不只对仓库读取错误作废；成功分支此前已如此处理。回归测试 `an_errored_mutation_clears_repository_results_for_the_previous_account`（A→B 读仓库→切回 A 后旧结果消失）。预览套件 11 项，全部 200 项通过。

## 第 4 轮：审查修复

Codex 第三轮两条 P2：

1. **输入不触发父视图重绘**：按钮的 `can_submit` 属于 `AccountPreview`，但没有订阅 `TextInput` 变化，输入或粘贴后按钮保持禁用。修复：`make_input` 返回实体与变化订阅，`Changed` 时 `cx.notify()`；`reset_input` 替换订阅。回归测试 `typing_a_token_enables_the_submit_button_without_another_operation`。
2. **按键订阅持有强引用形成环**：`intercept_keystrokes` 捕获强 `Entity`，而预览持有该订阅，导致视图释放后不执行 `Drop`，请求与运行时收尾都不发生。修复：改为 `cx.weak_entity()` 并忽略失败的 update。回归测试 `the_keystroke_subscription_does_not_retain_the_preview`。

修复过程中另发现并修掉一个真实缺陷：`watch::Sender::send` 在没有 receiver 时返回错误且**不写入**，关闭标志因此一直是 `true`，后台任务仍会入队。改用 `send_replace`，测试直接覆盖释放后 `alive == false`。

预览套件 13 项，全仓 202 项通过。

## 第 5 轮：审查修复

Codex 第四轮 P2：`reconciling` 只保存一个代次。先取消 A 的重新验证、再取消 B 的，A 的待对账标记被覆盖；B 结算后清空标记，A 晚到的核验结果就再也不会反映到界面。

修复：改为 `Vec<u64>` 记录全部未结算的已取消变更，逐个移除；只有最后一个结算才显示汇总提示，避免中间态反复覆盖。回归测试 `every_cancelled_mutation_is_reconciled_independently`（按任意顺序结算两项）。预览套件 14 项，全仓 203 项。

## 第 6 轮：审查修复

Codex 第五轮 P2：上一轮把待对账改成了列表，但“作废仓库结果”只在最后一个结算时执行。若 A 的重新验证先结算并记录了授权失败，`refresh_presentation` 会显示新的授权状态，而 A 账户的仓库结果仍标为 Complete 直到 B 也结算。

修复：每次结算都作废仓库展示并刷新权威账户状态；仍只在全部结算后给出汇总提示。回归测试扩展 `every_cancelled_mutation_is_reconciled_independently`，在每次结算前放入加载态并断言被作废。

## 第 7 轮：审查修复

Codex 第六轮两条 P2：

1. **reply 快照可能覆盖较新的对账状态**：`send_reply` 在 await `recovery_pending()` 之前抓账户快照；期间被取消操作的结算可能已经更新了其他账户的授权状态，随后旧快照被应用。修复：`Reply` 不再携带账户快照，应用结果时用 `refresh_presentation` 读取服务权威状态。
2. **全局待清理覆盖本次错误**：`recovery_pending()` 是目录级状态，不代表本操作已提交；无条件覆盖会把 401 等真实失败改写成“已提交”。修复：保留操作自身结果，把待清理作为追加警告。

新增回归 `a_failed_operation_keeps_its_error_and_presentation_comes_from_the_service`。预览套件 15 项，全仓 204 项。

## 第 8 轮：审查修复

Codex 第七轮 P2：关闭预览时 `app::run` 立即退出，而 SQLite/凭据提交的排空在线程中异步进行，进程退出会截断它；线程内部的 5s 等待并不阻止进程结束。

修复：新增进程退出闸门（`OnceLock<(Mutex<usize>, Condvar)>`）。开始关闭前计数，SQLite 关闭与执行器 join 完成后才减到 0；`app::run` 在预览模式下于事件循环返回后调用 `wait_for_shutdown()`，最多等待 10s，使卡住的平台凭据调用也不能无限阻塞退出。回归测试 `shutdown_waits_for_background_teardown`。

预览套件 16 项，全仓 205 项。

## 第 9 轮：Ely / GPUI 0.3.10 迁移（同步 main 后）

负责人在 PR #30 合并主线迁移后授权原 Agent 完成本任务剩余迁移；本地先由主 Agent 合入 main `7ed70cda`（merge commit `c650e80a`），未推送。

改动范围只有本任务文件：

- `src/app/accounts/mod.rs`、`src/ui/accounts/view.rs`、`src/app/accounts/tests.rs` 的旧 `ely_gpui_component::*` 全部改为 `ely_core::*`；测试初始化改用 `ely_core::init_for_tests`。
- 未新增依赖：Cargo 已是官方仓库 pin `30c66fe1` + `ely-core`（`default-features=false`，保留 `runtime_shaders`）+ GPUI `=0.3.10`；没有 vendor、fork、patch 或缓存修改。
- 业务层（`data`/`auth`/`github`/`network`/`storage`）未改动；`src/app/mod.rs` 保留主线的 `ely_core::init`、`move` 闭包与 account-preview 分支，仅核对完整启动路径。

迁移做了行为核对，不只是改 import：

| 关注点 | 结论 |
| --- | --- |
| 掩码输入 | `TextInput::masked()` / `is_masked()` / `set_text` 在新版本签名不变；提交后重建实体仍清空旧值与撤销历史 |
| 粘贴 | 新版本仍用 `secondary-v` 绑定 `Paste` 读取剪贴板；新增 `paste_and_cjk_text_stay_masked_and_submit` 用真实剪贴板与 `ctrl-v` 验证，粘贴后仍为掩码 |
| 中文输入 | 同一测试经 `simulate_input` 输入中日韩文本，断言文本被接受且仍掩码；组合态 Enter 仍由按键拦截器让给候选列表 |
| Tab / Enter | `PasswordInput` + 提交按钮的焦点顺序未变，`tab_reaches_the_submit_button_and_enter_submits` 仍通过 |
| 事件/按键 API | 预览使用 `cx.spawn`、`intercept_keystrokes`、`Keystroke::parse` 静态字面量，不受 `ScrollEvent`/`KeyBinding::load` 迁移影响 |
| 取消/回写隔离 | 16 项既有回归全部保持通过，没有为升级放宽任何断言 |

本机检查（`-j 1`，独立 target）：fmt、locked all-target check、`clippy -D warnings`、`cargo test --locked`（**362 passed**，其中本任务 17 项）、`cargo build --locked`、`git diff --check` 全部为 0 退出码。

真实 GUI 证据只到启动路径：`--account-preview` 启动后窗口标题为 `Githux · Account wiring preview` 且 Responding，默认启动标题为 `Githux`；两者按 PID/创建时间核对后由本任务关闭，未触碰其他任务进程。**真实 PAT 仍未验证**，没有读取任何凭据。

## 第 10 轮：同步 main（统一滚动条、PR 门槛）并接入公共 scroll

负责人授权同步 main 后由原 Agent 完成适配。本地 main 同步提交 `9d91eb7`（合入 `0014d6c`：PR #29 统一滚动条、PR #31 PR 标签/规模门槛），未推送；分支、原 PR 与裁决历史保留。

**接入方式**：预览原本已有真实 `ScrollHandle` 的纵向溢出区域，直接把该区域接到 `ui::scroll`：预览持有自己的 `Bars`，`hover` 绑定整个区域，`drag` 交给 `Bars::drag`，窗口失活与视图释放走 `Bars::cancel`，外观用 `Style::DEFAULT` + `Placement::overlay(4.0, 8.0)`。没有复制几何/拖动/Fade，没有为仓库行另建滚动区，`ui/scroll` 只把模块入口放宽为 `pub(crate)`。

**排查与结论**：过程中多次把“内容变短后 offset 未归零”当成缺陷，逐层验证后确认是实现正确的表现——缩短后仍溢出 98px，因此 reach 仍为 98px，GPUI 已把 offset 从 −70.5 收敛到合法范围，滚动条仍应显示。我据此把测试断言改为验收真正要求的“offset 不越界 + 末行消失”，而不是错误地要求归零；无溢出场景改用宽窗口下的空预览单独验证不绘制条。整个过程没有保留临时调试输出，也没有为让测试变绿放宽断言。

**验证**：fmt / check --locked --all-targets / clippy -D warnings / `cargo test --locked`（378 passed，本任务 19 项）/ build / `git diff --check` 全部通过（`-j 1`，本 worktree 独立 target）。真实窗口按 PID 与创建时间核对：预览标题 `Githux · Account wiring preview`、默认外壳标题 `Githux`，均 Responding，随后由本任务关闭。

## 第 11 轮：独立 Cargo example 与正式 binary 隔离

负责人合入 content-reader（`dd2b1f2`）后，要求 account-ui 按 reader 的独立 example 规范交付预览。本地同步提交 `cd7516c` 将最新 main 合入分支，未推送；本任务没有重放同步或强推。

### 最终装配

- 新增 `examples/account.rs`，命令改为 **`cargo run --example account`**。它使用原生标题栏、Ely Assets/init、相同深色主题与真实字体；窗口关闭后在事件循环返回处调用既有有界排空，关闭行为没有因迁移丢失。
- 新增 `src/bin_support/account_modules.rs`，以薄 `#[path]` 模块装配复用真实账户视图、GPUI 桥接和共享滚动条；`tests/account.rs` 用同样薄入口真正执行原 19 项账户测试，没有复制测试或业务代码。
- 正式 `src/app/mod.rs` 移除 `--account-preview` 参数识别、预览窗口分支及预览退出等待；`src/ui/mod.rs` 不再声明账户预览模块。正常产品启动没有因预览打开数据库、凭据或网络。
- 没有新 Cargo package、通用预览注册体系、新依赖或正式调试入口；reader 的 example 和测试未被修改。

### 产品与发行边界

产品 binary 的模块图不再包含预览启动器；`cargo run`、`cargo build --release` 都只进入产品 Shell。标准 example 不是默认 release 构建目标。CI artifact 从 `target/<target>/debug/githux` 复制产品 exe 和必需资源，没有打包 `target/debug/examples/account.exe` 或 reader example。此结论来自实际模块装配与 CI 打包路径核对，不只是 release 字符串搜索。

### 验证

`-j 1`、本 worktree 独立 target：

- `cargo fmt --all -- --check`、`cargo check --locked --all-targets`、`cargo clippy --locked --all-targets -- -D warnings` 通过。
- `cargo test --locked`：主 binary 456 项、账户装配 305 项、reader 装配 118 项全部通过；本任务 19 项真实运行，没有因 example 迁移丢失。
- `cargo build --locked --example account` 与 `cargo build --locked --release` 均成功。
- `git diff --check` 通过。

真实启动：独立 `D:\githux-account-ui\target\debug\examples\account.exe` 启动两次（PID 7660，创建时间 2026-10-28 08:18:40；PID 64932，08:19:02），窗口标题 `Githux · Account wiring preview`，均 Responding 后按 PID 关闭。`target/release/githux.exe` 启动（PID 54860，08:19:21），进入标题 `Githux` 的产品 Shell 后按 PID 关闭。只操作本任务进程。

原 `cargo run -- --account-preview` 及 app 内预览分支的交付仅属历史；新命令是 `cargo run --example account`。真实 PAT、真实 Windows IME/粘贴/焦点与滚动条观感仍待负责人在新 example 中验收，不能沿用 reader 的验收。

## 第 11 轮补充：sidebar-layout 合入 main 后同步

首次新 head 审查返回 `base_changed`：sidebar-layout 已合入 main（`c1a4ad8`）。普通 merge 保留账户 example、正式 Shell 与 sidebar 实现，没有重放同步或强推。

组合验证：主 binary 519 项、账户装配 310 项、reader 装配 123 项通过；example 与 release 均构建成功。新组合中独立 example 启动（PID 10620，2026-10-28 08:28:18），标题 `Githux · Account wiring preview`；release 产品启动（PID 75204，08:28:39），标题 `Githux`，均按 PID 关闭。没有改 sidebar 的交互。

## 剩余问题

- 真实 PAT、真实组织 SSO、私有仓库可见性，以及真实 Windows 输入法候选、实际粘贴与焦点观感，须由负责人在预览窗口确认（模拟覆盖不等同）。
- 正式 Shell 接入：切账户时标签处理、移除账户 UI、私有缓存保留政策、正式仓库列表组件与头像下载仍未定。
- 本预览的仓库区只是接线验收，不是正式侧栏组件。
