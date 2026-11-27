# 2026-11-27：正式 Shell 挂载 SettingsView（settings-accounts-shell）

## 交付内容

- 在 `src/ui/mod.rs` 注册 `pub(crate) mod settings` 并移除 `src/ui/settings/mod.rs` 的模块级 `#![allow(dead_code)]`。
- 在 `src/app/pages/mod.rs` 新增 `MountedPage::Settings(Entity<SettingsView>)`，并在 `src/ui/shell_refresh.rs` 与 `src/ui/page.rs` 完成对应匹配；`suspend` 调用 `SettingsView::cancel_interactions()`。
- 在 `src/ui/mod.rs` 用 `MountedPage::Settings` 替代过渡 `settings_accounts` 表单：
  - `Shell::ensure_active_page` 在存在 app 级 `Accounts` 控制器时挂载 `SettingsView + SettingsBinding`（无控制器时保持不可用回退）。
  - `Shell::sync_pages` 在 `AuthContext` 变化时保留 `MountedPage::Settings`，确保当前账户 Replace PAT / Add 完成后 `SettingsBinding::settle` 仍能收起表单并恢复焦点。
  - `Shell::island` 在 `SettingsView` 挂载时关闭外层 `outer_scroll` / `page_bar`，避免与 `SettingsView` 自带的 `settings-page-scroll` / `settings-page-bar` 叠出双滚动条。
  - `Shell::focus_accounts_form` 改为在已挂载的 `SettingsBinding` 上打开 `FormMode::Add` 并聚焦 masked PAT 输入框；若 `Add` 表单已打开，保留现有输入并重新聚焦输入框。
  - 将 `Shell::new` 中 `sidebar_keys` 捕获的 `cx.entity()` 改为 `cx.weak_entity()`，修复新增 `SettingsBinding -> Accounts` 强引用后暴露出的 `Shell -> sidebar_keys -> Shell` 析构环。
- 在 `src/ui/settings/binding.rs` 的 `SettingsBinding::settle` 中改用 `window.defer(cx, ...)` 恢复 `origin` 焦点，确保在 `Shell` 根 `FocusScope` 内关闭表单后焦点可靠落回触发按钮。
- 更新 `src/app/accounts/tests.rs` 中依赖过渡表单选择器的用例，并新增正式 `Shell -> Settings` 生命周期、切页重开、添加/切换/失败保留/重验/替换/脱敏/防重入/焦点恢复回归；更新 `docs/settings.md`。

## 验证证据

- `node scripts/verify.mjs --no-lock -- cargo fmt --all`
- `node scripts/verify.mjs -- cargo fmt --all -- --check`
- `node scripts/verify.mjs -- cargo clippy --locked --all-targets -- -D warnings`
- `node scripts/verify.mjs -- cargo test --locked --bins --tests`
- `node scripts/verify.mjs -- cargo test --locked --example settings settings::`
- `git diff --check`

## 未覆盖与后续（integrated main）

- 以上为 GPUI 测试平台与离线 `AutoReposTransport` / `MockTransport` / `InMemoryCredentialStore` 结果，不替代 Windows 原生窗口视觉、DPI 缩放、真实键盘/IME 输入法与真实 PAT 的人工验收；这些原生验收按协调者指令延后至合入 `main` 后统一进行。
