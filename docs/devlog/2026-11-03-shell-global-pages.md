# shell-global-pages 开发日志

任务：把已验收的 Notifications、全局 Issues/PR 组件正式挂载到 Githux Shell，取代全局空白入口，并为已在 Shell 接入的 Settings › Accounts 保留统一挂载模型。base `d217116`，分支 `feature/shell-global-pages`，交付 `pr`，milestone #1。

## 实现范围与架构决策

1. **页面挂载架构** (`src/app/pages/mod.rs`, `src/app/pages/{notifications,work_items}.rs`)：
   - `MountedPage` 扩充 `Notifications(Entity<NotificationsPage>)` 与 `WorkItems(Entity<WorkItemsPage>)`。
   - `NotificationsPage`：从 `AccountServices.repositories().notifications()` 获得控制器；`on_repository` 回调经 `RepoService::is_current` 校验捕获的授权上下文后，通过 `NavigationSink` 派发 `PageNavigation::Open(Route::Repository)`。无活动账户或服务不可用时显示确定性 unavailable 占位；`set_active` 转发到 `NotificationsView::set_active` 配合 tabs 生命周期停启时间刷新/动画。
   - `WorkItemsPage`：在构造前以 `repo_service().work_item_session()` 预检，按 `WorkItemKind::{Issue, PullRequest}` 构建 `WorkItemsView`；仓库链接同一复核后打开应用内 Repository；`set_active` 同步到视图。
   - `Settings`：经核查，main 分支上的正式 Shell 已经通过 `Shell::settings_accounts`（使用 `ely_settings::SettingsSection/SettingsRow` 与 `ui::accounts::accounts_section`）将生产 app-global `Accounts` 控制器挂在 `Route::Settings` 上，并有 30 项针对 `account-token-form`/`settings-accounts` 的回归测试。我们保持 Settings 现有实现及 `focus_accounts_form` 契约不变，不破坏已验收的账户测试。

2. **模块注册与双 Target 约束** (`src/ui/mod.rs`, `src/ui/notifications/mod.rs`, `src/ui/global_work_items/mod.rs`)：
   - 在 `src/ui/mod.rs` 声明 `pub mod notifications; pub mod global_work_items;`，使这两个组件正式进入产品模块树。
   - 保留组件内部的 `#![allow(dead_code)]`，并更新其注释，说明组件为产品二进制与独立开发 example 双 target 共享，example 专属 API（如 `set_now`、测试构造器等）在产品 target 属合法未调用。
   - `ui::global_work_items::tests` 改为使用相对 `#[path]` 引入 `work_items_fixture.rs`（与 `notifications/tests.rs` 保持一致），避免要求 crate 根存在 `ui::fixture`。
   - `src/ui/page.rs`：针对 `Notifications`、`Issues`、`PullRequests` 提供统一的不可用提示，PR 详情页（`Route::PullRequest`）仍按产品约定留空。

3. **测试覆盖** (`src/app/pages/tests.rs`)：
   - 扩充 `PageTransport`，支持 `/notifications`（返回未读 threads，模拟 DELETE 204 及 PUT 200 `ignored=true`）以及 `/search/issues`（辨别 `is:issue`/`is:pr` 查询与 Home 的 `involves:` 查询）。
   - 新增 `notifications_route_reads_routes_repositories_and_completes_done`：测试 Shell 打开 Notifications 路由、读取未读通知行、点击仓库链接进入应用内 RepositoryPage、返回 Notifications 标签并执行 Done（触发真实 DELETE 请求且从 inbox 移除）。
   - 新增 `issues_and_pull_requests_read_distinct_lists_and_reuse_their_tabs`：测试 Issues 与 Pull Requests 路由分别读取各自列表、状态隔离，重复导航同一资源复用已存在的 tab。

## 验证记录

