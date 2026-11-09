# Notifications 只读收件箱

本任务交付独立的数据适配、账户控制器、列表、三项条目动作和原生 example。正式 Shell 的 Notifications 路由与动作接线已由 shell-global-pages 集成任务挂载（`app::pages::NotificationsPage`）；开发 example 仍是独立预览 target，不等同于正式应用功能已上线。实现不依赖全局 Issues/PR 或 cache-core。

条目动作是 GitHub 收件箱语义：**Done（完成）**、**Unsubscribe（取消订阅）**、**Save for later（稍后处理）**。Done 不是 mark read；三者都不代表订阅/取消订阅之外的写能力扩张。

## Native Shell 原位替换（2026-11-09）

按负责人 2026-11-09 指示（`docs/native-shell-inplace.md`），本页面原位迁移为纯 Rust Native Shell 架构：
- 页面控制器与视图位于 `src/ui/notifications/mod.rs`，宿主位于 `src/app/pages/notifications.rs`，遵循 `PageView` 生命周期契约（`element()`, `poll()`, `refresh()`, `set_active()`, `retire()`）。
- 视图内部通过 UI 线程事件队列与状态驱动，用户点击产生 `Intent`（First / Refresh / More / Action），`poll` 调用在 UI 线程分发异步工作至既有 BackgroundExecutor，数据回传通过 `PageSlot<InboxSnapshot>`，受 `PageTicket` 和运行时 `Sessions` 的 account、incarnation、tab、resource 及 cache authorization 校验门禁保护。
- 无论新请求还是已展示数据，遇到账户切换/换 PAT/移除或撤权时，单次 drain 即时清理旧行，绝不将旧账户数据泄漏给新账户。
- 渲染过程零 I/O、无锁竞争；时钟更新使用 NSH 轻量定时任务驱动，页面失活（`set_active(false)`）或 retire 时停止计时与在途读取，不占用后台资源。
- 视觉采用 NSH 默认主题与控件样式，排版结构、字体（Githux UI / Google Sans Code）、相对时间与交互语义继续保留。

## 已确认范围

默认 `GET /notifications?all=false&participating=false&per_page=50&page=1`，不加 `since/before`，包含 watched 与 participating 来源的未读通知。第一次进入只加载一次，之后仅显式刷新和加载更多；不后台轮询，不实现磁盘业务缓存。首版只提供单条 Done、单条 Unsubscribe 与禁用的 Save for later：不做 mark read、全部已读、Done-all、按仓库通知页或其它 Watch/Ignore/订阅管理动作。

行展示真实字符串 thread ID 对应的标题、仓库、reason、更新时间、unread，以及右下角常显的 Done、Unsubscribe、Save for later 三个紧凑图标按钮（Check、EyeOff、Bookmark）。时间在上、按钮在下，不依赖 hover 才可发现；窄屏时右列固定宽度、标题真实收缩，按钮不挤出条目或覆盖文本。thread ID 不作为 Issue/PR ID。列表保持服务端顺序，跨页按 ID 去重；标题和未知类型不猜详情地址，不跳浏览器。仓库名独立输出 typed `RepositoryNavigation`（真实 repository ID、owner/name、`RepositoryReadContext`），供宿主核验当前账户及 tab/incarnation 后打开应用内 Repository。

## 行排版与读状态

每行是紧凑两行布局，参照 GitHub 网页通知列表：

- 左侧固定窄槽依次放 unread 圆点（6px）与 subject 类型图标；槽位固定且竖直居中。
- 内容块两行：第一行是较小的弱化 owner/repo 元数据，右侧是相对时间（有主次、不挤压正文）；第二行是主要标题，标题与仓库文本同左起点。
- 读状态只用背景 + unread 圆点 + 标题字重区分：未读用 `UNREAD` 浅蓝底、accent 圆点、`FontWeight::MEDIUM`，已读用透明底、无圆点、`FontWeight::NORMAL`；hover/selected 采用 NSH 主题 accent。不渲染 Read/Unread 字样或 badge。无障碍名称仍表达读状态。
- reason（Watching/Mentioned/Review requested…）不另占行，放在 subject 图标 tooltip。
- 标题用 NSH 原生单行省略（`number_of_lines(1)` + `flex_1` + `min_w_0`）。
- 时间复用 Home 已验收的 `relative`（抽取到 `src/ui/relative_time.rs`），显示 Just now / N minutes ago / N hours ago / Yesterday / N days ago，绝对时间只放 tooltip；无效为 Time unavailable、未来为 In the future，不伪装成功 ago。可见时低频本地时钟刷新，隐藏/释放/失活不留下计时任务，且不触发任何 GitHub 请求。
- 顶部只显示已加载行数 `N`，tooltip 说明它只是当前视图已加载数量，不是远端未读总数。

## 数据和控制器

- `src/domain/notification.rs` 定义 thread、subject、partial 和错误。未知 subject 类型保留为 `Unknown`；未知 reason 安全显示 Other notification。subject/comment/repository URL 不发起读取、不变成导航地址。

