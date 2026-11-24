# 2026-11-24 · PR Files changed

在独立 worktree 实现账户作用域 reader、只读文件与文本 diff 视图和离线预览。所用接口是 GitHub 的 `List pull requests files`，文档规定每页默认 30 项、总计最多 3,000 项。无凭据请求公开仓库 `nocpiun/githux` 的 PR #100，API 版本请求头为 `2022-11-28`，查询 `per_page=2`。观测到 `Link` 用 `/repositories/1423952057/pulls/100/files` 返回下一页，响应中的新增和修改带 `patch`。离线 fixture 以同样的字段及分页结构合成，覆盖额外的删改、重命名、缺失、截断和失败路径。[GitHub REST 文件列表](https://docs.github.com/en/rest/pulls/pulls#list-pull-requests-files) · [公开响应](https://api.github.com/repos/nocpiun/githux/pulls/100/files?per_page=2)

正式 PR 详情在已核对的 `main`（`f888a31`）上仍是空白。从 Home Feed 进入单条 PR 时，Shell 对 `Route::PullRequest` 没有挂载页，页面分支返回空白。后续接入点、接口与本地验收命令见 [PR Files changed](../pr-files-changed.md)。独立预览使用临时账户数据库和合成凭据。 [Shell 挂载入口](../../src/ui/mod.rs#L598) · [空白回退](../../src/ui/page.rs#L26)

- Windows 聚焦测试共 4 项通过，覆盖变更类型和行号、缺失与截断、文件上限、保留当前页并重试、长行横向滚动范围及旧 PR 和旧账户响应。命令运行 32 秒，构建槽等待 631 秒，runner `7c86af0f-bddf-470a-bb6a-724965eb34e0`。
- Windows 原生预览编译通过。命令运行 12 秒，构建槽等待 103 秒，runner `553f38cf-bc93-4605-8169-c32dcf968804`。在真实 Ely 窗口通过 Windows UI Automation 打开 PR #1，触发 Next、Retry 和 Previous，确认次页文件出现、首页文件恢复，随后切至 PR #2 确认旧文件行消失。空态静态文案和正式 Conversation 组合仍待验收。
