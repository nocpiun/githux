# 2026-10-28 repository-overview

任务：`repository-overview`。目标：实现可复用的 Releases / Contributors / Languages 仓库顶部资料组件，使用已实现的 `domain::repository_extras` 模型；独立于 repository-browser 的 Code 组件。

## 结果

- 新增 `src/ui/repository_overview/**`：`RepositoryOverview`、`OverviewBinding`、`ReleasesData`、`SectionState`、`OverviewAction`、`OverviewUpdate`、确定性 fixtures。
- 新增 `examples/repository_overview.rs` 与 `tests/repository_overview.rs`，通过 `src/bin_support/repository_overview_modules.rs` 的薄 `#[path]` 装配复用真实组件、`domain` 与公共 `ui::scroll`。
- 新增 `docs/repository-overview.md`，记录输入/动作 API、三段语义、布局/有界策略、预览命令、验证边界与后续正式接入步骤。
- 未修改 `src/ui/mod.rs`、`src/app/mod.rs`、`src/workspace/**`、`src/ui/page.rs`、repository-browser、Cargo 配置、CI 或 `docs/README.md`；正式产品模块树不导入该组件。

## 行为要点

- `OverviewBinding` 固定账户、仓库和宿主 generation；`apply_*` / `apply` 对不匹配绑定返回 `false` 且不改变当前状态。组件不读取凭据、不调用 HTTP、不下载 asset、不打开 URL、不写缓存。
- Releases 的 latest 使用 GitHub latest 语义；`None`/404 使用“无可见 latest，不证明仓库没有 Release”的歧义文案，列表仍显示 Release。draft/prerelease 标签保留；列表有界；Release body 只进入 bounded plain-text popover，不渲染 Markdown。
- Contributors 区分 204 `EmptyRepository`、成功空内容与错误；显示 `observed_entries()`，区分关联用户与匿名贡献者，不显示 email/raw JSON，不下载头像；大量贡献者只画有界行数。
- Languages 显示字节派生 share 与 bytes，`total_bytes == 0` 不推断 0%；名称省略号、颜色取主题 chart 色板、文字标签随 bar，注明舍入不保证总和。
- TOP 横条布局：宽窗口并排，窄窗口在顶部区域内换行/堆叠；行数按宽度收紧（宽 3 / 窄 2），组件高度有界并给下方阅读器留空间；不建立右侧纵栏。popover 使用公共 `ui::scroll` 的 `Motion` / `Bar` / `Bars`；关闭时取消 motion，避免与相邻阅读器竞争。

## 验证

执行命令与结果（本机 Windows，工作树 `D:\githux-repository-overview`）：

- `cargo fmt --all -- --check` 通过。
- `cargo check --locked --all-targets` 通过。
- `cargo clippy --locked --all-targets -- -D warnings` 通过。
- `cargo test --locked` 通过：主 binary 519 项、account 310 项、reader 123 项、repository_overview 44 项，0 失败。
- `cargo build --locked --example repository_overview` 通过。
- `cargo build --locked` 通过。
- `git diff --check` 通过。
- `cargo run --locked --example repository_overview` 实际启动到被外部 `timeout` 终止（退出码 143），说明 example 入口能进入事件循环；未做人工视觉/交互验收。

GPUI 测试覆盖：旧仓库/账户/generation 拒绝、单 section 失败/Partial 不影响其他段、latest 404 歧义、draft/prerelease 标签、匿名与大量贡献者及有界行、204/空内容/错误分离、零语言、长名称与舍入、popover 打开/关闭/焦点/Escape/外部点击、popover 滚动不泄漏到相邻模拟阅读器、回调使用当前绑定且每次点击一次、桌面与窄窗口 TOP 有界布局。

## 未执行 / 后续

- 未做真实 Windows 预览窗口的人工视觉、鼠标手感和多 DPI 验收；`cargo run` 启动事实不等于视觉验收。
- 未接入正式 Shell 或 repository-browser；未调用真实 `RepoService` / `repository_extras` 读取；未读取真实 PAT 或访问真实 GitHub。
- 正式接入仍须宿主构造绑定、发起独立三段读取、发布前复核 `is_current`、处理 typed 动作，并把组件放在仓库页 TOP 区域。边界与建议步骤见 `docs/repository-overview.md`。
