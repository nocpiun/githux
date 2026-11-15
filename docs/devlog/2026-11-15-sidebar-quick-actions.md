# sidebar-quick-actions 开发日志

任务：用户问题 8 —— 账号状态 emoji 换成小型 Notifications 铃铛按钮，旧的 Notifications 快捷入口换成明暗主题切换。分支 `fix/sidebar-quick-actions`，First Release #1；交付 PR，准备验收时加 `pending`，不合并。

## 实现

- 删除 `AccountDisplay.status_emoji/status_label` 与占位状态按钮（原 `account-status`）。`src/ui/sidebar.rs` 的账号行在展开态右侧换成 `account-notifications` 铃铛按钮：沿用原 24px 盒（`STATUS_BUTTON_SIZE`，注释改为账号行动作按钮），透明边框 + hover 背景的 ghost 风格、`aria_label`/tooltip 均为 “Notifications”、Tab 焦点 + Enter/Space、点击 `navigate(Route::Notifications)` 并 `stop_propagation`，因此不读成 Profile 点击。收起态铃铛隐藏（与旧状态按钮一致），但 Notifications 在收起态 More 菜单中仍然可达，不丢失唯一入口。
- 展开态底部四入口的 Notifications 换成 `theme-toggle`：`Shell::toggle_theme` 读取 `cx.theme().is_dark()` 后调 `Theme::set_mode(Mode::Light|Dark)`，图标/标签随当前模式切换（Sun/Moon，tooltip 为 “Light theme”/“Dark theme”），不导航、不改变 route、不引入持久化。模式切换只换 Ely 主题 global 的 mode 并重绘窗口；`Theme::update` 里的 `font_family`/`mono_family`（Noto Sans SC / Google Sans Code）独立于 `Palette`，切换后保持。启动默认 Dark 仍在 `src/app/mod.rs`，未改。
- 首轮 Codex P2 修复：主题切换时同步原生 Windows 标题栏。`platform::caption::set_theme` 在 Windows 上把当前 Ely 主题的 `colors.bg`/`colors.fg` 与明暗偏好写回已安装的 caption 适配器（Tall 的 `AppWindowTitleBar` 或 legacy DWM），避免标题栏按钮停留在启动时的 dark 调色板。Tall 适配器新增 `set_theme` 重设 `SetPreferredTheme` 与各 background/foreground 色；legacy 适配器新增 `set_theme` 写 `DWMWA_CAPTION_COLOR`/`DWMWA_TEXT_COLOR` 并翻转 `DWMWA_USE_IMMERSIVE_DARK_MODE`，无适配器安装时均为 no-op。
- 收起态 More 菜单增加 `Toggle theme` 项（图标 Moon），作为启用项参与方向键高亮与 Enter/Space；主题项不标 route Check，`New` 仍 disabled。`move_sidebar_menu_cursor`/`pick_sidebar_menu_cursor` 的启用项从 3 扩到 4，`Shell::toggle_theme` 在菜单执行路径与展开按钮路径共用。
- 调用方 `src/ui/mod.rs`：`account_row` 新增 `notifications: Run` 参数；`quarter_row` 首个槽位改为主题切换。

## 验证

- 复用既有侧栏 fixture/harness（`src/ui/sidebar_tests.rs` 的 `shell()`），把 `account_identity_opens_profile_while_status_and_blank_do_not` 改为 `account_identity_opens_profile_while_bell_and_blank_do_not`：展开态点铃铛导航 Notifications、点身份导航 Profile、身份与铃铛之间的空白不打开 Profile、收起态铃铛隐藏而身份仍开 Profile。`src/ui/tests.rs` 的 `navigation_rows_and_quarter_entries_route_and_keep_selection` 改为点 `quarter-theme-toggle` 断言不导航、点 `account-notifications` 断言导航 Notifications。
- `src/app/accounts/tests.rs` 的已登录前断言 `account-status` 隐藏改为 `account-notifications` 隐藏（未登录/恢复中无身份即无铃铛）。
- 回归设计重点：主题切换的按钮点击不改变 route 且字体 family 在切换前后保持不变（待补充到最终检查，见下）。

## 检查与证据

本地 Cargo 检查、Clippy 与相关测试在 `node scripts/verify.mjs -- cargo ...` 下执行：

- `a3b84e07`（首轮 `cargo check --locked --bin githux`）：失败，`E0382` 账号块 `this` 移动后借用 + 未使用 `Mode` 导入。已修。
- `02747706`：`cargo check --locked --bin githux`，SUCCESS。
- `3f783357` / `a51cc6d6`：`cargo clippy --locked --bin githux -- -D warnings`，SUCCESS。
- `d1147969`：`cargo test --locked --bin githux -- sidebar`，61 passed（首版）。
- `278134c7`：`cargo test --locked --bin githux -- sidebar`，62 passed（含 collapsed_menu_keeps_notifications_and_toggles_theme_in_place）。
- `873766fc`：`cargo test --locked --bin githux -- navigation_rows_and_quarter_entries theme_toggle account_identity`，5 passed。
- `3493ece5`：`cargo test --locked --bin githux -- pat_submit_is_masked_cleared`（登录前 `account-notifications` 隐藏断言），1 passed。
- `31060709`：`cargo test --locked --bin githux` 全量，1402 passed，0 failed，3 ignored。
- 首轮 Codex P2 修复后：`94b34271` `cargo check --locked --bin githux` SUCCESS；`7b1bf404` `cargo clippy --locked --bin githux -- -D warnings` SUCCESS；`de0edeb9` `cargo test --locked --bin githux` 全量 1402 passed，0 failed，3 ignored。

远端 CI/Codex 与 head 见 PR 交付记录。

## 边界

- 主题切换为 session-wide（进程内 Ely 全局），不做磁盘持久化，符合“不新增持久化/设置架构”。
- 收起态 Notifications 与主题切换都保留在 More 菜单；展开态主题切换无 route 选中态。
- 真实 Windows 原生窗口的视觉与滚动手感仍待负责人验收；GPUI 测试平台不经过 Windows 原生输入分发，按 L002 记录验证边界。
