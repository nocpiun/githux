# 开发日志：ui-profile-surface 页面背景与链接命中区修复

- 日期：2026-11-10；任务 ID：`ui-profile-surface`；分支：`fix/ui-profile-surface`；执行者：worker。
- 目标与状态：已完成代码实现与测试，待提交 PR 与等待评审。

## 背景与问题核查
1. **用户 Issue 4**：Profile 背景色与 Workspace Tab Bar / 页面表面不一致，头像/头部上方的间隙带露出 `colors.bg`，而 Island 和 Tab Bar 底色为 `colors.surface`（与 Home 一致）。
   - 追踪：`src/ui/mod.rs` 中 Island 容器使用 `colors.surface`，Tab 栏使用 `transparent_black()` 贴在 island surface 之上。Home 视图在 `src/ui/home/mod.rs` 中使用 `colors.surface`。Repository 在 `src/ui/repository/view.rs` 中使用 `colors.surface`。Notifications 在 `src/ui/notifications/mod.rs` 中使用 `colors.surface`。而 `src/ui/profile/render.rs` 之前错误地使用了 `colors.bg`，导致 Profile 页面包括头像上方出现对比色条。
   - 修复：将 `src/ui/profile/render.rs` 中的根背景色及 section 错误面板背景色从 `cx.theme().colors.bg` 改为 `cx.theme().colors.surface`，使 Profile 在浅色/深色主题及各种状态下与 Tab 栏无缝融合。
2. **相关 Issue 2（文本链接命中区域）**：
   - 审计：`src/ui/profile/header.rs` 中的 website link 原使用了 `max_w_full()` 且位于 flex-col 容器中，容易在特定测量下延伸至整行空白区域，导致点击空白也会触发链接。
   - 修复：保留 `max_w_full()` 与 `min_w_0()`，文本标签添加 `min_w_0().truncate()`，添加 `Role::Link`，并保持 `gap_1p5()` 和图标 + 文本包裹，确保 hit area 仅覆盖文本与图标本身，并在超长 URL/超窄容器下自然截断不溢出。增加回归测试验证点击链接右侧空白处不触发导航动作，以及超长链接截断在 header 容器内。
3. **相关 Issue 7（Profile 裁剪检查）**：
   - 审计：检查 Profile 首尾元素在滚动容器中的可见性。顶部头像及资料在初始状态完全可见无遮挡，底部贡献日历在滚动到底部时最末周与单元格完整可见（`bottom <= 560.0`），无异常裁剪或超出边界。

## 改动与决定
- `src/ui/profile/render.rs`：
  - `ProfileView::render` 根节点背景从 `theme.colors.bg` 改为 `theme.colors.surface`。
  - `state_panel_with_retry` 从 `theme.colors.bg` 改为 `theme.colors.surface`。
- `src/ui/profile/header.rs`：
  - Website link 增加 `role(Role::Link)`、`flex_none()`，移除 `max_w_full()`，确保命中区仅围绕文字和图标。
- `src/ui/profile/tests.rs`：
  - 增加对 Website 链接命中区域和点击空白区域不触发动作的断言。
  - 增加对滚动触底时最末贡献单元格完整可见无裁剪的断言。
  - 现有全部 27 项 Profile 单元测试及 284 项集成测试全部通过。
- `docs/profile-view.md`：
  - 记录 2026-11-10 页面背景表面与链接命中区修复。

## 验证
- `node scripts/verify.mjs -- cargo check --locked --all-targets`：通过。
- `node scripts/verify.mjs -- cargo test --locked --test profile`：284 项通过。
- `node scripts/verify.mjs -- cargo test --locked --bin githux ui::profile`：27 项通过。
