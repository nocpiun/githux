# Notifications 只读收件箱

本任务交付独立的数据适配、账户控制器、列表和原生 example，正式 Shell 的 Notifications 路由由后续唯一集成者挂载。尚未接入 Shell，不把预览视为正式应用功能已上线。实现不依赖全局 Issues/PR 或 cache-core。

## 已确认范围

默认 `GET /notifications?all=false&participating=false&per_page=50&page=1`，不加 `since/before`，包含 watched 与 participating 来源的未读通知。第一次进入只加载一次，之后仅显式刷新和加载更多；不后台轮询，不读取/已读/Done/全部已读/Watch/Ignore/订阅，不实现磁盘业务缓存。

行展示真实字符串 thread ID 对应的标题、仓库、reason、更新时间、unread。thread ID 不作为 Issue/PR ID。列表保持服务端顺序，跨页按 ID 去重；标题和未知类型不猜详情地址，不跳浏览器。仓库名独立输出 typed `RepositoryNavigation`（真实 repository ID、owner/name、`RepositoryReadContext`），供宿主核验当前账户及 tab/incarnation 后打开应用内 Repository。

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
- `src/github/notifications/mod.rs` 每次只读一页，复用固定请求头、标准 `reqwest::Url` origin 判断、共享错误分类及接收时 body 限制。continuation 为不可构造的 cursor；仅接受同源、相同 notifications path、固定 all/participating/per_page 和恰好下一页的 Link。跨域、不同端口、userinfo、循环、额外查询参数均停止并标 Partial。
- 默认每页 50、最多 20 页/1,000 个已加载 thread、单响应 2 MiB。达到限制标 Partial，不声称全局未读总数。UI 的 `N loaded` 仅表示当前加载的去重行数。
- `RepoService::notifications()` 捕获一个 `RepositorySession`，`Notifications` 通过其 `read` 复用凭据 blocking load、共享 semaphore、取消和 auth epoch/selection/service-instance 检查。没有第二套 PAT 管理或 endpoint 私有 semaphore。
- `begin` 在请求开始即设置 busy/generation。首次进入幂等；手动刷新可使在途追加失效；重复相同请求不派发。`finish` 在成功、失败、headers、validator 和 busy 发布前核验账户与请求 generation。账户变化必须由宿主创建新 controller，旧 controller 的 snapshot 不返回旧行。
- 追加失败保留既有行与 continuation；首页失败不变成 empty。只有成功、完整、无错误的空列表才显示 No unread notifications。刷新失败保留旧快照并显示错误；SSO partial 即使零行也不显示有效空。
- `X-Poll-Interval` 约束手动刷新，缺失时保守 60 秒；响应 headers 在 status/parse 失败时仍进入冷却。限流遵从共享 typed 分类、Retry-After 和 primary reset，未知限流至少 60 秒；冷却不自动重发。分页为显式 continuation，不受 page-one polling interval 阻止，但仍受 rate cooldown 约束。
- 仅对同 controller、同参数、已保存且无 partial/error 的首页表示原样发送 `If-Modified-Since`。304 保留后续加载行和 continuation；没有验证器的 304 是错误。partial/失败清除 validator，不能因条件请求跳过缺页补取。

## 组件与挂载

`src/ui/notifications/mod.rs` 的 `NotificationsView::new(controller, executor, on_repository, window, cx)` 在构造时开始 first load，通过共享后台 executor 和 oneshot 发布完成。render 不请求；实际列表用 GPUI ListState 虚拟化，公共 `scroll::Motion` / `Bars` / `Bar` 驱动 wheel 与 thumb。每个页面自己的列表焦点、选择、scroll/fade 状态不共享。相对时间复用 `src/ui/relative_time.rs`（由 Home 的 `time.rs` 抽取而来，仅移动/改名，Home 的引用与可见性同步调整，时间行为与回归测试原样保留）。

Arrow/Home/End/PageUp/PageDown 移动选择并 reveal；列表焦点下 Enter 明确打开当前 repository。仓库 link 可单独 Tab/Enter 激活，父列表不重复处理 link 键盘事件。通知标题无动作。刷新/加载更多有 in-flight 限制，加载以紧凑 spinner 表示；错误图标 tooltip 包含恢复建议，点击重试对应请求。窗口失活取消交互动画；页面隐藏（keyed state release）或宿主调用 `leave` 时取消在途读取，返回保留实体不会重新首载。宿主仍须在 callback 执行时核验自己的 tab/incarnation。

## 官方接口证据

2026-11-01 重新只读抓取 [GitHub Notifications REST 官方文档](https://docs.github.com/en/rest/activity/notifications?apiVersion=2022-11-28) 的 [机器可读正文](https://docs.github.com/api/article/body?pathname=/en/rest/activity/notifications&apiVersion=2022-11-28)。文档确认 Classic PAT、`notifications` 或 `repo` scope、fine-grained token 不支持该端点；默认 all=false/participating=false、按更新时间排序、per_page 最大 50、Last-Modified 与 X-Poll-Interval。响应为 Thread 数组，id 为字符串，repository 为 Minimal Repository，subject 含 title/type/url/latest_comment_url，last_read_at 可 null。此端点需要认证且可能包含私人通知，因此本任务只核验官方公开 schema，再用确定性 fixture；没有读取任何实际私人通知，也没有远端业务写入。

## 预览与验证边界

运行 `node scripts/verify.mjs -- cargo run --locked --example notifications`。example 复用真实 AccountService → RepositorySession → Notifications adapter/controller → view；仅临时 SQLite 账户元数据和既有 InMemoryCredentialStore，合成 PAT，不打开正式账户目录/平台凭据。`src/bin_support/notifications_fixture.rs` 按请求资源重复应答，测试和预览共用，支持 mixed、多页重复 ID、empty、首页/后页一次失败后恢复、SSO partial、未知类型、延迟和 1,000 行。合成 fixture 的 `page_one_rows` 用注入固定时钟生成分钟/小时/天/未来/无效的时间分布，且混合 Issue/PR/Release/Commit/Discussion/unknown 类型与已读/未读，不全是同一硬编码绝对时间。未建模请求为开发错误 599，不伪装 404。

自动化模拟、原生窗口与正式 Shell 集成分别记录在 [本任务开发日志](devlog/2026-11-01-notifications.md)。UI PR 保留 pending，等待负责人原生验收；编译成功与 GPUI 测试不能代替 Windows 输入、滚动手感、多 DPI 和真实 PAT 的验收。
