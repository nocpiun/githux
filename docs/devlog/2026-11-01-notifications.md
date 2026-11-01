# Notifications 独立只读收件箱

任务 notifications；基线 `997f6e947fced45ca00b728fcea4756957f36061`，分支 `feature/notifications`。本任务独立交付 domain / GitHub adapter / account controller / view / reusable preview fixture。产品合同见 [Notifications](../notifications.md)，正式 Shell 挂载由协调者另行负责。

## 已实现行为

默认 unread、watched + participating，first entry 一次、显式 refresh/load more。请求和渲染分离；保留真实 thread ID、repository ID/name、reason、updated_at/unread，按服务端顺序去重。完整空、初次失败、追加失败、SSO partial、分页/资源上限分别表达。标题没有详情/浏览器动作；仓库 link 输出带账户上下文的精确 typed identity。

复用既有 RepositorySession::read 的凭据 blocking load、共享 semaphore、取消和账户/epoch/selection/service-instance guard。控制器额外给请求 generation，旧 success/error/headers/validator/busy 均不得回写。解析失败的响应头仍参与 poll/rate 冷却；手动刷新遵守 X-Poll-Interval，明确后续分页遵守 rate backoff。受限同源同端点 Link continuation，body/page/item 上限，subject/comment/repository URL 不被请求。Last-Modified 绑定同 controller 表示，partial/failure 清除 validator；304 保留所有已加载页。

UI 是单个有界 GPUI ListState，公共 scroll Motion/Bars/Bar，Noto Sans，窄屏标题省略与 tooltip，紧凑 loading/error。键盘选择与 repository Enter 激活，独立 link Tab/Enter；不新增未支持详情路由。`examples/notifications.rs` 用真正的 AccountService/adapter/controller/view，通过同一 deterministic transport 运行 mixed/empty/first or append failure/retry/partial/unknown/delayed/long scenes。临时 SQLite 仅保存 fixture 账户目录，合成 PAT 在既有内存 store，不接触正式账户库或真实凭据。

## 外部契约与证据

