# 2026-11-24 · PR Files changed

在独立 worktree 实现账户作用域 reader、只读文件与文本 diff 视图和离线预览。所用接口是 GitHub 的 `List pull requests files`，文档规定每页默认 30 项、总计最多 3,000 项。无凭据请求公开仓库 `nocpiun/githux` 的 PR #100，API 版本请求头为 `2022-11-28`，查询 `per_page=2`。观测到 `Link` 用 `/repositories/1423952057/pulls/100/files` 返回下一页，响应中的新增和修改带 `patch`。离线 fixture 以同样的字段及分页结构合成，覆盖额外的删改、重命名、缺失、截断和失败路径。[GitHub REST 文件列表](https://docs.github.com/en/rest/pulls/pulls#list-pull-requests-files) · [公开响应](https://api.github.com/repos/nocpiun/githux/pulls/100/files?per_page=2)

正式 PR 详情在已核对的 `main`（`f888a31`）上仍是空白。从 Home Feed 进入单条 PR 时，Shell 对 `Route::PullRequest` 没有挂载页，页面分支返回空白。后续接入点、接口与本地验收命令见 [PR Files changed](../pr-files-changed.md)。独立预览使用临时账户数据库和合成凭据。 [Shell 挂载入口](../../src/ui/mod.rs#L598) · [空白回退](../../src/ui/page.rs#L26)

- Windows 聚焦测试共 4 项通过，覆盖变更类型和行号、缺失与截断、文件上限、保留当前页并重试、长行横向滚动范围及旧 PR 和旧账户响应。命令运行 32 秒，构建槽等待 631 秒，runner `7c86af0f-bddf-470a-bb6a-724965eb34e0`。
- Windows 原生预览编译通过。命令运行 12 秒，构建槽等待 103 秒，runner `553f38cf-bc93-4605-8169-c32dcf968804`。在真实 Ely 窗口通过 Windows UI Automation 打开 PR #1，触发 Next、Retry 和 Previous，确认次页文件出现、首页文件恢复，随后切至 PR #2 确认旧文件行消失。

## 2026-11-26 · 正式 Shell 接线

在 [Conversation PR #109](https://github.com/nocpiun/githux/pull/109) 的 `98b9bcb486357e678139ac8aae057af2fb450ac8` 快照上接入真实 `PullRequestPage`。Files changed 与 Conversation 共用同一账户作用域。文件视图有界，选中时由其自身滚动并接收顶部刷新。离开工作区标签时取消文件在途请求，返回已读取的 PR 时复用视图。

Windows 通过 `node scripts/verify.mjs -- cargo test --locked --bin githux files_changed_` 的 5 项回归，包含正式 Shell 的 GPUI 测试。该测试打开两条 PR，确认 Conversation 零文件请求、切入后绘制文本 diff、PNG 和纯重命名显示不可用、另一 PR 显示空状态且没有旧行，最后返回原 PR 并切回 Conversation。命令运行 64 秒，构建槽等待 335 秒，runner `b241568f-1e0d-4512-96cc-0d9e49382d51`。本轮使用离线 fixture，负责人已将最终人工外观验收安排到 Issue/PR 阅读批次统一进行。

#109 合入后，仅将本任务的两个提交移到包含 Home 和 Notifications 的 `f12a9e2`。Windows 格式检查和 20 项正式页面测试通过。测试命令运行 69 秒，构建槽等待 28 秒，runner `e20ebee2-61db-45de-955e-05ab56201125`。

PR #111 首轮审查复现了 UTF-8 字节数不能代表 diff 行实际宽度的问题。现在用 GPUI 布局测量可见行，滚动时更新宽度参考。包含转义双向控制字符的回归会检查最宽行及横向可达范围。首轮 Ubuntu Clippy 报告只编译数据模块的预览中存在未用的 `RowKind` 重导出，已改为由 Files changed 视图直接引用类型。Windows 修订后 5 项 Files changed 回归通过，命令运行 65 秒，构建槽等待 4 秒，runner `2b594c4f-c9a9-4f4a-b4c3-d89487ca871f`。
