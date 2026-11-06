# Notifications 只读收件箱

本任务交付独立的数据适配、账户控制器、列表、三项条目动作和原生 example。正式 Shell 的 Notifications 路由与动作接线已由 shell-global-pages 集成任务挂载（`app::pages::NotificationsPage`）；开发 example 仍是独立预览 target，不等同于正式应用功能已上线。实现不依赖全局 Issues/PR 或 cache-core。

条目动作是 GitHub 收件箱语义：**Done（完成）**、**Unsubscribe（取消订阅）**、**Save for later（稍后处理）**。Done 不是 mark read；三者都不代表订阅/取消订阅之外的写能力扩张。

## 已确认范围

默认 `GET /notifications?all=false&participating=false&per_page=50&page=1`，不加 `since/before`，包含 watched 与 participating 来源的未读通知。第一次进入只加载一次，之后仅显式刷新和加载更多；不后台轮询，不实现磁盘业务缓存。首版只提供单条 Done、单条 Unsubscribe 与禁用的 Save for later：不做 mark read、全部已读、Done-all、按仓库通知页或其它 Watch/Ignore/订阅管理动作。

行展示真实字符串 thread ID 对应的标题、仓库、reason、更新时间、unread，以及右下角常显的 Done、Unsubscribe、Save for later 三个紧凑图标按钮（Check、EyeOff、Bookmark）。时间在上、按钮在下，不依赖 hover 才可发现；窄屏时右列固定宽度、标题真实收缩，按钮不挤出条目或覆盖文本。thread ID 不作为 Issue/PR ID。列表保持服务端顺序，跨页按 ID 去重；标题和未知类型不猜详情地址，不跳浏览器。仓库名独立输出 typed `RepositoryNavigation`（真实 repository ID、owner/name、`RepositoryReadContext`），供宿主核验当前账户及 tab/incarnation 后打开应用内 Repository。

## 行排版与读状态（2026-11-01 负责人反馈后）

每行是紧凑两行布局，参照 GitHub 网页通知列表：

- 左侧固定窄槽依次放 unread 圆点（6px）与 subject 类型图标；图标用 `control_height(Sm)` 固定槽位且竖直居中，不用 `pt_1` 这类手工补偿。
- 内容块两行：第一行是较小的弱化 owner/repo 元数据，右侧是相对时间（有主次、不挤压正文）；第二行是主要标题，标题与仓库文本同左起点。
- 读状态只用背景 + unread 圆点 + 标题字重区分：未读用 `info_subtle` 背景、accent 圆点、`FontWeight::MEDIUM`，已读用 `bg`、无圆点、`FontWeight::NORMAL`；hover/selected 用 `hover`/`active`。不渲染 Read/Unread 字样或 badge。无障碍名称仍表达读状态。
- reason（Watching/Mentioned/Review requested…）不另占行，放在 subject 图标 tooltip。
- 标题用 GPUI 原生 `.truncate()`（`min_w_0` + `flex_1`），不再使用 Ely `Ellipsis`：后者的隐藏全量测量 div 在虚拟列表 item 的 `MaxContent` 测量下会把行撑到整段文本宽度（#57 描述的现象），与本任务行内布局叠加后会扩大列表宽度。
- 时间复用 Home 已验收的 `relative`（抽取到 `src/ui/relative_time.rs`），显示 Just now / N minutes ago / N hours ago / Yesterday / N days ago，绝对时间只放 tooltip；无效为 Time unavailable、未来为 In the future，不伪装成功 ago。可见时一个 30 秒低频本地时钟刷新，隐藏/释放/失活不留下计时任务，且不触发任何 GitHub 请求。
- 顶部只显示已加载行数 `N`（无 `loaded` 字样），tooltip/无障碍说明它只是当前视图已加载数量，不是远端未读总数。

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

这些写操作建立在 `src/github/client.rs` 与 `src/network/transport.rs` 的通用 PUT/DELETE 基础上；该基础从 `feature/repository-star` 的固定提交 `a707021db4bef6945b27653a75988de8820f81ab`（相对 base `9c405942654237b95cceb10c0f8543ee1a6f06a5`）按协调者授权精确引入，保持与 Star 任务一致的形态以便后续归并；没有复制 Star 的其他文件，也没有另造 mutation 框架。

