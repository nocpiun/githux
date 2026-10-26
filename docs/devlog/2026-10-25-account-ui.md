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

## 剩余问题

- 真实 PAT、真实组织 SSO、私有仓库可见性与中文输入法/粘贴/焦点，须由负责人在预览窗口确认。
- 正式 Shell 接入：切账户时标签处理、移除账户 UI、私有缓存保留政策、正式仓库列表组件与头像下载仍未定。
- 本预览的仓库区只是接线验收，不是正式侧栏组件。