### 条目动作（账户作用域）

- `mark_done`：`DELETE /notifications/threads/{id}`，只有 **204 No Content** 才确认完成；200/403/404/超时都不当作成功，绝不使用 `PATCH` read 冒充完成。线程 ID 只允许 `[A-Za-z0-9_-]`，不能借分隔符跳出固定端点。
- `unsubscribe_thread`：第一步 `PUT /notifications/threads/{id}/subscription`，JSON `{"ignored":true}` 与 `Content-Type: application/json`，必须 200 且响应 `ignored=true` 才算忽略成功；随后调用 `mark_done`，因为 GitHub 收件箱的 Unsubscribe 会从 inbox 移除通知。**只用 `DELETE subscription` 对 watched repo 无效，禁止使用**。两步各自经过一次 `RepositorySession::read` 的授权检查，账户切换/换 PAT 发生在两步之间时第二步不会用旧凭据发出。
- 若第一步成功而第二步 Done 失败：保留「已确认取消订阅」的事实，状态为 `UnsubscribedPendingDone`，只提供重试剩余 Done 的入口，不回滚、不谎称全成功、不本地删行冒充完成。
- 写请求按 thread 串行：进行中该条两个动作禁用并以 spinner 表示，其他行不受影响；重复点击不产生第二个请求。失败保留条目，显示 warning 图标 + tooltip 与真实重试入口。
- 条目动作有独立的取消令牌：刷新/加载更多只替换列表请求令牌，不中止进行中的 Done/Unsubscribe；只有离开页面或视图销毁才停止动作。动作代次在控制器生命周期内单调递增，跨取消也不会被旧回复覆盖。控制器最后一个持有者释放时仍取消读写令牌（丢弃 source 不取消，见 L005），避免销毁页面后仍有排队写请求发出。动作冷却以截止时间实时计算，服务端冷却到期后按钮自行解锁，不需要额外网络请求。
- 已确认 `ignored=true` 的 thread 单独记录：之后重试 Done 再失败仍是“已取消订阅、待完成 Done”，不会重新提供 Unsubscribe。
- 账户切换、移除、换 PAT 或旧响应到达时，动作成功/失败都按 `RepositorySession` 的 context 与动作 generation 丢弃；已确认 Done 的 thread ID 记入 tombstone，之后到达的旧列表页或刷新不会把它复活。
- 写请求结果不明（超时/断连）不自动重放，也不把 401/403/404 当成功；`X-Poll-Interval` 与限流冷却沿用读取路径的语义，写失败同样进入冷却。
- **Save for later**：当前公开 REST 列表与公开 GraphQL 均无 save/read-saved 接口。按已确认 G2「暂不支持先放着，不做 GitHub 跳转」保留按钮但禁用，tooltip `Save for later (unavailable)`；不伪造成功、不本地收藏、不读 cookie、不使用未公开接口、点击零网络请求。若未来出现正式支持，先带一手证据报告协调者再改边界。

## 页面背景、标题与内边距

页面内容使用与其它正式页面相同的 `t::theme().background`；标题复用 18px SEMIBOLD 标题体系，行高 40px，内缩由 `t::PAGE_HEADER_INSET`（8px）控制，外层页边距由 `t::PAGE_GUTTER`（16px）、`t::PAGE_TOP`（16px）、`t::PAGE_BOTTOM`（8px）统一给出。已读行底色透明，未读行使用浅蓝 `UNREAD` + 圆点 + 字重；通知行排版、右下角三个动作与相对时间不变。

保留的 tab 若在首次读取返回前离开，控制器会取消请求但不设 `loaded`；`set_active(true)` 因此在未加载、无进行中请求且无错误时重启读取，避免返回后留下永久空白收件箱。已加载的空收件箱、进行中的请求与已有错误都不重发。

## 组件与挂载

`src/ui/notifications/mod.rs` 的 `NotificationsView::new(context, on_repository)` 在构造时入队首次加载请求，前台 poll 驱动真实控制器。列表使用 NSH `virtual_list` 虚拟化渲染，支持键盘焦点导航（Up/Down/Home/End 切换选择并滚动聚焦，Enter 激活当前选中条目的仓库导航）。

正式 Shell 宿主见 [`src/app/pages/notifications.rs`](../src/app/pages/notifications.rs)：实现 `PageView`，将 typed `RepositoryIdentity` 转化为 `PageNavigation::Open(Route::Repository { .. })` 应用内导航；无账户/控制器不可用时显示明确的 unavailable 占位而非空白或 fixture。

## 预览与验证边界

运行 `node scripts/verify.mjs -- cargo run --locked --example notifications`。example 复用真实 AccountService → RepositorySession → Notifications adapter/controller → view；仅临时 SQLite 账户元数据和既有 InMemoryCredentialStore，合成 PAT，不打开正式账户目录/平台凭据。`src/bin_support/notifications_fixture.rs` 按请求资源重复应答，测试和预览共用，支持 mixed、多页重复 ID、empty、首页/后页一次失败后恢复、SSO partial、未知类型、延迟和 1,000 行。
