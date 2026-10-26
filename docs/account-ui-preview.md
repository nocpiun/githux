# 账户与仓库读取预览（account-ui）

更新：2026-10-25。本文件记录 R2 真实登录与 GPUI 桥接的**接线验收预览**，不是正式 Shell 账户 UI，也不代表真实 PAT 联调已完成。账户服务、SQLite 目录、系统凭据、GitHub 读取与后台执行器沿用已有实现，没有新增 HTTP、SQLite 或账户服务路径。

## 启动

```sh
cargo run -- --account-preview
```

不带参数时仍是原有 Shell（启动 Home、fixture 侧栏与本地导航），本任务不修改其字段与布局。预览使用系统标题栏与独立窗口，不复用其他 Agent 正在修改的自定义标题栏/sidebar/tabs。

`ui/mod.rs` 只增加了 `pub mod accounts;` 声明。`app::run` 读取 `--account-preview` 后打开预览窗口并跳过 Shell 构建；默认路径代码未变。

## 能做什么

| 操作 | 行为 |
| --- | --- |
| 添加账户 | 掩码输入 Classic PAT，Enter 或按钮提交；先 `GET /user` 验证身份，再写系统凭据与 SQLite 目录 |
| 取消 | 取消等待中的请求；已投递的跨存储提交或在途 `revalidate` 继续在服务中收尾，完成后前端**对账**权威账户列表与当前账户，不发布被取消操作自身的成功消息 |
| 选择已存账户 | 调用 `AccountService::set_active`，先落盘再显示 |
| 替换 PAT | 只对指定账户使用 `AccountService::replace`；属于别的账户返回 IdentityMismatch，两个账户都不改 |
| 重新验证 | `AccountService::revalidate`，恢复账户从 `PendingVerification` 变为本次运行的真实核验结果 |
| 仓库预览 | `RepoService::list_repos_cancellable` 读取 `/user/repos` 全部分页，显示 namespace/repo、计数、Complete/Partial 与加载/错误 |
| 帮助链接 | 打开 GitHub 官方 Classic PAT 创建页，不自动创建 token，不修改账号设置 |

重启后从 `accounts.sqlite3` 恢复目录，条目一律是 `PendingVerification`，不会被当作当前已验证授权。真实凭据仍在 Windows Credential Manager；其他平台仍是 `Unsupported`，没有明文回退。

## 安全与错误边界

- PAT 只存在于掩码输入实体与提交到服务的那一份；提交后立即重建输入实体，旧文本和撤销历史一并丢弃。普通视图状态、状态消息、reply 结构和 Debug 输出都不含 token。
- 输入提交在按键拦截器里处理，输入法组合期间 Enter 留给候选列表。
- 每次操作有单调 generation；旧 generation 的成功与错误都被丢弃。取消后再次发起同类型操作不会串结果。
- 仓库读取在**请求前**捕获 `RepositoryReadContext`，接收结果前用同一个 `RepoService::is_current` 复核；请求期间切换账户、替换 PAT 或移除账户时，成功与错误都被丢弃，界面明确结束加载状态而不是停在“读取中”。
- 登录/替换会推进授权代次，这是预期结果，因此不拿请求后的上下文去否定它。
- 账户列表与当前账户在**应用结果时**从服务重新读取，而不是使用 reply 里操作期间捕获的快照：无关变更的结算可能已经更新了其他账户的授权状态，旧快照不能把它覆盖回去。
- 目录级“凭据清理待完成”作为独立追加提示，不把本次操作的失败改写成“已提交”，也不把全局待清理当作本操作成功的证据。
- `CleanupPending` 显示为“已提交但凭据清理待完成”的部分成功，不显示完整成功，也不盲目重放。
- 401/403/SSO/限流/404/超时/断网/取消/缺凭据分别提示，**都不会显示成成功空列表**；只有服务返回 `Complete` 且为空时才说明“完整的空集合”。
- 任何账户变更（选择、替换、重新验证、添加，无论成功、失败或部分成功）都会作废当前显示的仓库结果：`CleanupPending` 这类错误是在目录与活动选择已经落盘之后返回的，旧账户的“完整”列表不能留在屏幕上。
- 取消后对账按**每个**未结算变更记录，不是只保留最近一次：先后取消两个操作时，较早的那个晚到仍会对账。**每次**结算都会作废仓库展示（授权证据可能已变），全部结算后才给出汇总提示。
- 取消后对账：账户变更有可能在取消前已提交（跨存储提交与 `revalidate` 的运行时证据都不能中断）。后台任务结束时只压入 `Settled` 标记；前台收到后重新读取服务权威的账户目录与当前账户，被取消操作的 reply 仍然丢弃，也不会带入它自己的仓库结果。
- 输入变化会订阅并重绘父视图，输入/粘贴后提交按钮立即可用；提交重建输入实体时一并替换该订阅。
- 快捷键拦截器只持弱实体引用，预览不会被自己的订阅长期保留；视图释放即可执行 `Drop` 清理。
- 关闭标志使用 `send_replace`：没有任何接收者时 `send` 不写入，会让后台任务误以为仍可回写。
- 窗口关闭或视图释放：先停用消息入队、取消请求、中止预览任务，再在独立线程中关闭数据库并让执行器按既有契约收尾。

