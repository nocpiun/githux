# Profile Overview 展示组件

更新：2026-11-03。可复用只读 Profile Overview 位于 [`src/ui/profile/`](../src/ui/profile/)，由正式 [`app::pages::ProfilePage`](../src/app/pages/profile.rs) 接入账户数据、页面持久缓存、README 图片服务、workspace 与路由。Overview 居中、README 自然高度/背景/缺失省略、Pinned 简介三行及离线快照/图片展示已整合；生命周期和验证边界见[正式页面接入](shell-page-integration.md)。独立预览 `examples/profile.rs` 与 `tests/profile.rs` 的合成场景仍只用于开发验证。

## 已实现的展示范围

顺序固定为：

1. 公开基本资料：采用无外层卡片的横向头像/资料布局，name、login 上下排列，随后 bio 与自然换行的 location/实际 website 显示文本/followers/following/company。public repos/public gists/created_at 保留于传入模型，不在主视图堆叠。空字符串与缺失字段不显示；name 缺失时回退 login，头像只显示首字母/字素，**不请求 avatar_url，也不推断私有资料**。
2. 个人 README：消费 host 准备的 `PreparedContent`，直接复用现有 `ReaderView` 的 Markdown、选择、链接与自然高度 Flow 行为。外层是一层细边框与实际 owner/path 文件身份。正文不再自绘灰色底，随 Profile 页面背景；确认缺失/不确定/不适用时整段省略，真实失败保留紧凑 warning 图标。详见 [2026-10-30 README 可选缺省与无灰底](#2026-10-30-readme-可选缺省与无灰底)。
3. Pinned/Popular repositories：保持服务顺序，显示紧凑仓库摘要：链接层级名称、实际 Public/Private（未知省略）、描述 ellipsis/tooltip、语言点和 star/fork。语言点为中性 token，未发明 API 语言色。正常两列，组件宽度小于 700px 转单列，键盘步长随列数变化。没有 star 排序、自动导航或自动网络调用。
4. 贡献热力图：消费明确 `ContributionRange` 与 `ContributionCalendar`，展示服务总计和真实 day/count tooltip。键盘聚焦后方向键/Home/End 与 hover 都更新同一天数据说明；左右移动一周、上下移动一天，键盘操作同时滚动显示选中的周；长区间使用共享横向滚动。

不包含 Profile 其他 Tabs、Contribution Activity、gist 浏览、写操作、图片下载、磁盘/离线缓存、新主题或另一套 inline-code/Markdown 渲染器。

## 正式 Shell 接入（2026-10-29）

`app::pages::ProfilePage` 已把本组件接入正式 Shell。host 先用目标 login 读取 `ProfileBasic`，只有服务器返回后才用真实数字 id/canonical login 建立 `ProfileTarget`；随后独立读取 pins、host 生成的最近滚动一年贡献区间和同名公开仓库根 README。README 的缺失/不确定/不适用通过新增 `SectionState::Unavailable` 中性呈现，不作为认证或网络错误；2026-10-30 起该中性状态不再绘制任何区块，真实失败仍走 `SectionState::Error`。Header 头像通过新增 `set_avatar` 只接收 `app::avatar` 已解码图片；原有圆形、首字母 fallback 与尺寸不变。本人/他人由 `Route::Profile` / `Route::UserProfile` 统一寻址，canonical 更新只作用于原 TabId。接入细节与真实验证边界见 [Shell 页面接入](shell-page-integration.md)。

## 2026-10-29 第二轮反馈

- Pinned/Popular 卡片的描述与 language/star/fork 行改为按卡片内容起点对齐，不再跟随标题文字的图标缩进；名称层级、两列/单列、键盘行为不变。
- Profile section loading 使用 Ely spinner 加可读标签；section error 使用 warning 图标 + tooltip，原因文本保留。确认缺失/不确定的 README 使用中性 `Unavailable`；该状态在 2026-10-30 调整为整段省略，见下一节。
- 本人 Profile 打开时侧栏账户行不再显示导航选中线；头像、current 身份与账户点击不变。
- README 自身的固定 280px 高度和 overflow 容器已移除；该条在当时的 reader Flow API 落地前保持未完成，不用固定大高度或前端 strip 冒充。

## 2026-10-30 README 自然高度 Flow 与简介三行

Reader 的自然高度 Flow 能力已随 `reader-images`（PR #45，main `9522eee`）合入。Profile 现在构造 README 的 `ReaderView` 时设置 `MarkdownLayout::Flow`，并注册 `on_layout_changed` 让页面在文档高度变化后重新布局：

- 正文按真实块高完整展开，由 Profile 外层页面滚动；reader 不再持有有界视口、内部竖向滚动条或滚轮捕获，文末内容 inset 随文档测量保留。
- 上一轮"移除固定 280px 后正文塌缩"的缺陷因此真正修复；当时的实现只是删掉了宿主高度，没有消费方 Flow API，属于已知未完成，不是已完成状态。

Pinned/Popular 卡片的仓库简介改为自然换行、最多三行：`line_clamp(3)` 按真实排版换行并在第三行末省略，卡片高度上限取同一字号/行高的三个行盒，因此 1–3 行完整占高、卡片与 grid 行随内容增长，超过三行不再撑高卡片。名称截断、链接、Public badge、语言/Star/Fork 左对齐与边框不变。

## 2026-10-30 README 可选缺省与无灰底

- **正文不再自绘灰色矩形底。** Flow 布局下 `ReaderView` 根与 Markdown 根不再绘制 `theme.colors.surface`；宿主页面（Profile 的 section 背景）透出，正文与卡片背景一致。有界/Repository 布局的 `surface` 不变，代码围栏、inline code 与引用的语义色保留。见 [内容阅读器 Flow](content-reader.md#自然高度布局flow2026-10-29)。
- **确认缺失的 README 整段省略。** `SectionState::Unavailable` 不再渲染 border、header、说明文字或占位，`render_readme` 返回 `None`，因此也不会留下空行间距；Pinned 与贡献区块不受影响。该状态内部仍与 `SectionState::Error` 分开。
- **真实失败使用紧凑指示。** README 的 `SectionState::Error` 使用 `compact_status::failed` 的 warning 图标，原因在 tooltip 与无障碍名称里；正式 host 注册 `on_action` 时提供单次重试（`ProfileActionKind::RetryReadme`），独立无 handler 视图仍不显示 Retry。不再使用共享大错误面板。
- **404 归入可选缺省。** 同名仓库读取返回的 `GitHubError::NotFound`（GitHub 用 404 同时表示不存在与不可见）在 `app::pages::ProfilePage` 映射为 `Unavailable`，不显示 “not found or is not visible” 文案；网络、认证、限流、解析失败与离线未缓存（`NotCached`）仍为 `Error`。
- **无残留旧 README。** `bind`、`set_readme_unavailable`、非 Ready 的 `update_readme*` 都会通过 `set_image_provider(None, cx)` 和 `cancel_interactions(cx)` 取消并丢弃旧 `ReaderView` 及其在途图片请求；ready→缺失→换账户不会复用旧正文、旧 action、旧高度通知或旧图片请求。

## 2026-11-03 持久缓存与 README 图片正式接线

正式 [`app::pages::ProfilePage`](../src/app/pages/profile.rs) 已接入 [`CachedPageReadSession`](page-cache.md) 与 [`RepositoryImageService::with_cache`](reader-images.md)，在保留原有 `ProfilePage::new` 构造签名兼容的前提下支持已访问 Profile 资料与 README（含图片）的离线阅读：

1. **缓存会话注入与在线回退**：
   - `ProfilePage` 捕获 `PageReadSession` 后，通过 `services.repositories().cache(session.context())` 查询当前保存账户的 `(CacheStore, CacheScope)` 并构造 `CachedPageReadSession`。
   - 当应用尚未绑定 `CacheStore`（如 `cache-runtime` 未合入或缓存处于未对账恢复态）或当前 scope 被隔离（`AccessDenied`）时，自动保留原有在线 `PageReadSession` 读取能力，不额外打开第二套数据库。
   - 四个独立区块（`profile`、`profile_pinned`、`contribution_calendar`、`profile_readme`）优先执行 `PageReadMode::Online`；当遇到网络断开、超时、5xx 或限流错误时，自动回退尝试 `PageReadMode::Offline`。
2. **README 图片服务正式附着**：
   - 当 `profile_readme` 返回 `ProfileReadme::Prepared { repository, content, .. }` 时，`ProfilePage` 使用该已验证同名公开仓库的真实 `RepositoryIdentity`、当前有效 `RepositorySession`、`ReqwestExternalImages`（独立无凭据 HTTPS 通路，绝不向外部图片主机发送 PAT）以及 `services.repositories().cache(&context)`（`is_private = false`）构造 `RepositoryImageService::with_cache`。
   - 通过新增的 `ProfileView::update_readme_with_provider` 将 provider 交给 `ReaderView::set_image_provider`，同时保持 `MarkdownLayout::Flow` 自然高度、无灰底、无内嵌竖向滚动条和 HTML 子集行为。共享图片管线与缓存细节见 [README 图片加载与显示](reader-images.md)。
3. **历史快照、Partial 与缺失缓存的紧凑视觉区分**：
   - `ProfileProvenance` 记录 `historical`（`PageOrigin::HistoricalSnapshot`）、时间戳与 `CacheCompleteness`。
   - 离线历史快照在 Basic 标题行、README 头部、Pinned/Popular 标题行与 Contributions 标题行旁显示紧凑 Neutral `Badge`（`Cached · <relative time>`，复用 `ui::relative_time::relative`，hover tooltip 说明本地历史快照）。
   - `CacheCompleteness::Partial` 显示紧凑 Warning `Badge`（`Partial`，hover tooltip 显示具体不完整原因），与完整结果明确区分。
   - 离线无缓存快照（`PageValue::Miss(CacheMiss::NotCached)`）或缓存损坏映射为 `PresentationError::NotCached` / `CacheCorrupt`，显示黄色 warning 图标、hover 原因与点击重试（`RetryBasic` / `RetryRepositories` / `RetryContributions` / `RetryReadme`），绝不将未缓存或网络/授权失败伪装成“无 README”或空列表。
   - 已缓存的确认无 README（`ProfileReadme::Missing` / `NotEligible`）离线恢复时继续映射为 `SectionState::Unavailable` 并整段省略。
   - 贡献日历的缓存键包含区间端点，而 host 的区间是当天最近滚动一年。成功读取后把实际区间写入聚合门禁命名空间的 `profile-calendar-range` 标记；离线回退在当天区间 miss 后用记录的区间重读，命中时 host 采用该区间（`ProfileView::set_range`），标题、tooltip 与热力图因此始终描述屏上数据。标记缺失（未缓存或已被撤权清理）仍是带重试的 `NotCached`，不会显示成“无贡献”。
4. **授权、代次与缓存门禁（C1–C5）**：
   - 每次读取启动与结果提交前，同时校验页面 `generation`、账户 `selection`、section `request`、`services.repositories().is_current(&context)` 与 `session.verify()`。
   - 普通标签页切换（`ProfilePage::set_active(false)` → `ProfileView::suspend`）仅停止滚动与阅读器交互手势，保留已挂载 README 的图片 provider；而重新绑定、替换 README 或撕毁页面时调用 `clear_reader` / `cancel_reader` 显式取消并分离图片 provider。
   - 每个页面绑定只维护一条可取消的后台监测任务（`rebind`/`stop`/`Drop` 取消；不重放 profile 读取）。该任务独占 store 访问，前台 30ms tick 只消费它得出的失效结论，绝不在 UI 线程做周期性数据库检查；结论以产生它的绑定 generation 标记（`cache_invalidated: AtomicU64`），旧绑定遗留迭代发布的失效判定不会被新页面采纳；监测任务的存活也由每个绑定自己的 `Work` 句柄表示（`ensure_cache_monitor` 无条件替换上一绑定的任务），没有跨绑定的共享存活标志可供迟到任务压制。判定依据是写在 `repository=None` 聚合门禁命名空间里的 `AggregateGateStamp`：读取任务开始时建立/读取并持有所在条目的 `CacheLease`，**在线与历史结果都携带它**（含 Basic：即使三个兄弟 section 全部失败，Basic 的 stamp 仍是唯一可验代次），在屏期间由监测任务核对。lease 使普通容量淘汰无法删除 stamp（不会把仍授权的页面误判为撤权），而 stamp 被撤权删除、门禁被换 PAT/401 拒绝隔离或 scope 退休都会使已显示内容失效。因此即使撤权发生在首轮轮询前，或之后又写入了新的 stamp/快照，已展示的旧内容也会被清除，并同步清理旧 `ProfileView`、`ReaderView` 与在途图片，阻断迟到图片或 `on_layout_changed` 回调复活旧页。
   - **撤权不推进 `AuthEpoch`**，所以「当前 session 仍有效」不能作为保留已显示内容的理由：在线读取的聚合、README 及其内外图片与历史快照使用同一失效判据。撤权只让当次展示的快照退休——清理时同时推进页面 generation、清空 section request id（使已读完但尚未投递的回复也过不了 `accepts()`，无法取消随后的重试），并清除失效标记、释放旧请求；其他在线读取重新写入快照后，重试可按新 stamp 恢复，不会永久卡在错误态。
   - 回复先过 binding/request/context（旧绑定的迟到回复直接安静丢弃，无权改动新页面），再在 `drain` 出队后、`view.update` 之前用**回复携带的同一** stamp 做一次有效性检查：后台读到成功但发布前撤权、换 PAT 或被拒绝的回复丢弃并退休已显示内容；新捕获的 stamp 不等于旧回复可信。该提交点检查的最终形态是 cache-runtime 共享权限票据（`CacheStore::authorization(scope, None)` + `is_current()`，无锁无磁盘），本页先以等价的 lease 检查占位，接口发布后迁移。
   - 运行期缓存故障（SQLite/I-O 等）不是授权结论：除 `CacheError::Stale` 外，四类读取都会退回原 `PageReadSession` 在线读取，仍保留取消、上下文与授权检查；只有真实 401/取消/过期上下文才保持原义。

## Host adapter / mounting 职责

正式集成任务必须把组件声明进自己的 `ui` 模块树（本任务没有改 `src/ui/mod.rs`），并在 Profile 页面会话中持有 `Entity<ProfileView>`：

```rust
let view = cx.new(|cx| ProfileView::new(target, binding, range, window, cx));
let request = view.read(cx).request();
// 后台读取完成后，先复核服务上下文和页面请求，再发布成功或错误。
view.update(cx, |view, cx| {
    view.update_basic(&request, basic_state, cx);
    view.update_repositories(&request, repositories_state, cx);
    view.update_contributions(&request, &range, contributions_state, cx);
});
view.update_in(cx, |view, window, cx| {
    view.update_readme(&request, readme_state, window, cx);
});
```

- `ProfileTarget { host, id }` 是**被查看用户**的稳定资源身份，不是当前登录账户。`ProfileBinding { account, epoch }` 来自 host 捕获的授权/服务上下文。不能把 login 当授权身份，也不能用固定 SHA 代替服务上下文校验。
- 每个独立页面会话从 `request()` 取得 `ProfileRequest`，将它随基本资料、仓库、README 与贡献读取一起捕获。后台结果先经现有 page-data/服务 fresh-context 核验，再调用窄 `update_*`。组件再检查 target/binding/generation；贡献更新还必须匹配组件当前 range。返回 `false` 表示旧结果已丢弃。
- 切 Profile、切账户、授权 epoch/服务 binding 变化或改贡献区间时调用 `bind(target, binding, range)`，取得新 request。它清空所有旧 section、Reader、焦点动作与滚动状态；同一 login 的另一位用户、A→B→A 与旧错误响应都不能复用旧显示。独立 section 的 Loading/Error/Ready 可以分别更新，不互相清空。
- **README 发现属于 page-data/host：** 已合入的 [`PageReadSession::profile_readme`](page-data-service.md#个人-readme-helper) 可供后续 adapter 调用；尝试该用户同名 **PUBLIC** 仓库根 README，读取固定内容并准备 `PreparedContent` 后供给组件。旧版 Share-to-profile 标记当前 API 不可观察，组件不宣称与网站完全相同的资格判定。未发现、不可见、 denied 与真实空文件必须保留服务语义，不能猜测。
- 组件不选择默认日期。未来 host 按已批准产品策略提供**最近滚动一年**的 `ContributionRange`；本组件不在内部读取时钟或另算默认区间。
- `update_readme_with_context` 只用于没有服务 `PreparedContent` 身份的独立 fixture；正式 adapter 使用 `update_readme`，保留 `ReaderView::from_prepared` 的固定 repository/commit/path 相对链接上下文。
- 通过 `on_action` 接收 `ProfileAction`。每个动作携带产生它的 request 和类型化 `ProfileActionKind::Profile / Repository(RepositoryTarget) / ExternalLink / Readme(ReaderAction)`。host 决定是否打开 Profile、仓库或外链；仓库动作保留已有 `databaseId`（API 允许缺失）与 nameWithOwner，不发明身份，不自动调用系统浏览器。
- Reader 的仓库相对链接仍是原有 `ReaderAction`，由 host 处理。头像 URL 不交给 loader；组件无 PAT、HTTP、storage 或 cache 依赖。

## Pinned / Popular 策略

只显示 repository pins；实际观察到 gist pins 时给出简短提示，不增加 gist 功能。`PinnedRepositories` 使用专用 repository-pin 连接的顺序；部分 GraphQL 结果中该连接为空而 showcase 仍有类型化仓库时，可展示那些已观察仓库，并显示 partial 提示。

| 服务状态 | 展示 |
| --- | --- |
| `has_pinned_items=false` + `PopularRepositories` | 标题为 Popular repositories，保留 GitHub 返回顺序 |
| 完整 repository pins | 标题为 Pinned repositories |
| mixed pins | 只显示仓库，提示观察到的 gist pin 数 |
| gist-only | 显示 gist 提示；没有 repository pins 不等于用户没有仓库，**不转 Popular** |
| truncated | 只显示已观察前缀并明确 truncated，**不据前缀决定 gist-only/mixed，不转 Popular** |
| unclassified / 不一致状态 | 只显示可用类型化仓库，保留不确定性，**不发明 Popular** |
| `errors` 与数据并存 | 保留可用数据并显示 Partial data；不冒充完整结果 |
| 服务错误/denied | 独立错误面板，不显示成功空列表 |

## Contribution 语义

- 日期必须落在 host 明确区间内；范围生成与闰年/年边界经过验证，遵守领域 `MAX_DAYS=366` 的起止差上限（包含首尾时最多 367 天）。非法/重复日期、范围外记录不会被并入其他日期。
- 服务返回的 `count=0` 是真实零贡献；缺失 day 是 Missing，不显示为零。GraphQL field errors、缺失日期、无效/范围外记录都有 partial 说明。
- 已知 `NONE` 与四个 quartile 使用固定展示 ramp；未知 level 显示独立未知色，保留实际 count 与 level unavailable 文本，**不用未知 level 推断贡献数，也不把服务 color 字符串当 CSS**。
- 贡献图使用 12px cell、Monday-first 星期标识、按区间真实日期推导的月份列与 Less/More 图例。首尾周不足七天保留空槽，missing、zero、unknown level 独立着色；颜色使用现有 theme token。宽屏可容纳全年，窄窗口有界横向滚动，不压缩全年 cell。页面纵向与热力图横向分别使用共享 `Motion`/`Bar`/`Bars`。

## 独立预览与产品隔离

```sh
cargo run --example profile
```

预览默认 Normal / Design（短资料、短 README 正文、四卡片、完整一年），用单个开发场景 Select 保留 Partial + long、Mixed pins、Gists only、Truncated、Unknown levels、Missing fields、Errors、Real empty。`cargo run --example profile -- --long` 可直接打开长场景，只属于 example。窗口默认 1100×800 client，最小 880×560；底部只报告 typed action，不自动导航。场景选择器、fixture 与 launcher 仅在 example/test assembly，产品 binary 的模块树不含它们，没有生产 debug flag。

正式 Shell 挂载、真实 page-data 接线和系统导航仍是后续集成任务。GPUI 测试平台不等同 Windows 原生输入/视觉验收；真实窗口中的 README 选择/滚动、键盘焦点、长元数据、tooltip、触控板、多 DPI 与中文/emoji 字体回退需负责人验收。

## 验证

覆盖见 `src/ui/profile/tests.rs` 与 `heatmap.rs`：binding/account/target 变化与旧更新、部分 GraphQL 数据/错误、缺失基本字段、pin 顺序、gist-only/mixed/truncated/unclassified、真实空与 denied、Reader 链接、typed repository action、闰年/跨年、zero/missing/unknown levels、键盘/鼠标 day detail、长 README/元数据与有界横向范围。实际完整检查与本机窗口边界见 [开发日志](devlog/2026-10-28-profile-view.md)。

## 本轮设计基线与共享 API 阻塞

本轮实际读取负责人指定的原始 04-profile.png，比较内容区的横向资料头、Overview 层级、短 README、两列四卡片及贡献图空间关系；原图仅在忽略目录，不上传。保留 Ely token/字体，不复制旧 Shell/侧栏/颜色，不恢复其他 tabs、Activity 或写操作。

ReaderView 与内部 MarkdownView 当前均 size_full，虚拟 ListState 自持 viewport/Motion；无自然高度或外部纵向流 API。按字符/行猜高度、复制 renderer 或 measure_all 不被采用。已向协调者提交 additive ReaderLayout::Flow 建议：默认 Viewport 不变，复用现有 AST/选择/链接，短正文真实排版占高、长内容保留 lazy 外部页面流，移除冗余纵向 wheel/bar。涉及共享 reader/mod.rs、markdown/document.rs 与 Reader 专属测试/文档，Profile 未越权改这些文件。

负责人最新反馈为除 Overview tab 未居中外，其余当前视觉无问题。Overview 的图标与文字现作为一组在既有 112px tab 内水平居中，保留原高度、间距和下划线范围。README 仍使用现有固定 viewport；上述自然流 API 尚未实现，不把这次视觉反馈写成该能力已完成。`blocked` 仍由负责人管理，Agent 不移除。