- `node scripts/verify.mjs -- cargo check --locked --all-targets`：SUCCESS（无 error，无 warning）。
- `node scripts/verify.mjs -- cargo clippy --locked --all-targets -- -D warnings`：SUCCESS（无 error，无 warning）。
- `node scripts/verify.mjs -- cargo fmt --all -- --check`：SUCCESS。
- `git diff --check` & `git diff --cached --check`：Clean。
- `node scripts/verify.mjs -- cargo test --locked app::pages::tests::`：SUCCESS（16 passed；覆盖所有页面挂载）。
- `node scripts/verify.mjs -- cargo test --locked ui::tests::`：SUCCESS（60 passed）。
- `node scripts/verify.mjs -- cargo test --locked ui::sidebar_tests::`：SUCCESS（57 passed）。
- `node scripts/verify.mjs -- cargo test --locked app::accounts::tests::`：SUCCESS（30 passed）。
- `node scripts/verify.mjs -- cargo test --locked ui::notifications::tests::`：SUCCESS（8 passed）。
- `node scripts/verify.mjs -- cargo test --locked ui::global_work_items::tests::`：SUCCESS（13 passed）。

## 人工验收与后续约束

- 本任务属于 First Release（milestone #1）。
- 提交 PR 时添加 `pending` 标签，等待负责人进行正式 Shell 原生窗口的人工验收（验证真实窗口下 Notifications、Issues、Pull Requests 的导航、读写动作与样式表现）。
- 全程无裁决事项，`arbitration_ever_required` 保持 `false`。

## 2026-11-04 负责人视觉验收返工

负责人对 PR #71 的人工验收提出两点：Notifications 背景/标题/内外边距与其它正式页面不统一；全局 Issues/PR 与 Home 的条目必须是同一组件（Home 同步改用全局的彩色状态图标）。本轮在原分支/原 PR、原会话完成，保留 `pending`。

### 改动

1. **共享工作条目组件** `src/ui/work_item_row.rs`（新增，`ui::work_item_row`）：
   - 单一 `WORK_ITEM_ROW_HEIGHT = 72`，替代 Home 的 68 与全局的 72 两套行高；Home 的 `HOME_ROW_HEIGHT` 改为引用它，导出名与调用签名保持不变。
   - 统一标题（`TextSize::Base` + `MEDIUM` + GPUI 原生 `truncate()`）、图标槽竖直居中、`Radius::Md` 圆角、`colors.active` 选中底、`TextSize::Xs` 元信息、窄屏截断。
   - 元信息顺序统一为 `#number → repository → state → relative time`，编号仍在前（负责人此前要求），编号用主题 `mono_family`（Google Sans Code）。
   - 图标颜色按 `Tone::{Open, Closed, Merged, Unknown}` 映射到 `success / fg_subtle / accent / warning`，Home 因此从原来的固定 `fg_muted` 图标改为彩色状态图标。
   - 组件只接收展示值与回调（`RowAction` / `TitleAction`），两端继续各自持有数据模型、账户上下文与 typed navigation。
2. **Home 消费共享行** `src/ui/home/rows.rs`：`home_row` 改为构造共享 `Row`（owner 链接、repository 链接、标题激活仍走原 `HomeAction` / `view.select` + `view.activate`），保留 `home-item-row-*`、`home-title-*`、`home-icon-*`、`home-metadata-*`、`home-0-owner`、`home-0-repository`、`home-time-*` 这些既有 selector。Feed 卡片、数据获取与缓存语义未改。
3. **全局列表消费共享行** `src/ui/global_work_items/rows.rs`：`ROW_HEIGHT` 引用共享常量；`work-item-*`、`work-title-*`、`work-metadata-*`、`work-number-*`、`work-repo-*`、`work-updated-*` selector 与仓库回调、键盘选择语义保持不变；新增 `work-*-state` selector。标题仍无详情动作。
4. **页面背景/标题/内边距统一**：
   - 新增 `src/ui/page_layout.rs` 作为四个正式页面共用的内容几何（`PAGE_GUTTER=16`、`PAGE_TOP=16`、`PAGE_BOTTOM=8`、`PAGE_HEADER_HEIGHT=40`），与 Home 现有的 `px_4 / pt_4 / pb_2` 和 40px 列头一致。
   - Notifications：根背景由 `colors.bg` 改为 `colors.surface`（与 Home/Settings/全局列表一致），移除 header 自带的 `px_3/py_2`，改由页面 body 统一提供 gutter/top/bottom；标题补 `SEMIBOLD`、header 高 40；已读行底色由 `colors.bg` 改为 `colors.surface`，只保留未读行的 `info_subtle` 底色与圆点/字重差别。通知行排版、右下角动作与相对时间未改。
   - 全局 Issues/PR：根 `p_4` 改为统一 gutter/top/bottom，header 固定 40 高；筛选行与列表随同一 gutter 对齐。
