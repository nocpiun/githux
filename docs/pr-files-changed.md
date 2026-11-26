# PR Files changed

## 独立组件

`src/github/pr_files` 读取 `GET /repos/{owner}/{repo}/pulls/{pull_number}/files`。使用现有 GitHub 客户端、资源定位器及错误分类。每页 30 个文件，单次响应上限为 8 MiB。响应 `Link` 只提供下一页信号。校验 API 源、端点形状、PR 编号和相邻页码后，始终以原 PR 的仓库/编号重建请求。到达 3,000 项时显示 GitHub 文件上限提示，即使响应已省略 `next`。文件列表可翻回上一页，失败时保留当前页，重试原页码。分页反映每次独立读取时 GitHub 的列表，PR 在两次读取之间更新时，刷新会从第一页重新读取。[GitHub REST 文件列表](https://docs.github.com/en/rest/pulls/pulls#list-pull-requests-files)

文件行保留变更类型及现路径，重命名时在 diff 标题显示原路径。文本 diff 显示 GitHub 返回的 hunk、旧/新行号、增删、上下文与无行末换行标记。`patch` 缺失、空值或空字符串时显示文本 diff 不可用。该响应没有文件是否二进制的标志。有效前缀可读但 hunk 行数或文件增删统计不符时提示 patch 不完整。每个 patch 最多处理 256 KiB、8,000 行和单行 8 KiB，达到本地展示上限后保留此前的真实行并显示不完整提示。滚动列表只构建当前可见的 diff 行。路径和 patch 均按纯文本显示。

`src/data/pr_files.rs` 的 `FilesSession` 固定一个账户授权作用域和一个 PR。`PullRequestPage` 用 Conversation 的同一个 `RepositorySession` 创建 Files changed 视图，初始选中 Conversation。首次点击 Files changed 后读取第一页。切回 Conversation 或离开工作区标签时取消在途读取，返回时复用已读取的文件页和选中行。打开另一个 PR 或重建账户挂载时使用新视图。每次发布前复核授权、PR、取消信号和页码。暂时性翻页错误保留前一页，明确的访问失败清除文件内容。读取和 patch 解析走后台执行器，结果只驻留当前视图的内存。

Conversation 使用原有 Shell 纵向滚动。Files changed 使用有界的文件列表和可横向滚动的 diff 视口，避免 Shell 外层拦截滚轮。顶部刷新只刷新当前 PR 页签；选中 Files changed 时重新读取第一页。

## 验证

正式 Shell 的离线测试通过 `Route::PullRequest` 打开 Conversation、切换 Files changed，覆盖首次打开时的请求数、文本行、PNG 与无文本重命名的 patch 缺失、独立 PR 的空列表，以及返回同一工作区标签时保留已读取的文件。独立预览使用同一 reader 和视图，方便复现分页故障与重试。

```sh
node scripts/verify.mjs -- cargo test --locked --bin githux files_changed_
node scripts/verify.mjs -- cargo run --locked --example pr_files_changed
```

打开预览后选择 **Files changed · PR #1**，依次查看新增、删除、重命名、文本不可用和不完整 patch。第一次进入 **Next** 会返回 503，确认列表仍显示第一页并点击 **Retry**。选择 **Files changed · PR #2** 查看空状态，**Conversation** 可隐藏 Files changed。
