# 仓库浏览页面（repository-browser）

更新：2026-10-28（返工：可反复浏览的示例、行式文件树、页内 Tabs、分层错误）。R3 的可独立验证切片，位于 `src/ui/repository/**`、开发 fixture `src/bin_support/repository_fixture.rs`、独立预览 `examples/repository.rs`、测试入口 `tests/repository.rs`。它把已合并的 `data::RepositorySession` 与 `ui::reader::ReaderView` 串成一张可复用、真实数据的仓库阅读页。**尚未接入 `src/ui/mod.rs` / `src/app` / 正式 Shell**；接线是后续集成任务。

## 页面结构

沿用已确认的仓库页面结构：仓库摘要 → 横向页内内容 Tabs → Code 区域的左文件树/右阅读器。页内 Tabs 与应用级 workspace 标签是两层不同导航，不共用状态。本轮只有 **Code** 有内容，其余 Tabs（Issues、Pull requests、Releases、Settings）沿用确认的名称、样式与顺序，内容区直接留空，不添加“未实现”卡片、假设置或复制的 GitHub 功能菜单。Releases/Contributors/Languages 属于独立 overview 组件，本任务不接入。

切出 Code 不再绘制树与正文，但保留 Code 导航意图（分支/选中路径/展开目录）；切回时若当前授权仍有效则恢复，失效则先重新验证，绝不恢复旧授权。

## 构造与生命周期（正式接入契约）

主机只注入已存在的能力：`Arc<AccountService>`、`Arc<BackgroundExecutor>` 与一个 `RepositoryLocator`。构造后页面默认**未激活**：

```rust
let services = RepositoryPageServices::new(accounts.clone(), executor.clone());
let page = cx.new(|cx| {
    let mut page = RepositoryPage::new(services, locator, saved_navigation, window, cx);
    page.activate(cx);            // 每次都用全新 session，重新固定授权
    page.on_action(Rc::new(|action, _window, cx| match action {
        PageAction::OpenExternal(url) => cx.open_url(&url),
        PageAction::LinkNotFollowed { .. } => {} // 页面已保证不跨库/跨 commit
    }));
    page
});
```

| 方法 | 语义 |
| --- | --- |
| `activate(cx)` | 新建 `RepositorySession`、推进请求代次、清空旧数据、按保存的导航重新解析分支到固定 commit 并读取 |
| `deactivate(cx)` | 取消在途工作、结束阅读器手势、推进代次；只保留导航意图 |
| `rebind(services, cx)` | 换能力（例如主机恢复了另一个账户）；清空旧授权下的数据，用新 session 重读同一导航意图；**从不复活旧 `RepositorySession`** |
| `navigation()` | 可保存的导航意图：分支名、选中路径、展开目录集合。不含 commit、仓库 id、服务实例或授权 |
| `select_branch(name, cx)` | 切换分支并重新解析固定快照，立即丢弃旧树/内容 |
| `select_tab(tab, cx)` | 切换页内 Tabs；离开 Code 结束阅读器手势，回到 Code 复核授权 |
| `toggle_directory(path, cx)` | 展开/折叠目录；首次展开才发一次 tree 请求，不递归下载整个仓库 |
| `select_file(path, cx)` / `show_readme(cx)` | 切换阅读目标 |
| `retry(cx)` / `retry_directory(path, cx)` / `retry_reader(cx)` / `retry_readme(cx)` | 各自只重试对应资源的单次有界操作 |

## 导航与选中

选中由**当前阅读目标的稳定完整路径**决定（`current_path()`），不是父目录内 index，所以跨目录、跨分支都不会错选。README 的路径来自 `ReadmeDiscovery` 的 descriptor（可能在根、`docs/` 或 `.github/`），不使用伪“README”文件行，也不猜根目录。

阅读器头部显示当前仓库相对路径；“Back to README”入口只在**存在可用 README 且当前显示的是别的文件**时出现——已经显示 README 时不重复，无 README 时不提供假入口。

文件树是清晰的行式导航：每行填满视口宽度、统一行高与左右边距；hover/选中背景覆盖整行（选中的是整行，不是文字胶囊）；键盘焦点用单独的中性描边，与 hover、选中可区分。目录有独立 chevron 槽位、目录图标与名称，文件在 chevron 槽位留空以对齐；缩进随层级统一，目录优先、同级稳定排序。单击目录行或其 chevron 都只切换一次展开状态，且不清空右侧已打开的文件；单击普通文件行打开该文件。长名称单行截断并提供完整路径 tooltip。

加载中、空目录、失败目录分别可辨：目录行自身显示 `loading…` / `empty` / `Failed`（悬停查看原因），失败行内提供**该目录的**有界 Retry，其他目录与当前正文继续可用。截断或不完整子目录显示 `Incomplete`，即使没有保留任何条目也不标成 empty。根目录失败有独立的 `Retry files`，从当前 snapshot 派生根 handle，保留已成功的 README 和资料。

树可键盘聚焦：上下移动可见行、左右展开/折叠或回到父节点、Home/End 到首末行、Enter 打开文件或切换目录，移动行时经列表自身滚动目标滚入视口。只有无修饰的导航键被消费，带修饰键的事件留给平台、输入法与阅读器文本选择。

## 代次与发布

