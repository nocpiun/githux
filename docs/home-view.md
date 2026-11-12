# Home / Feed 页面视图

更新：2026-11-04。本文记录可复用 Home/Feed 页面视图的接口、已确认默认值、失败与完整性语义，以及 host/data 职责。底层查询能力见 [Home / Feed 查询服务](home-query-service.md)；本视图不请求网络、不读取凭据、不做关注关系判断。

## 状态与范围

- 已实现：`src/ui/home/**` 的双列呈现、独立状态、绑定/generation 隔离、虚拟化列表、共享滚动条与 wheel motion、键盘选择、typed 动作回调。
- 独立预览：`examples/home.rs` 通过 `src/bin_support/home_modules.rs` 的薄 `#[path]` 装配复用真实组件，提供合成 Mixed / Loading-error / Partial / Filtered-out / Empty-unavailable / Large 场景；可选首个参数按标签或 0-based 索引选择初始场景（仅预览用）。Mixed 与 Filtered-out 都通过真实 `build_feed`；前者混合合法活动与被过滤来源，后者仅含技术噪声和未被关注的主语，验证正常最小空态。
- 测试：`tests/home.rs` 在 GPUI 测试平台执行真实组件，测试代码位于 `src/ui/home/tests.rs`。
- 正式集成：`app::pages::HomePage` 提供账户作用域请求、页面缓存消费、异步结果发布、分页与内部导航，见[正式页面接入](shell-page-integration.md)。正式应用打开 `CacheStore` 仍依赖 `cache-runtime`；Issue/PR 详情正文、自动轮询与用户自定义筛选未实现。示例 fixtures 不是产品路由或运行时开关。

## 已确认默认值（负责人 2026-10-28，2026-10-30 补充筛选）

