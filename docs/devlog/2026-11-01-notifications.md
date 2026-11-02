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

## 2026-11-02 条目动作（Done / Unsubscribe / Save for later）

负责人要求每条通知右下角常显三个图标按钮，并确认语义为 GitHub 收件箱的 Done、Unsubscribe、Save for later（Done 不是 mark read）。实现：

- 右列固定宽度：时间在上，Check / EyeOff / Bookmark 三个紧凑 Ely `IconButton` 在下。Done 与 Unsubscribe 接到本组件现有账户作用域 controller；Save 无公开 API，禁用并带 `Save for later (unavailable)` tooltip，点击零请求。Ely 固定图标集没有 `BellOff`，语义最接近的是 `EyeOff`（对应 `ignored`），tooltip/无障碍名称仍为 Unsubscribe，已在交付说明中报告该替代。
- `mark_done` 用 `DELETE /notifications/threads/{id}`，仅 204 成功；`unsubscribe_thread` 先 `PUT .../subscription`（`{"ignored":true}`，200 且 `ignored=true`）再 `mark_done`，第二步失败保留 `UnsubscribedPendingDone` 并只重试 Done。只用 DELETE subscription 对 watched repo 无效，因此没有采用。
- controller 为每条 thread 维护 busy/failed 状态与动作 generation；重复点击同条返回 `Busy`，其他行不受影响。已确认 Done 的 thread ID 进入 tombstone，之后到达的旧列表/刷新不会复活；账户切换/换 PAT 后旧动作结果被 context 检查丢弃。
- 失败保留条目，只显示 warning 图标 + tooltip 与真实重试入口；超时/403/404 不吞行也不当成功，写失败同样进入 rate/poll 冷却。

公共基础：按协调者授权，从 `feature/repository-star` 固定提交 `a707021db4bef6945b27653a75988de8820f81ab`（相对 base `9c40594`）精确引入 `src/github/client.rs` 与 `src/network/transport.rs` 的 PUT/DELETE 支持并单独提交，未复制 Star 其他文件、未合并整条分支、未另造 mutation 框架。

验证：mock/localhost 覆盖 Done method/path/204、非 204 拒绝、Unsubscribe JSON + 两步、ignored=false 拒绝、第二步失败保留并重试、403/超时不吞、重复点击、跨账户旧回复、旧刷新不复活、thread ID 不能逃出固定端点；UI 测试覆盖三按钮位置/窄屏收缩、Save 点击零写请求、Done 点击后仅移除该行且不触发 Repository 导航。原生 144 DPI 截图 `.agents/task/screens/actions-wide.png` 与 `actions-narrow3.png` 确认时间在上、按钮在下、长标题真实省略与读状态背景；负责人已确认无需再截图。

**本轮 Codex 反馈（3 条 P2，均已修复并反向验证）**：

1. 列表请求与条目动作曾共享取消令牌，Refresh/Load more 会中止进行中的写；`cancel()` 后动作还会复用已取消令牌。改为独立 `action_cancel` 生命周期，只在显式 cancel/leave/teardown 时轮换；回归 `list_requests_never_cancel_an_in_flight_thread_action`、`actions_work_after_the_view_was_cancelled_and_reentered`。
2. 动作冷却只读 `snapshot.action_cooldown_seconds`，截止后不会自行解锁。改为视图保存 `action_cooldown_until` 截止时间并实时比较，冷却唤醒按 poll/action 较早截止重绘；回归 `action_buttons_unlock_when_the_server_cooldown_expires`。
3. 已确认 ignored 后再重试 Done 失败会被覆盖成普通 `Failed(Done)`，UI 会重新提供 Unsubscribe。改为独立 `unsubscribed` 集合保留已确认事实，重试失败仍是 `UnsubscribedPendingDone` 且不再提供 Unsubscribe；回归 `confirmed_unsubscribe_survives_a_failed_done_retry`。

4. 并发完成时较短限流截止会覆盖较长截止（例如 90s 后被 5s 缩短）。改为 `extend_request_after` 取现有与新截止的最大值，列表与条目动作共用；回归 `a_shorter_rate_hint_never_shortens_an_outstanding_cooldown`。

5. 离开保留视图时 `cancel()` 清空 `thread_states`，会丢掉已确认的 `UnsubscribedPendingDone`，返回后 Unsubscribe 按钮重新可用但控制器会静默拒绝。改为取消时只保留 pending-Done 状态、清除 Busy/Failed；回归 `pending_done_survives_leaving_and_returning_to_a_retained_view`。

6. tombstone 只按 thread ID 永久隐藏，会阻断同一 thread 之后合法的新通知。改为记录 Done 时观察到的 `updated_at`，只抑制该版本；同一 ID 的新版本重新进入 inbox。回归 `newer_activity_reenters_the_inbox_after_a_done`。

7. Done 完成时按 ID 移除行，会误删并发刷新已载入的同一 ID 新版本 B。改为只移除 Done 捕获的版本 A，较新版本保留；回归 `a_late_done_does_not_remove_a_newer_version_already_loaded`。

以上七条均按 L008 反向验证：临时移除修复后对应测试失败，恢复后通过。

**与最新 main 组合**：合并 `origin/main` 时 `src/data/mod.rs` 只有新增模块声明的文本冲突，保留双方声明即可。发现 Newer main 的 `global_work_items` 通过 `#[path = "../home/time.rs"]` 复用 Home 时间实现，而我已把实现移到 `src/ui/relative_time.rs`；为不破坏该消费者，把 `src/ui/home/time.rs` 恢复为仅转发的 shim，并在其独立装配 `work_items_modules.rs` 注册共享模块，仍只有一份解析器。第一次完整 `cargo test` 出现两个与本任务无关的 main 侧偶发失败（`repository_descriptions_deduplicates...`、`reader_images::fixed_revision_images...` 的 `InUse`），单独重跑均通过，随后完整重跑全部通过；记录为既有测试的顺序/资源偶发，不归因本任务，也不掩盖。

未测边界：真实账户写操作、真实 tooltip 悬停/焦点到达、150% 缩放切换与触摸板仍未原生验证，交负责人验收。

## 验收与集成边界

本任务须创建带 milestone #1 和 pending 的 PR，CI/Codex review 绑定最新 head。UI 需要负责人验收，基层执行者不合并。原生窗口的 mixed/long/narrow/keyboard/paging 与 DPI/手感如受环境阻塞必须明确报告；编译与 GPUI 测试不能替代正式 Shell 或真实 Windows 输入。本轮应用 L002/L007/L010/L012/L017，并把两步写操作的中间事实规则沉淀为 L018。
