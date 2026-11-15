# Repository refresh continuity

- 日期：2026-11-15；任务：`repository-refresh-continuity`；分支：`fix/repository-refresh-continuity`；起点：`812e5fc6b6389ea7c67c60f17a8c1b8ae53cfc15`。
- 目标：保留缓存首屏速度，在同账户/仓库/ref 的在线重验期间保持可读内容，按固定提交协调发布替换；账户、授权与 ref 的隔离边界不变。

## 起点与最小复现

实际读取路径为 `RepositoryPage::activate` 同时启动 `Job::Snapshot` 与在线 `Job::Repository`。快照作业只读本地；在线作业依次读取摘要 → 分支 → revision，再分别读取根树、README 定位与正文。并非“先缓存结束再开始网络”。

`apply_revision` 已对同分支、同 commit 保留树/reader，但不同 commit 的分支立即替换 `BranchState`、将树置空、清掉 reader，然后才启动后续读取，导致等待新内容期间绘制空树与 Loading。手动 `refresh_repository` 更早清空摘要、overview、树、reader 和授权槽，随后又从缓存重建。`app::pages::repository_page`/`Shell::sync_pages` 没有因普通缓存/网络结果重绑 host；host 只在账户授权上下文变化时重建。

最小回归 `continuity_changed_commit_keeps_cached_content_until_replacement_is_ready` 复用真实缓存服务和共享仓库 fixture：在线预热、重开服务、挂起网络并确认缓存树与 README 已渲染；将同一 `main` 指向 fixture 的第二个 commit，只挂起该 commit 的根树，检查等待中的实际页面与 reader 实体；再放行，逐次 drain/render 检查树和 reader 没有空白中间态。

## 基线失败证据

Runner `6e66d345-943b-4c2d-ad73-aa090d7993a7`，命令 `node scripts/verify.mjs -- cargo test --locked --test repository continuity_changed_commit -- --nocapture`：1 个回归失败、1013 个过滤。失败断言为新根树仍挂起时 reader 应为 `Showing`，实际为 `Loading`；测试运行 0.91s。构建槽等待 212ms；首次依赖/目标构建与运行合计 846.8s。此时生产源码仍为基线。

## 实现

- `src/ui/repository/refresh.rs` 通过已有 `CachedRepositorySession`/`RepositorySession`、同一取消令牌与固定 `RepoSnapshot` 在后台准备根树、已展开目录、README 定位和当前正文。只加载保存的展开目录与选中路径所需祖先，不递归下载仓库。
- `apply_revision` 遇到同 ref 新 commit 时保留显示中的 `BranchState`/reader，将新目标存为 `replacement`；完成后 `Body::Replacement` 在一个前台更新中发布新树、README、正文或真实终态。所有中间读取不修改已显示 commit 或把旧正文标成新版本。
- 替换仍核对原页面 generation、授权上下文、carried token、branch request、候选 commit 与 selection request。用户准备期间改变选择时重新准备最新路径；明确切 ref、账户、授权、取消会退休候选。失败保留旧快照与来源，使用已有 compact warning/Retry 样式提示。
- 手动 Refresh 推进读取身份、取消旧工作，但保留有效摘要、树、reader 和 overview。保留 reader 后重新绑定回调身份，不替换实体、滚动或选区。overview 保留显示 binding，单独用各端点请求代次取消旧读取。无需改变共享 overview API，也未修改并行任务负责的 `repository_overview/summary.rs` 或 `repository/view.rs::tab_strip`。
- 刷新不再丢弃仍在显示的授权令牌；撤权期间旧资源不能被新请求重新授权。重新激活、重绑和数字仓库身份变化仍清空旧数据。
- 渲染只增加同作用域的紧凑刷新错误入口，没有修改主题、布局间距或已有链接命中区域，因而无需主题文档变更。

## 运行与验收

正式应用启动：`node scripts/verify.mjs -- cargo run --locked --bin githux`。负责人可打开已缓存仓库，检查缓存首屏、自动重验、手动 Refresh、文件/分支切换与当前滚动/选区；正式应用继续使用其已有账户与缓存路径。

确定性离线复现/回归：`node scripts/verify.mjs -- cargo test --locked --test repository continuity_ -- --nocapture`。用例在可控传输层门闩下断言中间渲染状态，不以最后出现内容证明不存在空白帧。

## 验证记录

- 首轮修复边界检查：`node scripts/verify.mjs -- cargo test --locked --test repository repository_modules::repository:: -- --nocapture`，runner `a0e6edc0-616b-498b-8d28-892983a3f657`，111 通过、0 失败；包含原先失败的空白回归与既有 repository/navigation/extras/star/offline 检查。构建槽等待 473.4s；实际编译/运行 190.0s（测试本身 6.31s）。
- 首轮通过后自查补齐两项边界，因此最终检查重新运行：数字仓库身份改变时推进整页 generation，拒绝前一个仓库已排队的分支列表；替换 README 路径改变时把新祖先目录纳入展开意图，确保实际选中行可见。各自新增确定性回归。
- 中途 runner `933bce53-e40c-40b7-be91-dcca3439345b` 在等待构建槽时取消，以集中剩余场景；该次没有启动 Cargo，不视为检查结果。
- 最终仓库边界：runner `8db4c166-ed1c-4778-82b5-9339d7f5b65c`，113 通过、0 失败；槽等待 183ms，实际编译/运行 138.6s，测试本身 6.44s。包括同 commit 的 reader/overview 实体、滚动与选区保留，新 commit 准备期间逐次实际渲染、固定 commit 一致性，正文失败、最新选择、删除路径、README 移动、数字仓库身份、账户/ref 切换与刷新期间撤权。
- `cargo clippy --locked --bin githux --test repository -- -D warnings`：runner `5f8b36f5-3182-4d98-afc5-0799c8aba3cd` 通过；槽等待 191ms，实际执行 194.0s。
- `cargo build --locked --bin githux`：runner `1dc4697f-4b2f-45c2-b776-19339440bdc0` 通过；槽等待 155.4s，实际执行 225.3s。
- `cargo fmt --all -- --check`：runner `02178731-f0db-4742-a9a2-c69f3784a8d1` 通过；不占构建槽，实际执行 24.4s。`git diff --check` 与 `git diff --cached --check` 通过。
- 所有 Cargo 经共享 runner，重检查串行持有构建 mutex。最终通过后未重复运行未改变的测试；远端最新 head 的 CI/Codex 证据留在 PR 与任务交付状态。

## 可复用教训

缓存先行的测试需要检查“在线 revision 已返回，但新树/文档未准备好”的中间绘制状态。仅断言网络放行前有缓存、全部完成后有最新内容，会漏掉两个断言之间的清空/重建。保留显示资源与更新读取代次是两个操作；保留旧画面时也必须保留其 carried authorization，而不是用新请求的令牌替换它。

## 验证边界

使用临时 SQLite/CacheStore、合成账户与可控 transport 的 GPUI 组合测试，不依赖 GitHub 实网或真实 PAT。原生视觉验收由负责人完成，PR 保持 `pending`，不合并。
