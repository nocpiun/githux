# Home / Feed 页面视图

更新：2026-10-28。本文记录可复用 Home/Feed 页面视图的接口、已确认默认值、失败与完整性语义，以及尚未接入的 host/data 职责。底层查询能力见 [Home / Feed 查询服务](home-query-service.md)；本视图不请求网络、不读取凭据，也尚未挂载到正式 Shell。

## 状态与范围

- 已实现：`src/ui/home/**` 的双列呈现、独立状态、绑定/generation 隔离、虚拟化列表、共享滚动条与 wheel motion、键盘选择、typed 动作回调。
- 独立预览：`examples/home.rs` 通过 `src/bin_support/home_modules.rs` 的薄 `#[path]` 装配复用真实组件，提供合成 Mixed / Loading-error / Partial / Empty-unavailable / Large 场景。
- 测试：`tests/home.rs` 在 GPUI 测试平台执行真实组件，测试代码位于 `src/ui/home/tests.rs`。
- 未实现：正式 Shell 挂载、账户作用域请求、host 适配器、Issue/PR 详情、URL 打开、轮询、筛选、缓存 TTL。示例场景与 fixtures 不是产品路由或运行时开关。

## 已确认默认值（负责人 2026-10-28）

- Home 只使用 `involves:当前已认证登录`，无时间窗口，按 `updated` 降序。
- Feed 按服务器顺序展示现有 typed event 覆盖，未知类型使用简洁回退，初始只提供手动刷新。
- 不扩大 review-requested、不把结果解释为未读、不做事件过滤、不自动轮询。
- 搜索最多返回 1,000 条、Events 只覆盖最近 30 天且最多 300 条；页面不得把这些边界伪装成完整历史。

这些默认值记录在本组件及预览中；`product.md` 仍由 account-shell 任务维护，本任务未修改。

## 输入契约

`HomeView` 不构造服务、不读取 PAT、不访问网络。host 必须先对不透明授权完成服务侧核验，再提供结果；视图的 binding/generation 只是旧响应保护，不是授权检查。

