# 账户与仓库读取预览（account-ui）

更新：2026-10-28。本文件记录 R2 真实登录与 GPUI 桥接的**接线验收预览**，不是正式 Shell 账户 UI，也不代表真实 PAT 联调已完成。账户服务、SQLite 目录、系统凭据、GitHub 读取与后台执行器沿用已有实现，没有新增 HTTP、SQLite 或账户服务路径。

> 2026-10-28：预览已由 account-shell 任务接入正式 Shell——同一个 `app::accounts` 控制器复用生产账户服务，Settings › Accounts 提供 add/select/replace/revalidate，侧栏与 palette 使用真实账户与 `/user/repos` 数据。预览窗口仍作为开发者 example 保留（`cargo run --example account`），但不再作为接线验收的产物；其账户规则（generation、上下文复核、取消与对账、仓库列表失效）全部由正式控制器承载。

## 启动与产品隔离

```sh
cargo run --example account
```

账户预览与 `examples/reader.rs` 一样使用标准 Cargo example，是**独立开发 target**，不是产品启动参数。它使用原生系统标题栏、Ely Assets/init、相同深色主题、Noto Sans / Google Sans Code 与共享滚动条，不复制账户服务、HTTP、存储、字体或滚动条实现。

`examples/account.rs` 通过 `src/bin_support/account_modules.rs` 的薄 `#[path]` 模块装配直接复用 `src/ui/accounts/**`、`src/app/accounts/**` 与 `src/ui/scroll/**`。账户测试通过薄 `tests/account.rs` 真正执行；测试代码仍留在实现旁。

正式 `src/app/mod.rs` 没有账户预览参数识别、预览分支或专用退出逻辑；产品 binary 的模块树不引入这些预览模块，因此正常产品启动不会因预览打开真实数据库、凭据或网络。`cargo run` 与 `cargo build --release` 只走正常 Shell 产品路径。标准 example 不属于默认 release binary；CI 的正式 artifact 只从 `target/<target>/debug/githux` 打包产品 exe 与必需字体/运行时资源，不包含 `examples/account.exe` 或 `examples/reader.exe`。

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
- 进程退出不会截断上述收尾：`examples/account.rs` 在事件循环返回后调用桥接的有界等待（上限 10s），而不是让 detached 线程随进程消失。正式 `app::run` 不包含这项预览专用逻辑。

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

## 依赖基线

预览跟随主线 Ely 迁移（2026-10-27）：官方仓库 `https://github.com/ELY-Open-Source/Ely-GPUI-Components`，pin `30c66fe1d5d23db769a6c834b771d054d7f288cf`，只依赖 `ely-core`（regular/dev 均 `default-features=false`，保留 `runtime_shaders` / `test-support`），GPUI 与 platform 统一 `=0.3.10`，Rust 1.98.1。本任务代码全部使用 `ely_core::*` 与 `ely_core::init_for_tests`，不再引用旧 `ely-gpui-component` facade；没有 vendor、fork、Cargo patch 或本地缓存修改。

迁移没有改动业务层：登录、目录持久化/恢复、切换/替换/重验证、仓库读取、旧响应隔离、取消与退出收尾仍是迁移前已验证的实现。GPUI 0.3.10 的 `ScrollEvent`、`KeyBinding::load`、`Keystroke::parse` 变化只影响外壳（PR #30）；预览自身使用的输入、订阅、拦截器与 `cx.spawn` 形态在新版本下保持同等行为，并有测试覆盖。

## 滚动区域接入公共 scroll

预览原有的纵向溢出区域（`account-preview-scroll`，持有真实 `ScrollHandle`）现在接入 main 的统一滚动条（[统一滚动与滚动条](scrolling.md)，PR #29），不新增第二套几何/拖动/Fade：

- 预览自己持有 `Bars`，`hover` 绑定整个预览区域，`drag` 走 `Bars::drag(&scroll, Axis::Vertical, …)`，窗口失活与视图释放调用 `Bars::cancel`。
- 用 `Placement::overlay(4.0, 8.0)` + `Style::DEFAULT` 贴在右侧，外观仍是细 3px 圆头半透明、140ms 双向淡出。
- 只有 `ScrollHandle` 这一处滚动目标；没有为每个仓库行另建滚动区，也没有在消费者里重算 thumb 几何。
- 共享 `ui::scroll` 通过 example 的薄装配直接复用，未改 Shell 布局、侧栏或公共算法。

## 验证

已执行（本机 Windows，HEAD 含 content-reader 合入 main 的组合）：`cargo fmt --all -- --check`、`cargo check --locked --all-targets -j 1`、`cargo clippy --locked --all-targets -j 1 -- -D warnings`、`cargo test --locked -j 1`（主 binary 519 项、账户装配 310 项、reader 123 项，其中本任务 19 项）、`cargo build --locked --example account -j 1`、`cargo build --locked --release -j 1`、`git diff --check`。reader 的独立 example 与测试仍保留，默认外壳的标签条滚动/排序/toolbar 既有测试保持通过。

新增 `preview_scrollbar_drags_real_content_and_cleans_up` 用真实 100 条仓库结果验证：长内容确有溢出（末行已布局、reach 超出视口）、拖动公共 thumb 移动真实 `ScrollHandle`、外部释放结束拖动、离开区域 140ms 淡出、内容变短后 offset 收敛到新范围内且不再显示末行、无溢出窗口不绘制条、隐藏条不阻挡 PAT 输入、取消与窗口失活立即清理。

`src/app/accounts/tests.rs` 用**可注入服务/传输/临时 SQLite/内存凭据**覆盖：合成 PAT 登录→真实仓库结果进入视图、掩码与提交后清空、粘贴与中文文本仍保持掩码后提交、输入后按钮可用、Tab 到提交、重复提交不推进、替换/切换/重验证/重启恢复、401 与限流不冒充空列表、Partial 保留已取数据、取消后旧 generation 不覆盖新结果、请求期间切账户丢弃旧仓库结果、取消已提交变更后对账权威账户、取消 revalidate 不发布其迟到成功、多个取消独立对账并各自作废仓库结果、错误操作保留自身错误、视图释放不泄漏、退出前等待收尾。

**未执行**：真实 PAT 登录与真实 GitHub 私有仓库联调。该项须由负责人在预览窗口自行输入 token；本任务没有读取 `gh`、环境变量、日志或任何真实凭据，也没有对 `api.github.com` 发起带真实 token 的请求。真实窗口中文本输入法、粘贴与焦点行为同样需要负责人在预览里确认。

真实窗口启动已核实（最新 main 组合）：`target/debug/examples/account.exe` 独立启动，窗口标题 `Githux · Account wiring preview`、进程 Responding；`target/release/githux.exe` 正常进入原 Shell（标题 `Githux`），包含已合入 main 的 sidebar 布局。均按 exe 路径、PID 与创建时间核对后由本任务主动关闭，未触碰其他任务进程。这只能证明启动路径与关闭收尾，不构成视觉、滚动条观感或真实登录验收。
