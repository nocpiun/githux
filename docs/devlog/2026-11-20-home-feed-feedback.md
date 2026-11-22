# 开发日志：Home/Feed 视觉层级与交互反馈组合（home-feed-feedback）

- 日期：2026-11-20
- 分支：`feat/home-feed-feedback`（基线 `10a663a`，Geek Neutral 主题）
- 目标：将负责人从 UI 赛马 06 与 10 中选中的视觉层级与交互反馈改动组合为正式 PR，并包含 `sidebar-sash` 半圆端帽补充改动。

## 改动内容

1. **共享交互与表面层级 helper（来自 10）**
   - 新增 [`src/ui/interaction.rs`](../../src/ui/interaction.rs)：统一 `hover_fill`（`colors.hover`）、`pressed_fill`（`colors.active`）、`focus_ring`（1px `colors.focus`）与 `DISABLED_OPACITY`（`0.45`）。
   - 新增 [`src/ui/surface.rs`](../../src/ui/surface.rs)：定义 `window`（`bg`）、`page`（`surface`）、`raised`（`overlay`）、`well`（`sunken`）。
   - 在 [`src/ui/mod.rs`](../../src/ui/mod.rs) 及 [`src/bin_support/home_modules.rs`](../../src/bin_support/home_modules.rs)、[`src/bin_support/work_items_modules.rs`](../../src/bin_support/work_items_modules.rs)、[`src/bin_support/repository_modules.rs`](../../src/bin_support/repository_modules.rs) 同步暴露模块。

2. **Home / Feed 视觉层级与排版对齐（组合 06 + 10）**
   - [`src/ui/home/mod.rs`](../../src/ui/home/mod.rs)：页面根背景使用 `surface::page(&colors)`。
   - [`src/ui/home/rows.rs`](../../src/ui/home/rows.rs)：
     - Feed 卡片未选中态使用 `surface::raised(&colors)`，内嵌详情区使用 `surface::well(&colors)`，选中卡片保留 `colors.active` 并使用 `colors.border_strong` 边框（06）。
     - `RepositoryStarred` 事件使用 `IconName::Star` + `colors.warning`，其他活动保留原图标与颜色（06）。
     - 仓库描述（`pl_5`）、PR 摘要（`pl_6`）与“Read more”（外层 `div().pl_6().flex()`）分别与各自文字左沿对齐，不扩大“Read more”热区并沿用主题 `colors.accent` 原色。

3. **共享工作条目行（组合 06 + 10）**
   - [`src/ui/work_item_row.rs`](../../src/ui/work_item_row.rs)、[`src/ui/home/rows.rs`](../../src/ui/home/rows.rs)、[`src/ui/global_work_items/rows.rs`](../../src/ui/global_work_items/rows.rs)、[`src/ui/work_item_row_tests.rs`](../../src/ui/work_item_row_tests.rs)：
     - 元信息使用 `colors.fg_muted`，相对时间使用 `colors.fg_subtle`。
     - 未选中行启用 `interaction::hover_fill`（选中态保持 `colors.active` 不被 hover 覆盖）；有 `on_row_click` 的全局列表行启用 `interaction::pressed_fill`。
     - 新增 `show_selected_bar: bool`，仅 Home 开启选中态左侧 2.5px `colors.accent` 窄强调条（上下各 10px 留白），全局列表保持无左竖条。

4. **外壳与组件交互反馈（来自 10，严格限定范围）**
   - [`src/ui/sidebar.rs`](../../src/ui/sidebar.rs)：导航行、仓库行、账户行、通知铃铛、快捷按钮与 More 按钮统一使用 `interaction::focus_ring`、`hover_fill`、`pressed_fill` 与 `DISABLED_OPACITY`；已选中项保持 `colors.active` 不被 hover 覆盖，禁用项不触发 hover/press/focus。另按补充指令在 `sidebar-sash` 高亮子 `div` 的 `.w(px(SIDEBAR_SASH_WIDTH))` 后添加 `.rounded(px(SIDEBAR_SASH_WIDTH / 2.0))`。
   - [`src/ui/tabs.rs`](../../src/ui/tabs.rs)：非活动标签使用 `hover_fill` + `pressed_fill`，键盘焦点改用 `interaction::focus_ring`（保留透明 1px 边框，内容不跳动、拖动与激活切换不变）。
   - [`src/ui/repository/view.rs`](../../src/ui/repository/view.rs)：仅更新 `tree_row` 选中底色（`colors.active`）、未选中 hover/pressed 与键盘游标 1px `colors.focus` 轮廓；保留原有圆角及仓库页面背景。

## 验证与交付边界

- 通过 `node scripts/verify.mjs` 运行：
  - `cargo fmt --all -- --check`
  - `cargo clippy --locked --all-targets -- -D warnings`
  - `cargo test --locked --bins --tests`
  - `cargo test --locked --example global_work_items ui::global_work_items`
  - `cargo build --locked --example home` / `--example repository` / `--bin githux`
- 通过 `./scripts/prepare-windows-runtime.ps1 -Destination target/debug` 在 `target/debug/` 准备正式 Windows App SDK 运行时供负责人启动 `target/debug/githux.exe` 验收。
- **边界（L002）**：GPUI 模拟测试不能替代真实 Windows 窗口上的鼠标悬停/按下、Tab/Shift+Tab 焦点轮廓与视觉对比度人工验收；PR 标记 `pending` 等待负责人确认。

## 2026-11-20 负责人人工验收返工

1. **Feed 合并 PR 图标紫色**：[`src/ui/home/rows.rs`](../../src/ui/home/rows.rs) 新增 `feed_pr_icon_color`，仅对 `FeedActivityKind::PullRequestMerged` 使用 `colors.chart[4]`（Geek Neutral `#bc8cff`，浅色 `#8572bb`），其余活动保持 `colors.accent`。
2. **标签焦点遗留**：[`src/ui/tabs.rs`](../../src/ui/tabs.rs) 为标签左键按下补充 `window.prevent_default()`，避免鼠标点击/切换留下键盘焦点环；[`src/ui/mod.rs`](../../src/ui/mod.rs) 的 `open_route` 在切往新标签时若前标签 `strip_focus` 仍持有焦点则移交至新页面 `focus_page`，保留 `Tab`/`Shift+Tab` 键盘焦点可见性。
3. **仓库 Code 双栏标题带加深一级**：[`src/ui/repository/view.rs`](../../src/ui/repository/view.rs) 的共享 `code_header`（左 `Files`/commit，右文件图标/`README.md` 或当前文件名）统一设置 `.bg(theme.colors.bg)`，在深浅主题下较 `surface` 正文加深一级，保留字体、几何与 `border_b_1`。
4. **回归验证**：新增 `feed_pr_icon_color_uses_purple_only_for_merged_in_dark_and_light`、`clicking_a_tab_or_opening_a_link_from_a_focused_tab_leaves_no_stale_tab_focus` 与 `code_header_is_one_step_darker_than_page_in_dark_and_light`，并重跑全量 `fmt`、`clippy --all-targets`、`--bins --tests`、示例与 Windows 运行时准备。