- Home 只使用 `involves:当前已认证登录 is:open`，无时间窗口，按 `updated` 降序。`is:open` 是实际请求的一部分，最终可见条目只能是 open 的 Issue/PR。
- Feed 只保留三类产品活动：合并的 PR、星标仓库（`WatchEvent action=started`）、创建仓库（`CreateEvent ref_type=repository`）；保留源返回顺序与真实事件身份。Push、issue、评论、Release、Fork、成员、公开仓库、分支/标签创建删除、未知类型与非合并 PR 都不生成卡片、占位卡或回退句。
- **每条卡片的主语必须是当前账户关注的人**（负责人 2026-10-30 补充）。合并 PR 卡片标题为 `<user> contributed to <owner>/<repo>`，`<user>` 是 PR 作者，不是点了 merge 的维护者 actor；星标/建库主语是事件 actor。合并证据为 `action=merged`，或旧形状的 `action=closed` + `pull_request.merged=true`；`closed` 单独不算合并。`action=merged` 的 payload 没有作者，作者来自 PR 详情读取；作者缺失、读取失败或详情未确认合并时整条不显示，并在 Partial 原因里报告。
- **关注关系语义**：关注列表不完整/读取失败时，缺失的登录属于“未知”，既不当“已关注”也不当“未关注”，相关卡片扣留并显示 Partial；只有完整空列表才表示谁都没关注。策略在 host 里执行一次，视图不在 `apply_*` 里重算。
- **Follow 两类事件暂缓**（负责人 2026-10-30“先实现三类，关注两类暂缓”）：没有可靠事件来源前不实现、不占位、不伪造时间戳。暂缓的只是那两类事件卡片，**读取关注关系本身是已实现且必需的**；见 [Home 查询服务](home-query-service.md#35-follow-事件两类暂缓负责人-2026-10-30-确认)。
- 不扩大 review-requested、不把结果解释为未读、不自动轮询；初始只提供手动刷新。
- 搜索最多返回 1,000 条、Events 只覆盖最近 30 天且最多 300 条；页面不得把这些边界伪装成完整历史。筛选后为空是正常最小空态，不是 API 错误或不完整页；真实上游分页失败仍单独保留为 Partial。

这些默认值记录在本组件及预览中；产品范围见 [产品约定](product.md)。

## 输入契约

`HomeView` 不构造服务、不读取 PAT、不访问网络。host 必须先对不透明授权完成服务侧核验，再提供结果；视图的 binding/generation 只是旧响应保护，不是授权检查。

```rust
pub struct RequestBinding(String); // host 选择：稳定账户/页面身份，不含凭据
pub fn bind(&mut self, binding: RequestBinding, generation: u64, ...);

pub fn set_loading(&mut self, column, binding, generation, ...) -> bool;
pub fn apply_home(&mut self, binding, generation, HomeSearchOutcome, ...) -> bool;
pub fn apply_feed(&mut self, binding, generation, FeedOutcome, ...) -> bool;
pub fn apply_feed_page(&mut self, binding, generation, FeedOutcome, ...) -> bool; // 触底追加，不重置滚动/选择
pub fn set_repository_descriptions(&mut self, binding, generation, HashMap<String, String>, ...) -> bool;
pub fn set_repository_description(&mut self, binding, generation, repo, Option<String>, ...) -> bool;
pub fn feed_append_loading(&mut self, binding, generation, ...) -> bool;
pub fn feed_append_failed(&mut self, binding, generation, page, ...) -> bool;
pub fn apply_error(&mut self, column, binding, generation, HomeError, ...) -> bool;
pub fn set_unavailable(&mut self, column, binding, generation, message, ...) -> bool;
pub fn set_refresh_availability(&mut self, column, binding, generation, RefreshAvailability, ...) -> bool;
```

- 每次应用都要求 binding 与 generation 同时匹配；旧绑定、旧 generation 的成功与错误都返回 `false` 且不改变状态。
- `bind` 切换绑定前清空两栏内容、选择与滚动位置，旧 A→B→A 响应不会复活。
- 新快照到达时清除该栏旧的键盘选择，避免选择指向另一快照或另一账户的条目。
- 初建或 `set_unavailable` 是“无当前数据”，不是成功空列表；离线/无缓存不得显示成“没有活动”。
- `set_loading` 只影响一栏；另一栏的内容、错误或滚动不被擦除。
- `apply_error` 只终止对应栏；`HomeError` 的认证、限流、SSO、网络、超时、取消和解析失败分别给出不冒充空结果的提示。
- `RefreshAvailability` 由 host 根据 cooldown、在途、限流或轮询状态提供。不可用时刷新按钮禁用且 `refresh` 被忽略；首次加载（Loading）或一次可用刷新进入 refreshing 后，同一栏在下一次结果/错误前拒绝再次刷新。

## 输出动作

`on_action` 接收 `HomeAction`；所有身份为稳定资源身份，不修改全局路由，也不打开 URL。

| 动作 | 内容 |
| --- | --- |
| `Refresh(RefreshRequest)` | `column`、触发时观察到的 binding 与 generation |
| `LoadMoreFeed(RefreshRequest)` | Feed 触底时请求下一页更旧的活动；携带 binding/generation，host 重新核验后按 cursor 追加 |
| `OpenPullRequest { repository, number }` | PR 正文“Read more”的应用内目标：仓库身份与仓库内 PR 编号；host 打开对应的 Githux PR 标签 |
| `Repository(RepositoryIdentity)` | `id`（Feed 有，Home 搜索模型无）、owner、name |
| `Profile(ProfileIdentity)` | `Option<u64>` id 与 canonical 登录名；actor 保留真实 id，owner-only 为 None |
| `ActivateWorkItem(WorkItemIdentity)` | Home 条目的全局 id、number、Issue/PR kind、仓库 |
| `ActivateFeed(FeedIdentity)` | Feed 卡片来源事件的真实 id、typed event kind、仓库 |

Feed 的领域 payload 不保存 Issue/PR 全局 id，因此 Feed 激活不会把仓库内 `number` 冒充为全局 id。Read more 使用独立的 `OpenPullRequest`，以仓库与编号定位资源，不依赖全局 id 或外部 URL。

## 呈现与失败语义

- 两栏标题严格为 **Home** 与 **Feed**，各自拥有独立 header、状态区、虚拟化行、焦点、选择、滚动偏移、`Bars` 与 `Motion<ListState>`。
- 单栏最多同时呈现 loading、empty、error、unavailable 或 Ready 中的一种；Partial 仍保留已取条目并显示原因提示。
- Home 保持紧凑 work-item 列表；Issue/PR 图标和标题在同一 flex 行对齐，metadata 位于其下。与全局 Issues/PR 共用 `work_item_row`：56 logical px 行高、8px 水平内边距、两行间距 3px，metadata 的编号起点与标题文字左沿对齐。owner/name 无额外布局空隙地组成 `owner/repo`，各自独立导航；owner 的宽度上限按可用 metadata 空间计算，为窄栏名称保留空间，同时避免因 owner/name 长度比例而提前截断正常 owner；更新时间为相对时间。链接与 tooltip 的交互节点只占实际显示文字宽度，非交互的横向 flex slot 吸收余宽；长标题/名称在可用宽度内原生截断。整行原本的选择/激活行为由调用方保留。
- Feed 是紧凑的活动卡片：avatar、可点击的 credited 用户、动作句与相对时间组成 header，inset repository panel 放目标仓库及相关内容：
  - **星标仓库**：在仓库链接下方展示仓库真实描述（通过 `set_repository_description` 异步丰富），三行限制截断；描述缺失或为空时直接静默，不画空占位、不保留额外边距，也绝不渲染“Unavailable / Retry”错误段落。
  - **合并 PR**：展示 PR 标题与紧凑的正文摘要（`BodyPreview`，自动剥除 Markdown 标记、HTML 注释与标签，保证只展示纯净可读文本）；内容较短时完整展示、无展开按钮；内容超出上限（约 200 字符）时截断至单词边界并附省略号，附“Read more”按钮。
  - **Read more 交互**：点击或键盘激活经宿主的延迟导航通道打开 `Route::PullRequest { namespace, name, number }`。标签显示 `owner/repo #number` 和 PR 图标；同账户同仓库同编号复用标签，仓库名大小写别名不重复开页。当前仅接入内部导航，详情内容仍待独立实现；不打开系统浏览器、不在卡片内展开全文；阻止事件冒泡，不触发卡片选中或次级激活。
  - 没有 branch/ref/SHA/commit 数等技术噪声 prose，也不为被过滤事件补造正文。
- 头像使用真实模型的名称首字母，忽略可选 `avatar_url`，没有网络图片路径。
- 不再渲染 count/order 行，也不为它预留高度；metadata 仍保留在输入模型。`incomplete_results=true`、搜索超过 1,000 上限及 Partial 仍明确警示，不声称完整历史或未读列表。两列不画外框或 header/body 分割线，仅 Feed 卡片有细边框。
- 双列模式使用约 32 logical px 的间距、等宽列；窗口较窄（小于 760 logical px）时上下堆叠，纵向间距保持约 16px。顶部/左右留白不变；标题到首内容 8px、Partial 提示前后各 8px、页面末尾 8px 均为必需间距。起点间距属于首项的滚动布局，末尾使用原生 content padding；数据 viewport/paint clip 覆盖固定 header/提示区下沿至所属面板底边，中间偏移不会被额外空白带遮挡。实际追加状态显示时保留其内部间距及页底 8px，数据区域截止于控件上沿。生命周期 canvas 仍为绝对定位零尺寸，不贡献 flex gap。
- Feed 头像使用 `AvatarSize::Lg`（40 logical px），圆形且不收缩，与 actor/action + 相对时间组成的 header 整体居中；保留原 Profile 点击和键盘动作，无网络图片。

## 滚动与键盘

- 两栏的行 viewport 在尾侧 8px 独立 scrollbar 通道前结束，行背景不会伸到条下面；3px 可见线与行边缘相距 3.5px，外侧留 1.5px。隐藏条不回收通道，保证卡片换行、列表测量和分页位置稳定。几何与命中语义统一见 [列表 viewport 与尾侧通道](scrolling.md#列表-viewport-与尾侧通道2026-11-11)。专属回归在 `src/ui/home/viewport_tests.rs`，正式 Shell 组合回归在 `tests/scroll_viewports.rs`。

- 每栏使用真实 GPUI `ListState`、共享 `scroll::Bar`/`Bars` 与 `scroll::Motion<ListState>`，只渲染可见行。Home 保持固定行高，Feed 自然测量可变卡片高度；不调用 measure_all，不新建滚动几何。键盘远跳使用原生 logical item anchor，在 Motion tick 后应用，避免旧范围 clamp 覆盖末项 reveal。
- **Feed 自动追加**：最后三张卡片之一进入可见区时提前发一次 `LoadMoreFeed`；空页和不足三张卡片的首屏自动继续。使用已测量行的位置与原生逻辑滚动锚点，不把虚拟列表未知高度当成内容耗尽；每帧布局后也核验一次，首屏无需等待额外 hover/时钟通知。每账户/代次同一时刻至多一次在途追加，渲染、resize、重复滚动回调与刷新都不能重复请求。追加用 `ListState::splice` 在原列表尾部增长，保留滚动锚点与键盘选择；重复/重叠页按源事件 id 去重。加载中用紧凑动画图标，失败保留现有内容并显示黄色告警图标 + 点击重试，真正耗尽（服务端无 next 链接）后停止请求。
- 捕获阶段 `wheel_overlay` 每区域只消费一次，测试验证一次精确滚轮只改变目标栏且偏移恰好等于输入。
- 每栏是独立 Tab stop。列表根聚焦时 Up/Down/Home/End 移动选择，Enter/Space 激活所选 Home 条目或 Feed 事件；各内容链接也是独立键盘入口，键盘激活不抢回列表焦点。
- 移除右侧 Repository/actor 按钮。owner/name 各自 pointer + hover underline，斜杠普通文本且两侧无布局 gap；actor/avatar 打开 canonical login Profile，Issue/PR 标题激活对应 item/event。Feed 的 actor/repository/PR title 和 Read more 同样只由可见文字区域响应，纵向卡片不能把文字链接拉伸到整行。每个链接阻止冒泡，不发生卡片/列表二次激活。

## 正式 Shell 接入（2026-10-29）

本组件已由 `app::pages::HomePage` 挂到正式 Shell 的 Home 路由。host 使用 `PageReadSession::home(None, …)` 与 `feed(…)`，固定当前认证 login、无时间窗口、updated 降序，不自动轮询；Feed 的数据层负责源事件、`/user/following` 与有界 PR 作者详情读取，并在发布前完成策略过滤（`FEED_AUTHOR_LOOKUP_LIMIT = 30`、关注列表最多 1,000 账户/10 页）；`FeedOutcome` 带 `next_page` 游标，host 在 `LoadMoreFeed` 时按游标逐页追加，而不是预先读完整条（至多 300 条/30 天）时间线。两栏分别按服务上下文、TabId/incarnation 和请求代次发布。登录前或授权不可用时显示无当前数据，不注入 synthetic items。Feed 的 Poll-Interval 由 host 转成刷新禁用窗口。头像由 `app::avatar` 解析后经 `set_avatar_renderer` 只传入已解码元素；renderer 接收的是 credited subject（合并 PR 时为 PR 作者）。`FeedOutcome` 只含已通过策略的 `FeedActivity`，视图不会再自行过滤，也无法从显示字符串推断关注状态。接入与验证边界见 [Shell 页面接入](shell-page-integration.md)。

## 2026-11-11 Cache-first 首次内容呈现

- `HomePage` 在 `start` 时为每个栏目额外启动一个**只读本地快照**的作业（`read_cached_first`，`PageReadMode::Offline`），与原有在线读取并发；在线读取先开始，缓存 I/O 不前置。
- 快照命中时以历史快照（`Saved` 标记 + 原 fetched 时间）立即发布，但**不释放**该栏的 work 槽、不恢复刷新可用性、不触发 Feed 轮询冷却——这些仍由在线结果收尾，因此快照期间刷新保持不可用、不会重复刷新。
- 前台用 `PublishMark`（Empty/Historical/Network）决定覆盖顺序：网络结果永远覆盖快照，迟到的缓存完成不会回滚更新的网络内容；同一快照不重复发布（重复发布只会重置滚动与选择）。
- 缓存未命中、损坏、权限拒绝或存储故障不发布任何内容，保持原有 online loading（不伪装成空页/错误页）。
- Feed 快照同样设置集合代次（`CollectionGeneration`）与 seen ids，快照期间的续页属于同一集合；网络首页到达后整体替换，不混排不同代次的分页。
- 未新增“快照 + 进行中”专用呈现 API：`src/ui/home/mod.rs` 未改动，缓存的“历史在屏”状态由既有 `Saved` 标记与禁用刷新共同表达。

## 2026-11-03 离线页面缓存与历史快照呈现

`HomePage` 现已消费 `data::page_cache::CachedPageReadSession`：
- **离线回退与历史标识**：当且仅当在线网络或超时失败、且当前账户拥有合规缓存时，回退展示本地持久快照；离线未缓存首屏保持 Unavailable（不作为错误标或空活动展示）。历史快照明确保留其原有的 `fetched_at_ms`，并在栏目标题旁显示紧凑 `Saved` 图标与 tooltip 相对保存时间，在追加已缓存旧页时继续保留该标记，只有网络刷新替换整个集合后才清除。
- **分页同源约束**：Feed 触底续读保持严格的同源集合（一次快照周期内全为网络结果或全为历史快照），全过滤空页仍自动续读下一页。若历史 Feed 某一后续页未被缓存，显示紧凑的 `Page N is not available offline` 提示并停止自动重试，绝不把不同代次或新旧混合的分页盲目拼接；同一 Feed 集合内的星标仓库描述批次独立完成，追加新页不会取消前页仍在途的描述查找，而在线刷新替换集合时会取消旧描述批次、重置去重集合与游标。
- **授权门槛与边界**：401 凭据拒绝、请求取消和过时账户上下文绝不转为历史成功；缓存 quarantine/存储故障与账户会话有效性分开处理，当前 `PageReadSession` 仍有效时回退普通在线读取，不为恢复在线而解锁旧私有快照。切换账户、移除账户或替换 PAT 推进上下文后，已有展示副本在 `rebind`/前台检查时清空并回到 Unavailable。
- **权限代次票据（2026-11-04）**：消费 cache-runtime 独立核心提交 `9f87f47` 的 `CacheAuthorization`（`CacheStore::authorization(scope, None)`），票据只做内存原子失效判断，不授予权限、不读磁盘。读取、追加、描述回复均携带同一票据，后台完成入队前与 UI 发布前复核；已显示 Home/Feed/描述副本在现有轻节拍 drain 检测到失效后清除，迟到结果不得复活旧内容。
- **快照集合代次绑定（2026-11-05）**：经协调批准，在 `data::page_cache` 引入 `CollectionGeneration`，Feed 首页在线读取或离线回放时捕获该集合代次并与后续页绑定，快照元数据与 DTO 持久化代次；第一页在线替换后，离线续读旧代次后续页命中未缓存（`FeedAppendState::NotCached`），绝不拼接不同代次的分页。同代次后续页正常回放。通过确定性测试 `acceptance_replaced_first_page_never_appends_older_generation_continuation_offline`。
- **无缓存平滑回退**：当宿主未注入 `CacheStore` 或未绑定分区时，完全保留原有的在线请求链路，行为与接口不发生回归。

## 2026-10-29 第二轮状态呈现

列级硬失败改为共享 warning 标记 + tooltip 原因，不再用整段错误说明占据状态区；first load 仍显示 spinner 与 Loading 文案，Unavailable 仍显示中性圆点和文字。原因是登录/缺数据与硬失败不是同一语义，不能用 spinner 或 warning 互相冒充；刷新、Partial 和空结果行为不变。

## host / data 待接职责

- 用正式 data 服务读取账户作用域结果，并在请求前固定稳定账户上下文；返回后再核对上下文、页面 binding 与 generation，再调用本视图的 `apply_*`。
- 服务侧核验不透明授权；视图不接受 PAT，也不把 binding 当作授权。
- 决定首次加载调用顺序、刷新是否保留旧快照、错误时是否保留旧快照以及 host 自己的 cooldown/rate-limit 状态来源。
- 正式 Shell 挂载、TabId/资源身份核对和 Issue/PR 详情路由由后续集成任务完成。合成预览不代表真实数据集成已完成。

## 源码

| 位置 | 职责 |
| --- | --- |
| `src/ui/home/mod.rs` | 状态、更新 API、动作类型、面板布局与列表接线 |
| `src/ui/home/presentation.rs` | 错误与 Partial 文案、完整性提示 |
| `src/ui/home/rows.rs` | Home 行、Feed 卡片、内容导航与 typed 详情 |
| `src/ui/home/tests.rs` | 真实组件的确定性 GPUI 测试 |
| `examples/home.rs` | 独立原生预览与合成场景 |
| `src/bin_support/home_modules.rs` | 预览/测试的薄 `#[path]` 装配 |
| `tests/home.rs` | 集成测试入口 |

## 相对时间与刷新生命周期

- `time.rs` 无新增依赖，校验 RFC3339 日期、闰年、UTC/显式 offset、小数秒；以 UTC elapsed time 格式化 Just now、1 minute、N minutes、1 hour、Yesterday、N days。无效/缺失为 Time unavailable，未来为 In the future，不显示负值。原始时间保留 tooltip。
- 默认使用 `SystemTime::now()`；`set_now(Some(instant), cx)` 注入固定时间，None 恢复实际时间。每个视图只有一个 30 秒任务，不按行建 timer，不请求网络。
- `set_active(false, cx)` 供保留页面的宿主在切 Tab 时调用；窗口失活/隐藏、view dispose、退出 rendered tree 都停止 timer 并清理 motion。重显时由真实 render 恢复；window observers 只取消，不为已卸载的保留实体启动任务。固定 clock 不创建 timer。
- 刷新为 Ely `IconButton(RotateCw)`，busy 在同一固定方槽显示 Ely 动画 `Spinner`，没有独立 Refreshing 文本。Tooltip/accessibility label 保留；Loading/refreshing/cooldown 继续禁止重复动作。
- 独立预览固定注入 2026-10-28 12:00 UTC；fixtures 包含 10 minutes、1 hour、Yesterday、2 days、missing/invalid/future，`Finish refresh` 由开发 host 应用场景结果来结束 busy。

## 验证

本机 Windows 使用序列化 Cargo helper，独立 target/：fmt、locked all-target check、clippy -D warnings、完整测试、产品与 home example 构建、diff check。最终结果与 PR head 见 PR34 交付评论，不以旧 head 结果代替当前 head。

Home 装配的回归包含共享 scroll/domain 用例。新增/更新覆盖 count/order 行消失、同槽 spinner 生命周期、canonical login 与 owner-only id、独立链接单次动作、键盘子链接、标题/icon 几何对齐、真实可变高度卡片与末项可达、30 秒任务失活/隐藏/释放清理、相对时间边界/offset/无效/未来；保留旧 binding/generation 拒绝、Partial/empty/error、1,000/300 虚拟化和滚轮一次/跨栏隔离。2026-10-30 新增：closed/merged/未知/缺失 Home 状态永不成为行、选择索引或激活目标；只保留三类 Feed 活动且不产生占位卡；合并 PR 归因 PR 作者而非 merge actor；全噪声 Feed 仍是正常空态且 readiness 保持 Complete；仅含 closed 的刷新不会让旧 open 行残留。第二轮新增：视图直接渲染 `build_feed` 的真实策略输出（关注主语过滤 + 瘦 `action=merged` 事件 + 作者解析），断言被过滤事件没有行与占位、合并卡片链接到作者；关注关系不完整时显示 Partial 提示而不是空 Feed。第三轮新增：触底单次 `LoadMoreFeed`、在途去重、`splice` 追加保留滚动锚点与选择、追加失败告警+点击重试、真正耗尽停止。2026-11-01 新增：星标仓库描述展示与空描述静默（多卡共享同一结果）、PR 正文 Markdown 纯文本截断提取、超长正文“Read more”内部 PR 导航且不触发展开卡片；真实 Shell 点击覆盖新建与复用标签，键盘用实际焦点切换与 Enter 激活验证。

本轮实际查看 Windows 原生 PrintWindow 帧：144 DPI，宽布局 2130×1425 物理像素与窄布局 990×1000；计数/列框/分割线/右侧导航按钮消失，Home icon 对齐标题，Feed 卡片 header/inset 详情及相对时间正常。通过本任务 HWND 的消息切换 Mixed/Loading-error/Large，并取得 owner-only 与 repo-only hover underline 帧。未拿源码推断或测试平台代替视觉核验。

限制：原生消息驱动不等于真人输入手感；未确认触控板、IME、多屏/DPI、原生滚轮手感或所有键盘流程。UI 未获负责人验收，PR 保持 pending；最终 exe/PID/head 记录在任务交付状态与 PR 评论。本轮另做无凭据公开 API 形状核对，见任务 devlog；未做真实 PAT 的认证 Feed 验收。正式 data/Shell 已接线，其状态不等于真实账户验收通过。