## 线程与桥接

| 层 | 位置 | 职责 |
| --- | --- | --- |
| 视图 | `src/ui/accounts/**` | 只渲染 Ely 组件与纯展示状态，不构造服务、不读凭据、不发网络 |
| 桥接 | `src/app/accounts/mod.rs` | 操作 generation、输入清理、结果应用、上下文复核、取消与关闭 |
| 运行时 | `src/app/accounts/runtime.rs` | `AccountService::open`、桌面应用数据目录、平台凭据、`ReqwestTransport`、`BackgroundExecutor` |

所有 reqwest 与 SQLite/凭据调用都在既有 `BackgroundExecutor` 的 Tokio 线程或既有 `spawn_blocking` 中执行。后台只把结果压入受锁队列，GPUI 侧按 50ms 的轻量节拍取出并更新实体：**没有 `block_on`，render 内不做 I/O**。

## 正式 Shell 接入接口

预览已抽出可复用部分，正式任务可直接复用而不复制服务路径：

1. `ui::accounts::{AccountsView, AccountCallbacks, render}` — 输入、账户列表、替换/重验证/选择与仓库结果区，只接收展示数据。
2. `app::accounts` 的桥接模式 — 单调 generation + `RepositoryReadContext` 复核 + 取消/关闭边界；正式页面接入时应把账户列表与仓库展示替换成正式侧栏/页面组件。
3. `app::accounts::runtime::AccountRuntime` — 单一拥有者：服务、执行器、任务收尾。

正式接入仍需决定（不在本任务内）：切账户时标签如何处理、账户移除 UI、私有内容保留政策、正式仓库列表组件与头像下载（本预览用名字 fallback，没有用 fixture 冒充真实头像）。

## 验证

已执行（本机 Windows）：

```sh
cargo fmt --all -- --check
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --locked
git diff --check
```

`src/app/accounts/tests.rs` 用**可注入服务/传输/临时 SQLite/内存凭据**覆盖：合成 PAT 登录→真实仓库结果进入视图、掩码与提交后清空、Tab 到提交、重复提交不推进、替换/切换/重验证/重启恢复、401 与限流不冒充空列表、Partial 保留已取数据、取消后旧 generation 不覆盖新结果、请求期间切账户丢弃旧仓库结果、取消已提交变更后对账权威账户、取消 revalidate 不发布其迟到成功、视图关闭后不再发布。

**未执行**：真实 PAT 登录与真实 GitHub 私有仓库联调。该项须由负责人在预览窗口自行输入 token；本任务没有读取 `gh`、环境变量、日志或任何真实凭据，也没有对 `api.github.com` 发起带真实 token 的请求。真实窗口中文本输入法、粘贴与焦点行为同样需要负责人在预览里确认。

真实窗口启动已核实：`--account-preview` 在本机启动并保持窗口运行，默认无参数启动仍进入原 Shell（两者均随后正常关闭）。
