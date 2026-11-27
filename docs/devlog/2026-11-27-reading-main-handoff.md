# Issue/PR 阅读路线状态与 main 验收清单交接（reading-main-handoff）

- 日期：2026-11-27。任务 `reading-main-handoff`，分支 `docs/reading-main-handoff`，基线 `main@38fb8e0`。
- 目标与状态：文档任务，已完成编辑（见下）。把路线图和产品约定里仍写着“待合入 / 待贯通”的 Issue/PR 阅读条目改为实际已合入状态，并给负责人一份只用最新 main、不切分支的 Windows 验收清单。

## 背景与核对

目标是把 Issue/PR 阅读主线的状态写准确：仓库 Issues 列表与详情、仓库 PR 列表、PR Conversation、Files changed、全局 Issue/PR 标题入口、Home/Feed 与通知入口都已合入 main，只有 Releases 摘要续页是 #113 单独修复。核对用 `gh pr list --state merged` 与本地 `git log`：

| PR | 内容 | merge commit |
| --- | --- | --- |
| #106 | Issue 详情视图与路由 | `951da32` |
| #108 | 仓库 Issues 列表、筛选与续页 | `15b3a6d` |
| #109 | 只读 PR Conversation | `3ca98b0` |
| #110 | 全局 PR 列表标题打开 Conversation | `d0442b6` |
| #111 | 只读 Files changed Tab | `7e4ce73` |
| #112 | 仓库 Pull requests Tab 列表 | `fdcfd21` |
| #105 | 通知可识别目标跳转 | `434e0b5` |
| #107 | Home/Feed 工作条目打开详情 | `f12a9e2` |
| #113 | Releases 摘要规范续页读取（不新增页面） | `38fb8e0` |

源码位置也已逐条确认：仓库 Issues/PR 列表在 `src/ui/repository/{issues,pulls}`（`RepositoryTab::Issues/PullRequests`），详情在 `src/app/pages/{issue_detail,pull_request}.rs`（`Route::Issue` / `Route::PullRequest`），Files changed 由 `PullRequestPage` 的 `pr-tab-files-changed` 首次点击懒加载，全局列表标题接线在 `src/app/pages/work_items.rs`，通知在 `src/app/pages/notifications.rs`。

## 改动与决定

- `docs/roadmap.md`：
  - 顶部状态句改为“阅读主线已全部合入 main”，并注明 #113 只修 Releases 摘要续页、不新增 Releases 页面。
  - 基线由 `main@f12a9e2`（2026-11-26）改为 `main@38fb8e0`（2026-11-27）。
  - 证据表新增 “Issue/PR 阅读” 行；下一步依赖表第 2、3 行由“待交付/待合入”改为已合入；H2/H3 行不再写“全局 PR 标题跟踪 #110”。
  - “何时开始 Windows 日常试用”指向新清单；新增“Windows 日常试用：Issue/PR 阅读清单”一节。
- `docs/product.md`：“Issue/PR 列表与详情”一栏去掉“试用前还需贯通仓库 PR 列表、全局 PR 标题入口和 Files changed”，改为已接入并链接到路线图清单。

未改其他主题文档，也不声称 Settings/缓存并行任务或 Releases/Packages/Profile 功能已合并；发布评论、评审、合并等写操作仍标为未批准。

## 验证

纯文档改动：不触发 Rust 检查或构建。执行 `git diff --check` 通过；核对 PR 编号/提交通过 `gh pr list --state merged --json number,title,mergeCommit`（上文表）。路线图内新增锚点 `#windows-日常试用issuepr-阅读清单` 与标题一致，README/产品约定链接均为既有链接。未运行 Cargo 命令（无源码改动，且工作流要求纯文档不改源码）。

## 后续

- 负责人按 `docs/roadmap.md` 的清单在真实 Windows 窗口做 Issue/PR 阅读验收；无相关通知/Feed 事件时按清单中的回退执行并标注未覆盖。
- 第 4 步（发布评论、创建 Issue、评审/合并等试用必需项）仍未定稿，不因此清单宣告整套日常试用开始。
- Files changed 的 Commits、Checks 与行内评审线程仍另定；PR 列表暂无持久缓存（Issues Tab 已有），属后续独立工作。
- 已知但不在本任务范围内的陈旧表述：`README.md` 的页面内容段落仍写着“Read more 打开 Githux 内部 PR 标签（详情内容按当前约定留空）”和“通知、全局 Issues/PRs 等未接入的页面内容继续留空”，与 main 已接线的 PR Conversation、Notifications 和全局 Issues/PRs 不符。本任务只拥有 `docs/roadmap.md`、`docs/product.md` 与 devlog，README 的该段落需要单独文档任务或由负责人授权后修正，这里只记录不擅自改。
