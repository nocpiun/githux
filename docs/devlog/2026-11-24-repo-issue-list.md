# 仓库 Issues 列表

2026-11-24 · `repo-issue-list`

基线 `f888a31` 的仓库页点击 Issues 后只有空内容区。任务为该页加入 Open/Closed、仓库级 REST 分页、按账户分区的首批快照，以及交给仓库宿主的 typed `OpenIssue` 事件。仓库页保持同一实例时保留翻页结果与滚动位置。列表以创建时间降序读取，每页 30 条，首批使用现有页面缓存；后续页面保留在当前仓库会话中。行为与源码入口见[仓库浏览](../repository-browser.md#仓库-issues)。

通过 GitHub 官方文档和公开的 `octocat/Hello-World` 无凭据响应核对 `2022-11-28` REST 契约。公开响应包含 Issues 与 PR，`Link` 的 `next` 可改用 `/repositories/{id}/issues` 并带 `after`。用原创的确定性数据测试，公开响应未提交。 [GitHub 仓库 Issues](https://docs.github.com/en/rest/issues/issues?apiVersion=2022-11-28#list-repository-issues) · [分页](https://docs.github.com/en/rest/using-the-rest-api/using-pagination-in-the-rest-api)

## 无 PAT 预览

在 `feat/repo-issue-list` 运行 `node scripts/verify.mjs -- cargo build --locked --example issue_detail`，再执行 `target/debug/examples/issue_detail.exe --repository`。该入口启动正式 Shell 和 #106 的详情页面，传输与账号数据使用可重复的合成样例。

1. 在侧栏打开 `acme/widgets`，选择仓库内的 **Issues**。默认 Open，第一批有 30 条。点击 **Load more**，再滚到末尾。全 PR 的源页会被跳过，最终出现 `#42` 和 `#43`。
2. 点击 `#42`。核对独立详情标签、Markdown 正文和评论，点击 **Load more comments** 查看第二页。关闭详情标签，核对原仓库仍停在 Issues 的 Open 筛选、已加载页与原位置。`#43` 可验证正文保留和评论 Retry。
3. 把 `acme/widgets` 切到 Closed。首次读取返回 503，点击 **Retry** 后显示 **No issues**。从侧栏打开 `acme/other`，Open 有另一个 `#42`，Closed 为空。打开它应进入不同的详情标签，显示空正文和空评论。

## 组合验收步骤

负责人 2026-11-26 授权消费者叠加到 #106 的 `feat/issue-detail`。仓库宿主复核 `PageAction::OpenIssue` 的账户上下文后，经 `PageNavigation::Open` 消费其 `Route::Issue { namespace, name, number }`。以下步骤用于正式应用的真实账户验收。

1. 登录同一账户能访问的两个测试仓库 A、B，预先各建一条编号相同、标题和正文不同的问题。A 准备足够多的 Issues 和 PR，让至少一个源页全为 PR，且后面仍有 Issue。另准备一个筛选结果确实为空的仓库或状态。
2. 从侧栏打开 A，进入 Issues，核对默认 Open。切到 Closed 并切回 Open。用 **Load more** 跨过全 PR 页，直到能看到后面的 Issue。确认列表没有 PR，真正的末页不再显示 **Load more**。在空仓库验证空提示。
3. 在 A 的 Closed 或 Open 中选一个有多页的状态，加载后续页并滚到中段。点击标题或选中行后按 Enter。确认应用内打开详情，编号、标题、正文与 A 匹配。回到仓库标签并核对原筛选、已加载页、选中行及滚动位置。用同号的 B 条目重复，确认从 B 得到 B 的详情。
4. 在有缓存的同一账户重新打开 A 的 Issues。用慢网络或断网观察首批行和顶部刷新，确认刷新期间列表仍可读。断网失败后检查失败和 **Retry**，恢复网络重试。用没有仓库访问权限的另一账户确认看不到 A 的已有行，界面区分访问失败和空列表。快速切换筛选、仓库和账户，观察旧结果不会回填。
5. 返回 Code，确认分支、树、README 和选中的文件恢复。用键盘和滚轮检查列表的起点、中段、末尾与较窄窗口下的滚动。

## 验证状态

#106 已作为 `951da32` 合入 `main`，生产者快照与该提交的文件树一致。#108 将消费者改动重放到 `main`，稳定补丁 ID 与叠加分支一致。负责人暂时通过单项外观门槛，阅读批次交付后仍进行合并人工外观验收。

2026-11-26 核对前次聚焦运行 `def53451-17c1-4da7-93c4-7b518a9927c7` 的退出码为 101，编译错误来自把 `TransportError` 传给要求 `MockFailure` 的测试传输。已改用 `MockFailure::Timeout`。组合回归从实际仓库 Issues tab 点击 **Load more** 和标题，核对 #106 的 Markdown 正文及评论挂载，以及关闭详情后的条目位置。聚焦运行 `85187710-8753-403f-b97f-50d86722a3cb` 退出码 0，7 项通过。共享构建槽等待 44.7 秒，执行 62.1 秒。正文和评论使用确定性样例。Windows 预览中实际打开了 `acme/widgets #42`，核对其 Markdown 标题、正文和代码块，关闭详情后返回原处。Closed 首次 503，点击 Retry 后显示 No issues。真实 GitHub 账户验收由负责人执行。
