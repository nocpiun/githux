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
| `Profile(ProfileIdentity)` | actor 数字 id 与登录名 |
| `ActivateWorkItem(WorkItemIdentity)` | Home 条目的全局 id、number、Issue/PR kind、仓库 |
| `ActivateFeed(FeedIdentity)` | Feed 事件 id、typed event kind、仓库 |

Feed 的领域 payload 不保存 Issue/PR 全局 id，因此 Feed 激活不会把仓库内 `number` 冒充为全局 id；需要详情导航的 host 应使用 `ActivateFeed` 的事件 id/kind，或在未来 data 层补充字段后再扩展。

## 呈现与失败语义

- 两栏标题严格为 **Home** 与 **Feed**，各自拥有独立 header、状态区、虚拟化行、焦点、选择、滚动偏移、`Bars` 与 `Motion<ScrollHandle>`。
- 单栏最多同时呈现 loading、empty、error、unavailable 或 Ready 中的一种；Partial 仍保留已取条目并显示原因提示。
- Home 行区分 Issue/PR、标题、`owner/repo #number`、state 与更新时间；标题和长元数据使用省略号，不横向撑破栏。
- Feed 行显示真实模型的 actor、动作摘要、仓库与时间；`display_login`/`login`/仓库名/时间缺失时使用安全回退。未知类型只显示类型名和“activity”，不展开原始 payload。
- 头像使用真实模型的名称首字母，忽略可选 `avatar_url`，没有网络图片路径。
- `incomplete_results=true` 即使被外部构造成 `Complete` 也会在摘要中标注；Partial 明确说明不是完整历史或未读列表。
- 窗口较窄（小于 760 logical px）时两栏改为上下堆叠；两栏始终留在视图边界内。

## 滚动与键盘

- 每栏使用 `UniformListScrollHandle` 的真实 base handle、共享 `scroll::Bar`/`Bars` 与 `scroll::Motion`，只渲染可见行，不新建第二套滚动几何。
- 捕获阶段 `wheel_overlay` 每区域只消费一次，测试验证一次精确滚轮只改变目标栏且偏移恰好等于输入。
- 每栏是独立 Tab stop；指针点击行会聚焦该栏。Up/Down/Home/End 移动选择，Enter/Space 激活当前 Home 条目或 Feed 事件；指针点击与键盘激活都只发一次回调。
- 行内 Repository、Profile 按钮阻止冒泡，避免一次点击同时激活整行和按钮。

## host / data 待接职责

- 用正式 data 服务读取账户作用域结果，并在请求前固定稳定账户上下文；返回后再核对上下文、页面 binding 与 generation，再调用本视图的 `apply_*`。
- 服务侧核验不透明授权；视图不接受 PAT，也不把 binding 当作授权。
- 决定首次加载调用顺序、刷新是否保留旧快照、错误时是否保留旧快照以及 host 自己的 cooldown/rate-limit 状态来源。
- 正式 Shell 挂载、TabId/资源身份核对和 Issue/PR 详情路由由后续集成任务完成。合成预览不代表真实数据集成已完成。

## 源码

| 位置 | 职责 |
| --- | --- |
| `src/ui/home/mod.rs` | 状态、更新 API、动作类型、面板布局与列表接线 |
| `src/ui/home/presentation.rs` | 行、Feed typed 描述、错误与 Partial 文案、回退 |
| `src/ui/home/tests.rs` | 真实组件的确定性 GPUI 测试 |
| `examples/home.rs` | 独立原生预览与合成场景 |
| `src/bin_support/home_modules.rs` | 预览/测试的薄 `#[path]` 装配 |
| `tests/home.rs` | 集成测试入口 |

## 验证

本机 Windows、提交时工作区执行：

```sh
cargo fmt --all -- --check
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --locked --example home
cargo build --locked
git diff --check
```

`tests/home.rs` 共 43 项（含共享 scroll 模块用例），本任务新增用例覆盖：两栏独立、单栏错误不擦除另一栏、真空/错误/unavailable 区分、Partial 保留条目、1,000 条 Home 与 300 条 Feed 虚拟化、精确滚轮只动目标栏一次、绑定切换清空、旧 generation 成功与错误拒绝、键盘与鼠标回调一次且身份稳定、Home/End 无选择时的绝对边界、刷新不可用/加载中/双击阻止、窄窗口堆叠、未知事件与缺字段回退、全部 typed event 描述。

真实窗口：`target/debug/examples/home.exe` 以标题 `Githux · Home view preview` 独立启动、Responding、按 PID 正常关闭。该检查只证明启动路径与关闭收尾；锁屏/无输入环境下未做真实键盘、IME、DPI、拖拽和视觉验收，需负责人按 `pending` 验收。未连接 GitHub，也未读取真实 PAT。
