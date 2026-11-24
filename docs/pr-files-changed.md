# PR Files changed

## 独立组件

`src/github/pr_files` 读取 `GET /repos/{owner}/{repo}/pulls/{pull_number}/files`。使用现有 GitHub 客户端、资源定位器及错误分类。每页 30 个文件，单次响应上限为 8 MiB。响应 `Link` 只提供下一页信号。校验 API 源、端点形状、PR 编号和相邻页码后，始终以原 PR 的仓库/编号重建请求。到达 3,000 项时显示 GitHub 文件上限提示，即使响应已省略 `next`。文件列表可翻回上一页，失败时保留当前页，重试原页码。分页反映每次独立读取时 GitHub 的列表，PR 在两次读取之间更新时，刷新会从第一页重新读取。[GitHub REST 文件列表](https://docs.github.com/en/rest/pulls/pulls#list-pull-requests-files)

文件行保留变更类型及现路径，重命名时在 diff 标题显示原路径。文本 diff 显示 GitHub 返回的 hunk、旧/新行号、增删、上下文与无行末换行标记。`patch` 缺失、空值或空字符串时显示文本 diff 不可用。该响应没有文件是否二进制的标志。有效前缀可读但 hunk 行数或文件增删统计不符时提示 patch 不完整。每个 patch 最多处理 256 KiB、8,000 行和单行 8 KiB，达到本地展示上限后保留此前的真实行并显示不完整提示。滚动列表只构建当前可见的 diff 行。路径和 patch 均按纯文本显示。

`src/data/pr_files.rs` 的 `FilesSession::new(repository_session, target)` 固定一个账户授权作用域和一个 PR。`src/ui/pr_files` 的 `FilesView::new(session, executor)` 无网络副作用。宿主只在进入 Files changed 时调用 `set_active(true, cx)`，隐藏时调用 `set_active(false, cx)`，后者取消在途请求。宿主在仓库、PR 或授权变化时以新会话重建视图或调用 `rebind(session, cx)`。视图在发布前同时复核授权、PR 目标、取消状态和返回页的身份。`refresh(cx)` 重新读取第一页，`is_loading()` 可作为当前页签的刷新反馈。读取和局部 patch 解析走后台执行器，结果仅保留在当前视图内存中。

## 正式接入

生产者为 `feat/pr-conversation`。其正式页面进入 `main` 后，由本任务在已合入的 PR 页签与生命周期入口注册 Files changed，并在 `src/github/mod.rs`、`src/data/mod.rs`、`src/ui/mod.rs` 注册模块。用产品路由验证首次打开 Conversation、首次切入 Files changed、翻页失败重试、切换 PR 及账户和刷新。独立预览以脱敏 fixture 使用同一个 session 和视图。

```sh
node scripts/verify.mjs -- cargo test --locked --example pr_files_changed files_changed_
node scripts/verify.mjs -- cargo run --locked --example pr_files_changed
```

打开预览后选择 **Files changed · PR #1**，依次查看新增、删除、重命名、文本不可用和不完整 patch。第一次进入 **Next** 会返回 503，确认列表仍显示第一页并点击 **Retry**。选择 **Files changed · PR #2** 查看空状态，**Conversation** 可隐藏 Files changed。
