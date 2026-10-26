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
cargo test --locked                       → 200 passed（其中本任务 11 项）
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

## 剩余问题

- 真实 PAT、真实组织 SSO、私有仓库可见性与中文输入法/粘贴/焦点，须由负责人在预览窗口确认。
- 正式 Shell 接入：切账户时标签处理、移除账户 UI、私有缓存保留政策、正式仓库列表组件与头像下载仍未定。
- 本预览的仓库区只是接线验收，不是正式侧栏组件。
