# 仓库浏览页面（repository-browser，Native Shell）

更新：2026-11-09。状态：`src/ui/repository/**` 已原位替换为纯 Rust Native Shell 页面，使用真实
`RepositorySession` / 缓存适配器与 NSH 共享 Reader；`RepositoryPage` 实现原位替换契约中的
`PageView`。可视效果由负责人在集成后的 NSH 应用中验收。

历史实现（Ely/GPUI）与逐轮行为依据保留在
[仓库浏览 v1 日志](devlog/2026-10-28-repository-browser.md)、
[仓库 Star](devlog/2026-11-01-repository-star.md)、
[离线阅读](devlog/2026-11-03-repository-offline.md) 与
[本次迁移日志](devlog/2026-11-09-nsh-repository.md)。本文件只维护当前契约。

## 位置与所有权

| 内容 | 路径 |
| --- | --- |
| 页面控制器、`RepositoryPage`/`PageView`、请求/发布门禁 | `src/ui/repository/mod.rs` |
| 纯导航/树模型/失败分类（无 I/O） | `src/ui/repository/navigation.rs` |
| 仓库 overview 组合读取（Releases/Latest/Contributors/Languages/计数） | `src/ui/repository/extras.rs` |
| NSH 视图（摘要、页内 Tabs、文件树、Reader 列、底条） | `src/ui/repository/view.rs` |
| 页内 Tab 枚举 | `src/ui/repository/page_tabs.rs` |
| locator → 数字仓库 id 索引 | `src/ui/repository/locator_index.rs` |
| Overview 状态/组件 | `src/ui/repository_overview/**` |
| 开发/测试 fixture（唯一） | `src/bin_support/repository_fixture.rs` |
| 数据预览 | `examples/repository.rs` |
| 测试入口 | `tests/repository.rs`、`src/ui/repository/tests*.rs` |

`crate::app::runtime`、`crate::workspace`、`crate::ui::foundation`、`crate::ui::reader` 和
`app/pages` 契约由集成作者维护；页面只消费它们。`src/bin_support/repository_modules.rs`（GPUI
装配）与 `repository_overview_modules.rs` 属旧入口的测试装配，迁移后由集成作者决定去留；本任务
不再依赖它们。

## 工厂与生命周期（PageView）

```rust
use crate::app::runtime::{PageContext, RuntimeHandle};
use crate::app::pages::{NavigationSink, PageView};
use crate::workspace::RepositoryNavigation;

let page = ui::repository::RepositoryPage::new(context, owner, name, intent, navigation);
// 可选：宿主把自己的唯一后台执行器交给 README 图片 provider（见下文“集成依赖”）。
// page.set_image_executor(executor);

fn element(&self) -> AnyElement;                 // NSH 组件树
fn poll(&mut self, runtime: &RuntimeHandle);     // 壳的前台 tick/wake
fn refresh(&mut self, runtime: &RuntimeHandle);  // 顶部显式 Refresh
fn set_active(&mut self, active: bool);
fn retire(&mut self);                            // 关闭 tab / 账户绑定变化
```

- `new` 立即在捕获的 `PageContext` 下启动第一次读取；不再有单独的 `activate`。
  构造失败（例如非法 owner/name）时页面显示错误而不会发起请求。
- 普通 tab 切换只调用 `suspend`（页面在 NSH 中保留已验证内容）；离开 Code 页内 Tab 不取消
  仓库读取，只停止绘制树与正文、并卸载 Reader 元素。回到 Code 时用同一授权文档重新挂载
  Reader（不重新读 blob）。
- 账户切换/替换/删除或 `binding_revision` 变化由壳 `retire` 旧页并按新 `PageContext` 重建；
  页面自身也在每次 poll 重新核对授权，不依赖壳是否已重建。
- 导航意图（分支、选中路径、展开目录）只存在页面内；宿主可保存
  `RepositoryController::workspace_intent()` 的 `{branch,file}`。展开集合是页面本地状态。

## 发布与授权

`PageContext` 绑定账户、`TabId`、路由 key、服务实例与读取开始前捕获的缓存撤权令牌。页面对
四个顶层展示资源各持有一个 `PageSlot`：

| 资源 | 页面槽位 | 失败归属 |
| --- | --- | --- |
| 仓库摘要 | `summary_slot` | 页面级 |
| 分支列表 | `branches_slot` | 页面级（branch picker） |
| 固定 commit revision | `revision_slot` | 页面级 |
| 当前文档覆盖（content） | `content_slot` | 只替换阅读区 |

每个读取在开始前 `PageSlot::begin`，回复携带该 ticket；`publish(ticket, value, sessions)`
同时核对 ticket 代次、`PageContext::is_current(runtime.sessions())` 与被读资源上下文。目录树、
README 发现等 revision 子资源没有独立槽位，它们的回复还要核对页面代次、分支请求代次、
固定 commit、路径身份与 `cache_allowed()`（其中包含 revision/content 槽位保存的缓存令牌）。

`poll` 每次都会 `clear_invalid` 已展示值：账户切换、PAT 替换、删除账户、仓库撤权或凭据拒绝
会在**同一次 drain** 内清空摘要、分支、树、正文与 overview；已经显示的数据与新请求受同一门禁，
不做定时探测（LESSONS L022）。旧成功/旧错误同等丢弃；同一分支 A→B→A 由单调 `branch_request`
区分，同一路径重复打开由 `selection_request` 区分。

## 页面结构（NSH）

- 根：`repository-page`，`size_full`、`overflow_hidden`、NSH 默认 `background`/`foreground`；
  语言底条是根的最后一个 `flex_none` 元素，由岛裁剪。