5. **fixture/example 最小接线**：`home_modules.rs`、`work_items_modules.rs`、`notifications_modules.rs` 增加 `page_layout` / `work_item_row`（及 `tooltip`）的 `#[path]` 声明，使既有 preview 与组件测试继续编译同一份源文件。

### 验证

- `cargo check --locked --all-targets`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo fmt --all -- --check`：SUCCESS。
- `cargo test --locked --bin githux`：SUCCESS，1151 passed。
- 定向：`ui::home::tests::`（含新增 `home_work_item_row_is_the_shared_row_presentation`）、`ui::global_work_items::tests::`、`ui::notifications::tests::`、`app::pages::tests::` 全部通过。

### 未覆盖

- 原生 Windows 窗口的四页对照（宽/窄、图标与标题对齐、长标题/长仓库名、四种状态色彩）仍需负责人复验；GPUI 测试平台的 bounds 断言不替代原生视觉确认（L002/L017）。PR 保持 `pending`，由负责人重新验收后移除。

### 审查反馈修复（Codex P2）

Codex 对 `4e51b3e` 指出：首次通知请求未返回前切走页面会取消请求并清除 `busy`，但不设 `loaded`；返回保留 tab 时此前只调用 `set_active(true)`，既不重发请求也没有未加载态恢复路径，收件箱会一直空白直到手动 Refresh。

修复：`NotificationsView::set_active(true)` 在 `resume_needed()`（`!loaded && busy.is_none() && error.is_none()`）时重启读取。已加载的空收件箱、进行中的请求和已有错误的快照都不重发，因此普通标签切换仍复用快照。回归 `reactivating_an_interrupted_cold_inbox_resumes_the_first_read` 断言恢复规则（未加载→First，已加载→不重读）与恢复实际触发了读取；`reactivating_a_loaded_inbox_does_not_re_read` 断言已加载快照切 tab 不产生新请求。

## 2026-11-05 负责人第二次视觉验收返工（PR #71 仍 pending）

负责人对 `5d78055` 的三张截图仍不通过，两处问题都被定位为真实几何缺陷，不是断言措辞：

### 1. 共享工作条目：#编号与 owner/repo 不在同一条线上、选中底色下重上轻

**根因 A（垂直分布）**：`work_item_row` 固定 `h(72)` + `px_2/py_2` + `flex_col/gap_1`，但没有竖直分布策略。两行（标题行 + 元信息行）实际只占约 43px，于是 `72 − 16(padding) − 43 ≈ 13px` 的余量全部落在元信息下方，选中圆角底色看起来"上面贴顶、下面一大块空"。修复：内容容器加 `justify_center`，余量均分到标题上方与元信息下方。

**根因 B（行盒不一致）**：GPUI 默认行高是 **按 run 的字体度量**（phi × 字号）计算的；`#number` 用 `mono_family`、owner/repo 用 UI 字体，两者各自测出不同行盒，`items_center` 只是把不等的盒子互相对中，绘制出的字形仍然错位。修复：标题行与元信息行都设显式行高 `rems(字号 × 1.5)`（用 rems 而不是 px，跟随窗口实际 rem/字号缩放；`App` 上没有 `rem_size()`，只有 `Window` 有）。所有元信息单元格现在共享同一个行盒，居中后才真正落在同一条线上。旧注释里提到的"相对 1px 上移"在代码里根本不存在，已删掉换成实测依据。