- `src/github/notifications/mod.rs` 每次只读一页，复用固定请求头、标准 `reqwest::Url` origin 判断、共享错误分类及接收时 body 限制。continuation 为不可构造的 cursor；仅接受同源、相同 notifications path、固定 all/participating/per_page 和恰好下一页的 Link。跨域、不同端口、userinfo、循环、额外查询参数均停止并标 Partial。
- 默认每页 50、最多 20 页/1,000 个已加载 thread、单响应 2 MiB。达到限制标 Partial，不声称全局未读总数。UI 的 `N loaded` 仅表示当前加载的去重行数。
- `RepoService::notifications()` 捕获一个 `RepositorySession`，`Notifications` 通过其 `read` 复用凭据 blocking load、共享 semaphore、取消和 auth epoch/selection/service-instance 检查。没有第二套 PAT 管理或 endpoint 私有 semaphore。
- `begin` 在请求开始即设置 busy/generation。首次进入幂等；手动刷新可使在途追加失效；重复相同请求不派发。`finish` 在成功、失败、headers、validator 和 busy 发布前核验账户与请求 generation。账户变化必须由宿主创建新 controller，旧 controller 的 snapshot 不返回旧行。
- 追加失败保留既有行与 continuation；首页失败不变成 empty。只有成功、完整、无错误的空列表才显示 No unread notifications。刷新失败保留旧快照并显示错误；SSO partial 即使零行也不显示有效空。
- `X-Poll-Interval` 约束手动刷新，缺失时保守 60 秒；响应 headers 在 status/parse 失败时仍进入冷却。限流遵从共享 typed 分类、Retry-After 和 primary reset，未知限流至少 60 秒；冷却不自动重发。分页为显式 continuation，不受 page-one polling interval 阻止，但仍受 rate cooldown 约束。
- 仅对同 controller、同参数、已保存且无 partial/error 的首页表示原样发送 `If-Modified-Since`。304 保留后续加载行和 continuation；没有验证器的 304 是错误。partial/失败清除 validator，不能因条件请求跳过缺页补取。

## 页面背景、标题与内边距（2026-11-04 返工，2026-11-05 二次修正）

页面内容 surface 使用与 Home/Settings/全局 Issues-PR 相同的 `colors.surface`；此前用 `colors.bg` 会与顶部 tab bar 形成两条不同底色。标题复用其它正式页面的标题体系（`TextSize::Lg` + `SEMIBOLD`，行高 40），正文 gutter/top/bottom 来自共享的 `src/ui/page_layout.rs`（与 Home 现有 `px_4 / pt_4 / pb_2` 一致），不再由 header 自带 `px_3/py_2` 叠加出双重留白。已读行底色改为 `colors.surface`，未读行仍是 `info_subtle` + 圆点 + 字重；通知行排版、右下角三个动作与相对时间不变。

**2026-11-05 二次修正（负责人截图否决后）**：只统一 gutter 不等于标题对齐。Home 的列头在 16px gutter 内又加了 `px_2`（8px）内缩，所以 Home 的标题起点是 gutter + 8，而行起点才是 gutter。Notifications 的 header 此前只有 gutter，标题因此比 Home 更靠左。现在 header 额外使用 `page_layout::PAGE_HEADER_INSET = 8`（与 Home 的 `home-header` `px_2` 一致），**只有标题行**加这层内缩：列表与空状态仍只占 gutter，左右内容边缘与 Home 相同，不能跟着标题一起右移。空收件箱与有条目两种状态共用同一 header/列表容器，几何一致；不新增占位文案。回归 `notifications_heading_and_content_edges_match_home_in_empty_and_populated_states` 在 Empty 与 Mixed、宽/窄两种尺寸下分别断言列表左右边缘等于 gutter、标题文本起点等于 gutter + inset（`debug_bounds` 返回的是边框盒，内缩体现在标题内容盒上，不能拿 header 盒去比 inset）。

保留的 tab 若在首次读取返回前离开，控制器会取消请求但不设 `loaded`；`set_active(true)` 因此在未加载、无进行中请求且无错误时重启读取，避免返回后留下永久空白收件箱。已加载的空收件箱、进行中的请求与已有错误都不重发。

## 组件与挂载