2026-11-01 只读抓取 GitHub [官方 Notifications 文档](https://docs.github.com/en/rest/activity/notifications?apiVersion=2022-11-28) 的公开正文 API，核对 Classic PAT、notifications/repo scope、fine-grained 不支持、per_page 50、all/participating defaults、Last-Modified/X-Poll-Interval 与 Thread 数组 schema。未读取真实通知或私人内容，未执行远端业务写入。确定性 payload 按官方 schema 中所消费的字段构造，subject URL 刻意放外域以验证不会自动读取。

## 验证记录（持续更新）

所有 Cargo 命令均通过 `node scripts/verify.mjs -- cargo ...`，结果保存在本任务被忽略的 runs 记录。初始 fmt 等待其他任务的共享 Windows mutex；第一轮前台工具 60 秒超时后已核对该等待进程未继续，重新以可记录的后台 runner 顺序等待，未同时运行两份 Cargo。

自动化最终结果（run_id，均经共享 runner）：

- `cargo fmt --all -- --check`：`4b76a864` 通过；后续布局/文档编辑后再次执行最终检查。
- `cargo clippy --locked --all-targets -- -D warnings`：`4d571b38` 通过。
- `cargo test --locked`：`b73a34c1` 通过，含新增 `tests/notifications.rs` 目标（355 项中 2 项 Notifications 组件测试）。
- `cargo check --locked --all-targets`：`9a289e95` 通过。
- `cargo build --locked --example notifications`：`03f7828d` 通过。

覆盖入口参数/固定头、可信分页与敌意 URL、资源上限与去重、未知类型、有效空与错误、可重复 fixture/一次失败重试、手动冷却、in-flight 去重、refresh/append success/error generation、账户替换/ABA/移除重加/关闭/不同服务实例、SSO partial/validator 与 304 保留追加行。UI 测试覆盖 960×740 与 360×440 下 viewport/row/title 不越界、方向键/Home/End、Enter 与仓库 link 点击的精确 repository identity、空/错误/partial 区分，并断言交互不产生 subject 请求。首次原生预览发现长标题缺省略的宽布局风险；补齐 flex 边界后，测试断言标题框在两种宽度内均止于 row 右侧（844/244 logical px），但 `Ellipsis` 的省略号仍未在此原生绘制路径出现，视觉细节待负责人判断。

原生预览：`cargo run --locked --example notifications` 实际打开标题为 “Githux · Notifications preview” 的窗口，`PrintWindow` 取得混合列表真实渲染帧（合成数据，中英文混排、图标、repository/reason/time）。合成窗口可启动、可渲染、进程响应；用 `SendMessage`/鼠标注入切换场景没有反映在随后捕获的画面中，因此**场景切换、加载更多、滚动、键盘、多 DPI 与真实输入尚未原生验收**。自动化 GPUI 测试不等于 Windows 原生输入证据（L002）。

## 2026-11-01 负责人反馈后返工

负责人否决不原样文案重排的初版，要求参照 GitHub 网页通知列表重做。四条反馈分别处理：

1. **图标与文字上下居中**：subject 图标固定 `control_height(Sm)` 槽位并 `justify_center`，移除旧的 `pt_1` 补偿。几何测试 `row_geometry_read_state_and_relative_time` 断言 `notification-icon-0` 中心与 `notification-row-0` 中心 y 偏差小于 1 logical px。
2. **读状态用背景而不是文字**：未读 `info_subtle` 背景 + accent 6px 圆点 + `FontWeight::MEDIUM`，已读 `bg` + 无点 + `FontWeight::NORMAL`；移除 Read/Unread 字样与 badge，无障碍名称保留读状态。hover/selected 分别用 `hover`/`active`。
3. **相对时间**：把 Home 已验收的 `src/ui/home/time.rs` 纯实现抽取为 `src/ui/relative_time.rs`（`pub(crate) fn relative`），Home 的 `mod time` 删除、`rows.rs` 引用改为 `crate::ui::relative_time::relative`，`ui/mod.rs`、`home_modules.rs`、`notifications_modules.rs` 注册同步调整。时间行为与 Home 回归测试原样保留。时间放第一行右侧，绝对时间仅 tooltip；可见时单 30 秒本地时钟刷新，隐藏/释放/失活停止，不请求 GitHub。
4. **参照 GitHub 网页通知排版**：紧凑两行（顶部弱化 owner/repo 元数据 + 相对时间，下面主标题，标题与仓库同左起点），左侧窄槽放 unread 圆点 + subject 图标。reason 移入图标 tooltip。顶部只显示已加载行数 `N`，不再显示 `N loaded` 字样。

**宽度缺陷**：初版行内用 Ely `Ellipsis`，其隐藏全量测量 div 在 GPUI 虚拟列表 item 的 `MaxContent` 测量下把行撑到整段标题宽度（#57 已记录的现象），导致长标题行宽超出视口。改为 GPUI 原生 `.truncate()` + `min_w_0`/`flex_1` 后，测试 `bounded_narrow_keyboard_and_exact_repository_callback` 断言 960×740 与 360×440 下 title/time/row 均不越出 viewport。原生 PrintWindow 截图（`.agents/task/screens/notifications-rework10.png` 等）确认两种读状态背景、无 Read/Unread 行、类型图标与相对时间呈现。

**审查反馈（Codex P2）**：窗口在 polling cooldown 期间失活时，`cancel_activity()` 会丢掉 cooldown 唤醒任务，重新激活后无人重建，空 inbox 又不会启动时钟任务，Refresh 可能一直禁用。修复为幂等的 `sync_cooldown_wake`：每次活跃 render 按剩余时间补建一次性唤醒，`sync` 也复用同一入口。回归 `cooldown_wake_is_rearmed_after_deactivation` 覆盖“有唤醒 → 失活丢弃 → 复活补建 → 固定时钟不建时间任务”；按 L008 反向验证，移除补建后该测试稳定失败。

原生边界不变：合成窗口可启动/渲染，但场景切换、加载更多、真实滚轮、键盘、多 DPI 与 100%/150% 缩放仍未在本机可靠注入验收，交由负责人验收；GPUI 测试与窗口截图不等于原生输入证据（L002）。

## 验收与集成边界

本任务须创建带 milestone #1 和 pending 的 PR，CI/Codex review 绑定最新 head。UI 需要负责人验收，基层执行者不合并。原生窗口的 mixed/long/narrow/keyboard/paging 与 DPI/手感如受环境阻塞必须明确报告；编译与 GPUI 测试不能替代正式 Shell 或真实 Windows 输入。没有新增跨任务稳定教训；应用 L002/L007/L010/L012，LESSONS 写入权留给当前 Home 执行者。
