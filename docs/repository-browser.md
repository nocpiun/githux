# 仓库浏览页面（repository-browser）

更新：2026-10-28。R3 的可独立验证切片，位于 `src/ui/repository/**`、独立预览 `examples/repository.rs`、测试入口 `tests/repository.rs`。它把已合并的 `data::RepositorySession` 与 `ui::reader::ReaderView` 串成一张可复用、真实数据的仓库阅读页：仓库摘要、分支选择、惰性目录树、默认 README 与选中文件的 Markdown/源码阅读。**尚未接入 `src/ui/mod.rs` / `src/app` / 正式 Shell**；接线是 account-shell 与本页都验收通过后的后续集成任务，本任务不把独立预览当作正式 Shell 已接入。

## 状态边界

本页消费 `AccountService::repo_service()` 的 `RepositorySession`、`RepositoryReadContext` 与既有 `PreparedContent`，并把结果交给既有 `ReaderView`。它不打开第二个账户数据库/runtime，不自行取得 PAT，不下载图片，不做磁盘缓存/离线。固定版本不变量仍遵守 [仓库快照服务](repository-snapshot-service.md) 与 [blob 正文服务](blob-content-service.md)，账户/授权代次与发布边界遵守 [仓库账户数据服务](repository-data-service.md)。

## 构造与生命周期（正式接入契约）

主机只注入已存在的能力：`Arc<AccountService>`、`Arc<BackgroundExecutor>` 与一个 `RepositoryLocator`。构造后页面默认**未激活**，由主机调用 `activate` 才开始读取：

```rust
let services = RepositoryPageServices::new(accounts.clone(), executor.clone());
let page = cx.new(|cx| {
    let mut page = RepositoryPage::new(services, locator, saved_navigation, window, cx);
    page.activate(cx);            // 每次都用全新 session，重新固定授权
    page.on_action(Rc::new(|action, window, cx| match action {
        PageAction::OpenExternal(url) => cx.open_url(&url),
        PageAction::LinkNotFollowed { .. } => {} // 页面自身已保证不跨库/跨 commit
    }));
    page
});
```

| 方法 | 语义 |
| --- | --- |
| `activate(cx)` | 新建 `RepositorySession`、推进请求代次、清空旧数据、按保存的导航重新解析分支到固定 commit 并读取 |
| `deactivate(cx)` | 取消在途工作并推进代次；只保留导航意图，此后落地的成功与错误都无法发布 |
| `rebind(services, cx)` | 换能力（例如主机恢复了另一个账户）；清空旧授权下的数据，用新 session 重读同一导航意图；**从不复活旧 `RepositorySession`** |
| `navigation()` | 返回可保存的导航意图：分支名、选中路径、展开目录集合。它不含 commit、仓库 id、服务实例或授权 |
| `select_branch(name, cx)` | 切换分支并重新解析固定快照，立即丢弃旧树/内容 |
| `select_file(path, cx)` / `show_readme(cx)` | 切换阅读目标；README 与选中文件语义独立 |
| `toggle_directory(path, cx)` | 展开/折叠目录；首次展开才发一次 tree 请求，不递归下载整个仓库 |
| `retry(cx)` / `retry_readme(cx)` | 用户点按的单次有界重试；不存在自动重试循环 |

失败与空态从不混同：无 README 是「完整空根树证明缺失」的克制空阅读器（树仍可用）；404 是「缺失/不可见不确定」；409 是「Git 数据为空或不可用」；截断/Partial 目录与分支列表带明确说明；auth/SSO/限流/网络分别归类。未设计仓库 Tab 不渲染任何占位卡片。

## 代次与发布

一次浏览代次持有「一个已验证 session + 页面代次 + 分支/commit/资源身份」。所有慢操作在既有 `BackgroundExecutor` 的 Tokio runtime 上执行，结果经受锁队列回 GPUI，按 ~30ms 节拍在 UI 线程排出。每条结果在应用前都核对：页面代次、捕获的 `RepositoryReadContext`、分支名、固定 commit，以及阅读目标的身份；过期（切账户、替换 PAT、移除账户、A→B→A）一律丢弃，成功与错误同等对待。后台任务只入队不触碰 GPUI；关闭/失活取消在途并阻止旧结果发布。切换分支后旧快照的树/README/正文不再落地。

## 文件树

`navigation::TreeModel` 惰性保存已加载目录：只有用户展开的目录才发一次 `GET /git/trees/{sha}`；`SnapshotDirectory` 始终携带仓库、commit、tree 与相对路径，展开子树从不使用分支名，也从不带 `recursive`。可见行由展开集合展平，目录优先、名字稳定排序；`MAX_TREE_ROWS`（20,000）与 `MAX_TREE_DEPTH`（64）是有界防护，超过时页面明确说明「树被截断」，不是无声丢弃。树视图用真实 `uniform_list` 虚拟化，只构建可见行；滚动与条走共享 `ui::scroll`（`Motion` + `Bars` + `Bar`），阅读器自身的滚动仍归 reader-motion。

## 链接

阅读器发出的 `ReaderAction` 由本页接管：绝对 `http(s)` 转交主机 `PageAction::OpenExternal`；同仓库同 commit 的相对链接在本页代次内跟随（不落到分支 latest）；跨仓库、跨 commit 或危险 scheme 转交 `PageAction::LinkNotFollowed` 或直接拒绝。Markdown 图片占位保持 reader 既有行为。

## 预览与测试

`cargo run --example repository` 用确定性脚本传输 + 临时账户目录 + 合成凭据跑真实服务管线，无真实网络/凭据。`tests/repository.rs` 是薄模块装配，测试体留在实现旁。生产 binary 不声明这些模块，release artifact 不包含 `examples/repository.exe`。