`src/ui/notifications/mod.rs` 的 `NotificationsView::new(controller, executor, on_repository, window, cx)` 在构造时开始 first load，通过共享后台 executor 和 oneshot 发布完成。render 不请求；实际列表用 GPUI ListState 虚拟化，公共 `scroll::Motion` / `Bars` / `Bar` 驱动 wheel 与 thumb。每个页面自己的列表焦点、选择、scroll/fade 状态不共享。相对时间复用 `src/ui/relative_time.rs`（由 Home 的 `time.rs` 抽取而来，Home、Notifications 与其它已存在消费者共用同一实现；`src/ui/home/time.rs` 保留为仅转发的兼容 shim，避免其它 `#[path]` 消费者出现第二份解析器；Home 的行为与回归测试原样保留）。

Arrow/Home/End/PageUp/PageDown 移动选择并 reveal；列表焦点下 Enter 明确打开当前 repository。仓库 link 可单独 Tab/Enter 激活，父列表不重复处理 link 键盘事件。通知标题无动作。刷新/加载更多有 in-flight 限制，加载以紧凑 spinner 表示；错误图标 tooltip 包含恢复建议，点击重试对应请求。窗口失活取消交互动画；页面隐藏（keyed state release）或宿主调用 `leave` 时取消在途读取，返回保留实体不会重新首载。宿主仍须在 callback 执行时核验自己的 tab/incarnation。

正式 Shell 宿主见 [`src/app/pages/notifications.rs`](../src/app/pages/notifications.rs)：`AccountServices.repositories().notifications()` 建控制器，`on_repository` 经 `RepoService::is_current` 复核后转 `Route::Repository` 应用内导航；无账户/控制器不可用时显示明确的 unavailable 占位而非空白或 fixture。

## 官方接口证据

2026-11-01 重新只读抓取 [GitHub Notifications REST 官方文档](https://docs.github.com/en/rest/activity/notifications?apiVersion=2022-11-28) 的 [机器可读正文](https://docs.github.com/api/article/body?pathname=/en/rest/activity/notifications&apiVersion=2022-11-28)。文档确认 Classic PAT、`notifications` 或 `repo` scope、fine-grained token 不支持该端点；默认 all=false/participating=false、按更新时间排序、per_page 最大 50、Last-Modified 与 X-Poll-Interval。响应为 Thread 数组，id 为字符串，repository 为 Minimal Repository，subject 含 title/type/url/latest_comment_url，last_read_at 可 null。2026-11-02 再次只读核验 [Mark a thread as done](https://docs.github.com/en/rest/activity/notifications#mark-a-thread-as-done)、[Set a thread subscription](https://docs.github.com/en/rest/activity/notifications#set-a-thread-subscription)、[Delete a thread subscription](https://docs.github.com/en/rest/activity/notifications#delete-a-thread-subscription) 与 [Managing notifications from your inbox](https://docs.github.com/en/subscriptions-and-notifications/how-tos/viewing-and-triaging-notifications/managing-notifications-from-your-inbox)：Done 是 `DELETE`（204），与 read/PATCH 不同；订阅更新是 `PUT .../subscription`，`ignored=true` 才是忽略；收件箱的 Unsubscribe 会一并移除通知，因此实现为「先 ignored、后 Done」两步；对 watched repo 只删订阅不会生效。Save for later 在这两份公开 API 中都没有对应条目接口。

此端点需要认证且可能包含私人通知，因此本任务只核验官方公开 schema，再用确定性 fixture；没有读取任何实际私人通知，也没有对真实账户执行写操作。

## 预览与验证边界

运行 `node scripts/verify.mjs -- cargo run --locked --example notifications`。example 复用真实 AccountService → RepositorySession → Notifications adapter/controller → view；仅临时 SQLite 账户元数据和既有 InMemoryCredentialStore，合成 PAT，不打开正式账户目录/平台凭据。`src/bin_support/notifications_fixture.rs` 按请求资源重复应答，测试和预览共用，支持 mixed、多页重复 ID、empty、首页/后页一次失败后恢复、SSO partial、未知类型、延迟和 1,000 行。合成 fixture 的 `page_one_rows` 用注入固定时钟生成分钟/小时/天/未来/无效的时间分布，且混合 Issue/PR/Release/Commit/Discussion/unknown 类型与已读/未读，不全是同一硬编码绝对时间。未建模请求为开发错误 599，不伪装 404。

自动化模拟、原生窗口与正式 Shell 集成分别记录在 [本任务开发日志](devlog/2026-11-01-notifications.md)。UI PR 保留 pending，等待负责人原生验收；编译成功与 GPUI 测试不能代替 Windows 输入、滚动手感、多 DPI 和真实 PAT 的验收。