**验证**：
- 新增 `home_work_item_row_balances_its_fill_and_shares_one_metadata_line`（宽 1180 / 窄 640）：断言标题行上方余量与元信息下方余量差 ≤1px、`#number`/owner/repository/relative time 的中心 y 与元信息行中心差 ≤1px、`#number` 仍在 owner 之前。
- **反向证伪**：临时把 `justify_center` 去掉、行高改回 phi，该测试立即失败（above=8px, below=21px），证明断言确实咬住原缺陷而不是恒真；随后还原。
- 新增 `work_item_rows_balance_their_fill_and_share_one_metadata_line`（全局 Issues 列表，宽/窄）：同样断言余量平衡、`#number` 与 repo 同线，并断言行实际高度等于 `ListState` 预留的 `WORK_ITEM_ROW_HEIGHT`（虚拟列表估算与真实高度不脱节）。
- Home 行新增 `home-{index}-number` selector（原来 `number: None`，编号没有可断言的盒子），其余 selector 与回调签名未动。

### 2. Notifications 标题仍比 Home 靠左

**根因**：上一轮只把 `PAGE_GUTTER=16` 对齐了 Home 的 `px_4`，但 Home 的列头在 gutter 内**又**有 `px_2`（8px）内缩——Home 标题起点是 gutter+8，行起点才是 gutter。Notifications 的 header 只有 gutter，标题因此更靠左。

**修复**：`page_layout` 新增 `PAGE_HEADER_INSET = 8`（与 Home `home-header` 的 `px_2` 对应），Notifications 与全局 Issues/PR 的 **header** 各自加这层内缩；列表与空状态保持只占 gutter，不跟着标题右移（否则内容边缘会越过 Home）。空收件箱与有条目共用同一 header/列表容器，不新增占位文案。`home/mod.rs` 未改。

**验证**：新增 `notifications_heading_and_content_edges_match_home_in_empty_and_populated_states`，Empty 与 Mixed 两种 fixture、960/360 两种宽度下分别断言：列表左右边缘 = 页面 gutter；标题文本起点 = gutter + inset。注意 `debug_bounds` 返回边框盒，header 的 padding 体现在**标题内容盒**上，所以断言对象是标题而不是 header 盒（第一版断言写错对象已当场修正）。

### 本轮检查

- `cargo check --locked --all-targets`：SUCCESS。
- `cargo clippy --locked --all-targets -- -D warnings`：SUCCESS。
- `cargo fmt --all -- --check`：SUCCESS；`git diff --check` clean。
- `cargo test --locked --bin githux`：SUCCESS，1156 passed（含上述 3 个新几何回归）。
- 定向：home/global_work_items/notifications 全部 66 个用例通过。

### 未覆盖 / 边界

- 仍是 GPUI 测试平台的 bounds 证据，**不是原生窗口截图**。本轮环境未做真实 Ely 窗口复验；同一 Shell 几何下 Home/Issues/Pull Requests/Notifications 的宽窄对照、选中/未选中、长名称截断与四种状态色彩仍需负责人在原生窗口确认（L002/L017）。PR 保持 `pending`，不自行合并。

## 2026-11-05 Codex 二次审查修复（P2：无动作的标题仍是死链接）

Codex 对 `5328553` 指出（`work_item_row.rs` line 199）：全局 Issues/PR 行传入 `on_title_click: None`，但共享标题组件**无条件**给标题加 `Role::Link`、`cursor_pointer` 和 `tab_index(0)`——对键盘与屏幕阅读器用户而言这是一个可聚焦、可点击但按下毫无反应的死链接。

修复：只有存在 `on_title_click` 时才附加 link role / 可聚焦 / 指针 / hover 下划线 / `on_click`；无动作的标题保留 `flex_1/min_w_0/MEDIUM/truncate/tooltip` 的几何与外观，但不进 Tab 序、不承诺动作。Home 标题有动作，仍走原来的链接分支（Home 自己的测试覆盖）；全局列表的键盘入口继续只有仓库链接，不做外部跳转或未支持的详情导航。

回归：`unconnected_title_is_not_a_tab_stop` 聚焦列表后只走一步 Tab 并 Enter，断言落点是仓库链接而非死标题。已反向证伪——临时把无动作标题重新设为 `tab_index(0)` 后该测试立即失败（actions=0），证明断言咬住的是 Tab 序而非恒真。

验证：`cargo check --locked --all-targets`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo fmt --all -- --check`、`cargo test --locked --bin githux`（1157 passed）全部 SUCCESS；`git diff --check` clean。PR 保持 `pending`，等待负责人原生窗口视觉验收。