```rust
pub struct RequestBinding(String); // host 选择：稳定账户/页面身份，不含凭据
pub fn bind(&mut self, binding: RequestBinding, generation: u64, ...);

pub fn set_loading(&mut self, column, binding, generation, ...) -> bool;
pub fn apply_home(&mut self, binding, generation, HomeSearchOutcome, ...) -> bool;
pub fn apply_feed(&mut self, binding, generation, FeedOutcome, ...) -> bool;
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
| `Repository(RepositoryIdentity)` | `id`（Feed 有，Home 搜索模型无）、owner、name |
| `Profile(ProfileIdentity)` | `Option<u64>` id 与 canonical 登录名；actor 保留真实 id，owner-only 为 None |
| `ActivateWorkItem(WorkItemIdentity)` | Home 条目的全局 id、number、Issue/PR kind、仓库 |
| `ActivateFeed(FeedIdentity)` | Feed 事件 id、typed event kind、仓库 |

Feed 的领域 payload 不保存 Issue/PR 全局 id，因此 Feed 激活不会把仓库内 `number` 冒充为全局 id；需要详情导航的 host 应使用 `ActivateFeed` 的事件 id/kind，或在未来 data 层补充字段后再扩展。

## 呈现与失败语义

- 两栏标题严格为 **Home** 与 **Feed**，各自拥有独立 header、状态区、虚拟化行、焦点、选择、滚动偏移、`Bars` 与 `Motion<ListState>`。
- 单栏最多同时呈现 loading、empty、error、unavailable 或 Ready 中的一种；Partial 仍保留已取条目并显示原因提示。
- Home 保持紧凑 work-item 列表；Issue/PR 图标和标题在同一 flex 行对齐，metadata 位于其下。owner/name 分段导航，更新时间为相对时间，标题和长名称有界截断。
- Feed 是有间距的活动卡片：avatar、可点击 actor、动作句与相对时间组成 header，详情在下面的 inset repository panel。Issue/PR 显示可用 title/number/status（merged 优先）；Push/ref/release/member/comment/unknown 使用既有 typed payload，缺字段安全回退，不补造仓库统计或正文。
- 头像使用真实模型的名称首字母，忽略可选 `avatar_url`，没有网络图片路径。
- 不再渲染 count/order 行，也不为它预留高度；metadata 仍保留在输入模型。`incomplete_results=true`、搜索超过 1,000 上限及 Partial 仍明确警示，不声称完整历史或未读列表。两列不画外框或 header/body 分割线，仅 Feed 卡片有细边框。
- 双列模式使用约 32 logical px 的间距、等宽列；窗口较窄（小于 760 logical px）时上下堆叠，纵向间距保持约 16px。顶部/左右留白不变，底部约 8px；生命周期 canvas 以绝对定位零尺寸参与 render，不再作为 flex child 留出额外 gap。
- Feed 头像使用 `AvatarSize::Lg`（40 logical px），圆形且不收缩，与 actor/action + 相对时间组成的 header 整体居中；保留原 Profile 点击和键盘动作，无网络图片。

## 滚动与键盘

- 每栏使用真实 GPUI `ListState`、共享 `scroll::Bar`/`Bars` 与 `scroll::Motion<ListState>`，只渲染可见行。Home 保持固定行高，Feed 自然测量可变卡片高度；不调用 measure_all，不新建滚动几何。键盘远跳使用原生 logical item anchor，在 Motion tick 后应用，避免旧范围 clamp 覆盖末项 reveal。
- 捕获阶段 `wheel_overlay` 每区域只消费一次，测试验证一次精确滚轮只改变目标栏且偏移恰好等于输入。
- 每栏是独立 Tab stop。列表根聚焦时 Up/Down/Home/End 移动选择，Enter/Space 激活所选 Home 条目或 Feed 事件；各内容链接也是独立键盘入口，键盘激活不抢回列表焦点。
- 移除右侧 Repository/actor 按钮。owner/name 各自 pointer + hover underline，斜杠普通文本；actor/avatar 打开 canonical login Profile，Issue/PR 标题激活对应 item/event。每个链接阻止冒泡，不发生卡片/列表二次激活。

## 正式 Shell 接入（2026-10-29）

本组件已由 `app::pages::HomePage` 挂到正式 Shell 的 Home 路由。host 使用 `PageReadSession::home(None, …)` 与 `feed(…)`，固定当前认证 login、无时间窗口、updated 降序，不自动轮询；两栏分别按服务上下文、TabId/incarnation 和请求代次发布。登录前或授权不可用时显示无当前数据，不注入 synthetic items。Feed 的 Poll-Interval 由 host 转成刷新禁用窗口。头像由 `app::avatar` 解析后经新增 `set_avatar_renderer` 只传入已解码元素。接入与验证边界见 [Shell 页面接入](shell-page-integration.md)。

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

Home 装配当前 54 项（含共享 scroll/domain 用例）。新增/更新覆盖 count/order 行消失、同槽 spinner 生命周期、canonical login 与 owner-only id、独立链接单次动作、键盘子链接、标题/icon 几何对齐、真实可变高度卡片与末项可达、30 秒任务失活/隐藏/释放清理、相对时间边界/offset/无效/未来；保留旧 binding/generation 拒绝、Partial/empty/error、1,000/300 虚拟化和滚轮一次/跨栏隔离。

本轮实际查看 Windows 原生 PrintWindow 帧：144 DPI，宽布局 2130×1425 物理像素与窄布局 990×1000；计数/列框/分割线/右侧导航按钮消失，Home icon 对齐标题，Feed 卡片 header/inset 详情及相对时间正常。通过本任务 HWND 的消息切换 Mixed/Loading-error/Large，并取得 owner-only 与 repo-only hover underline 帧。未拿源码推断或测试平台代替视觉核验。

限制：原生消息驱动不等于真人输入手感；未确认触控板、IME、多屏/DPI、原生滚轮手感或所有键盘流程。UI 未获负责人验收，PR 保持 pending；最终 exe/PID/head 记录在任务交付状态与 PR 评论。无真实 PAT/GitHub 请求，正式 data/Shell 适配仍待后续集成。