- 顶部：标题（owner/name）、visibility/Archived/Fork 徽章、描述、`Cached` 描边徽章与 `Refresh`
  按钮（仅历史快照时）、Star（真实写）与 Fork（禁用）复合入口。
- Overview：`Releases`/`Contributors` 自然高度紧凑条（同一 `RepositoryOverview` 状态），
  Languages 只在底部 4px 色条。
- 页内 Tabs：Code/Issues/Pull requests/Releases/Settings（icon+label），Issues/PR 数量来自真实
  GraphQL 计数；只有 Code 有内容，其他 Tab 留空（`repository-tab-empty`），不放假卡片。
- Code：左 268px 文件树 + 右 Reader 列。树使用 NSH `virtual_list`（回收行、原生滚动条），
  行高 26px、7px 内边距、目录优先稳定排序、chevron 槽位对齐文件名、整行 hit target、长名称
  tooltip；目录 loading/empty/incomplete/failed 行内表达，failed 行只重试该目录。
- 树键盘：树视口获得焦点时绑定无修饰 Up/Down/Left/Right/Home/End/Enter；修改键、输入框与
  Reader 文本选择不被吞掉。
- Reader 列：当前仓库相对路径 + `Back to README`（仅在存在 README 且显示其他文件时）；正文用
  共享 `crate::ui::reader` 的 `ReaderHandle`/`ReaderDocument`/`ImageBridge`，
  `ReaderLayout::Repository` 保留正文首尾内留白；仓库内相对链接在同仓库同 commit 内跳转，
  跨仓库/跨 commit 只报告、不外跳。
- 加载/失败/空：真实失败是黄色 warning + tooltip，可重试时同一标记可点击；确认的缺省
  （无 README、空目录、README 不确定）各有文字/状态，不伪装成网络错误。

## 缓存与离线

`PageContext::cache()` 有绑定时页面构造 `CachedRepositorySession`/`CachedPageReadSession`，
否则走原在线会话：

- 摘要/分支/revision 使用 `OnlinePreferred`；在线失败回退快照时，后续目录/README/正文改用
  `OfflineOnly`。未缓存资源返回 `RepositoryCacheError::Miss`，页面显示“本地缓存没有该资源”
  （不伪造空目录/空正文），与“网络被拒绝”区分。
- `locator_index` 在账户聚合分区记录 locator → 数字 id；跨重启离线打开时先读索引，首次访问
  才在线解析。索引缺失/损坏/被撤权一律回到在线解析。
- 历史快照页面：顶部 `Cached` 徽章显示**最旧**的缓存验证时间；Star 初始状态不请求、按钮禁用
  且不乐观改数；Open Issues/PR 计数来自历史时不画 tab 数字；overview 各段显示 `Cached` 与
  手动重试，历史 Releases 不挂 `Latest`。
- 显式 `Refresh` 推进页面代次与分支请求代次、清空在途结果与所有保留令牌，再在线重验；旧的
  排队回复无法回填。
- README 图片：`RepositoryImageService::with_cache` + 固定 commit 的资源身份 + 捕获的撤权令牌
  作为 `ImageBridge` guard；令牌失效后图片桥永久 retired，重新授权需要新的文档代次。

## 集成依赖（重要）

1. `RepositoryPage::set_image_executor(Arc<BackgroundExecutor>)`：README 图片 provider 复用应用
   唯一后台执行器。未注入时页面仍读取摘要/树/正文，但图片桥如实显示 `NoProvider`，不会新建
   第二个执行器或网络栈。
2. `crate::app::runtime::Services` 需要现有 `PageContext`/`PageSlot`/`PageTicket`（本页已使用）。
3. `crate::workspace` 需导出 `PageIdentity`、`Sessions`、`TabId`、`Route`、
   `RepositoryNavigation`（`repository.rs` 的测试装配按当前 `src/nsh/runtime/sessions` 导出；
   sessions 迁到 `src/workspace` 后由集成作者调整 `#[path]`）。
4. Contributor 头像仍归账户头像加载器；页面当前以首字母回退呈现（overview 状态已支持
   `AvatarImage` 注入），集成时由壳体提供 decoded image。

## 验证

`src/ui/repository/tests.rs`（`cargo test --lib ui::repository` 于隔离 probe 中）用真实
`AccountService` + 临时 SQLite + 真实 `CacheStore` + 请求路由 fixture 覆盖：正常首屏与真实
README 路径、单一行选中、无伪 README 行、嵌套目录各自一次读取、同文件重复打开、README↔源码、
分支差异与 A→B→A 旧回复丢弃、切分支不重启 overview、README 缺失/不确定/409/空分支列表/截断树、
目录失败与单次重试、页内 Tab 恢复、账户切换清空、旧回复拒绝、跨仓库链接只报告、overview 动作
复核、离线重启同 commit/路径/目录、未缓存 miss、手动 Refresh 恢复在线与 Star、单次 drain 撤权
清空、撤权前回复拒绝、历史计数隐藏；另有 NSH headless 测试验证真实行/README/Star 数字渲染与页内
Tab 空态。Star 写入、超时对账与不重放见 `star_tests.rs`。

边界：以上全部离线；不访问真实 GitHub、不读取真实 PAT，headless 引擎不替代真实 Windows 输入、
IME、焦点和像素验收。`tests/repository.rs`、`examples/repository.rs` 属集成后的测试/预览装配，
需要集成作者的 NSH 依赖与最终模块路径，本任务未在 probe 中编译它们。