一次浏览代次持有“一个已验证 session + 页面代次 + 分支请求身份 + 分支/commit/资源身份”。分支名本身不足以区分 A→B→A 的两次同名解析，因此每次 `begin_branch` 推进单调 `branch_request`，旧身份的 Revision 及其树/正文结果不论成功或失败都被丢弃。同一路径的多次打开还各有单调 `selection_request`，A→B→A 与同文件重复点击不会让旧正文成功/错误覆盖新结果。发布前直接复核服务 context；链接回调延后执行并携带页面、分支和选中代次，避免回调重入 reader 或使用失效链接。`activate` 在获取 session **之前**清空一切授权绑定状态，所以获取失败时不会残留上一个账户的内容。所有慢操作在既有 `BackgroundExecutor` 的 Tokio runtime 上执行，结果经受锁队列回 GPUI，在 UI 线程排出。每条结果在应用前都核对：页面代次、捕获的 `RepositoryReadContext`、分支名、固定 commit，以及阅读目标身份；过期（切账户、替换 PAT、移除账户、A→B→A、切分支、关闭/隐藏）一律丢弃，成功与错误同等对待。失活时调用 `ReaderView::cancel_interactions`（main 新增的公开接口）结束阅读器手势，不改 reader 实现。

## 文件树加载

`navigation::TreeModel` 惰性保存已加载目录：只有用户展开的目录才发一次 `GET /git/trees/{sha}`；`SnapshotDirectory` 始终携带仓库、commit、tree 与相对路径，展开子树从不使用分支名，也从不带 `recursive`。可见行由展开集合展平，目录优先、名字稳定排序；`MAX_TREE_ROWS`（20,000）与 `MAX_TREE_DEPTH`（64）是有界防护，超过时页面明确说明“树被截断”。树视图用真实 `uniform_list` 虚拟化，只构建可见行；滚动与条走共享 `ui::scroll`（`Motion` + `Bars` + `Bar`），阅读器滚动仍归 reader 自身，不引入第二套 scrollbar。

## 错误分层

失败只表达一个资源，由页面决定它作用于哪一层：

| 层级 | 结果 |
| --- | --- |
| 仓库详情 / 分支列表 / revision | 页面级：替换浏览区，不与“空目录”混淆 |
| 目录 | 该行内表达 + 该目录的 Retry；其余目录与正文继续可用 |
| blob / 文件正文 | 只替换阅读区；仓库资料、分支、树与其他内容保留 |
| README 发现 | `No README`（完整空树证明）/ `README not confirmed`（404 不确定）/ 发现失败，各有独立状态与重试 |

404 只说“该资源不存在或当前账户无法访问”，不断言一定是权限问题。401/403/SSO/限流/超时/网络/解析/截断保持各自区别。所有重试都是用户点按的单次、针对一个资源的操作，不会重新登录、重载整页或互相重置；不存在自动重试循环。

已经显示中的文档不会因为无关目录加载完成而被重新请求：`resolve_selection` 在“正在显示同一路径”时直接保留，读者滚动与选区因此不会丢失。反过来，若选中的祖先目录在已加载的父目录中根本不存在（或该条目是文件），页面立即给出终态而不是无限 Loading，目录加载失败也会终止等待该目录的选中状态。

## 开发预览与 fixture

`src/bin_support/repository_fixture.rs` 是**预览与测试共用的唯一 fixture**，只经开发装配引用，不进入产品：

- 按 method、URL path/query、分支/commit/tree/blob 身份应答，同一资源可反复读取，**与点击顺序、并发完成次序无关**，不是一次性队列。
- Normal 场景含两条真正不同的分支（不同 commit、不同根 tree、不同 README 与 `src`），根 `README.md`、`docs/guide.md`、`docs/guides/nested/deep-note.md`、`src/main.rs`、`assets/`（二进制、空文件、超大文件）、symlink 与 submodule 行；所有展示的普通目录/文件都可操作。
- fixture 不建模的请求返回 `599` 并打印开发侧诊断，明确要求补 fixture，**不伪装成 GitHub 权限错误**。
- 只有明确场景才产生失败：`EmptyRepository`（409）、`ReadmeUncertain`（404 不确定）、`NoReadme`（完整空树）、`DirectoryFailure`/`FileFailure`（只失败一次，单次 Retry 可恢复）、`NoBranches`（完整空分支列表）、`TruncatedTree`（`truncated: true` + 未知类型）、`LongTree`（400 行）、`Delayed`（每次读取延迟 250ms）、`ReadmeInDocs`（首选 README 在 docs/README.md）、`RootFailure`（根目录失败一次，Retry files 恢复）。

`cargo run --example repository` 在页面**之外**提供紧凑的场景下拉框、Reset 与 action 输出；这些脚手架不属于产品页面、不出现在 release binary。临时账户目录与合成凭据同样只存在于预览/测试。

## 验证

页面测试（`tests/repository.rs` → `src/ui/repository/tests.rs`）与预览使用同一 fixture，覆盖：正常首屏（摘要、Code Tab、真实 README、真实路径单一选中、无伪文件行）、两层嵌套目录展开/收起/再展开与只请求一次、同文件重复打开 3 次、README→源码→README、分支真实差异与切回、目录行与 chevron 单次切换且不清空正文、键盘导航不吞修改键、README 相对链接跟随与外部链接只报告、页内 Tab 切换清空/恢复 Code、目录失败/文件失败的作用域与单次重试、无 README / 不确定 / 409 / 空分支列表 / 截断树 / 长树、场景重置后恢复正常浏览、Back to README 的显示条件、慢旧请求不覆盖新选择、授权变化与 A→B→A、失活、rebind、两页独立。
