# 正式深色主题：Geek Neutral

- 日期：2026-11-18；任务 ID `geek-neutral-theme`；分支 `feat/geek-neutral-theme`；任务 base `78d59400112225a069332e1c6c2dfef4d79b5f3b`，交付前已 rebase 到当时 main `4157641` 重新验证组合。
- 目标：负责人已在 `dark-theme-comparison` 第二轮选定 **Geek Neutral**，把该配色接到正式 Githux Windows 应用的深色模式；原实验继续保留对照。
- 状态：**已实现、测试与 release 构建通过，待负责人原生观感验收**。稳定契约与完整 token 表见[正式深色主题](../theme.md)。

## 来源与边界

色值权威来源是本地 Git 对象 `cb2d350b410123174a489370fe3b2dcdec48f9e0` 的 `src/bin_support/dark_theme_palettes.rs`（`Candidate::GeekNeutral.palette()`，`GEEK_NEUTRAL_RAMP` + `BLUE_SIGNALS`），并对照同提交的 `dark_theme_compare_tests.rs` 与实验开发日志。正式分支独立提供 palette，不引用实验 worktree 路径、候选切换器、对照 UI 或实验 target；实验的 `Prev Neutral`、`Geek Deep`、`Geek Phosphor` 与 `Current main` 均未进入产品。

范围保持：

- 只替换 Ely 的**深色**目标；浅色仍是 `Palette::light(false)`。
- 不引入主题候选切换、持久化、新 Settings 项、布局/字体/间距/滚动/tab 几何改动。
- 独立数据色与作者图片不进入主题覆盖：仓库语言条使用固定的 GitHub Linguist 数据色（rebase 到 main #92 后确认），图表等仍消费语义 `chart`；页面状态不另写暗色常量。
- 启动默认仍为 Dark；Windows 原生标题栏仍随当前 `colors.bg`/`colors.fg` 同步。

## 实现

- 新增 [`src/app/theme.rs`](../../src/app/theme.rs)：`geek_neutral()` 以 `Palette::dark(false)` 为底，显式赋值全部表面、文字、信号、chart、ANSI、syntax 与 `glass` token；未覆盖的 `on_media`、`backdrop`、`media_backdrop`、`shimmer`、`shadow`、`paper`、`ink` 保持 Ely 深色值。
- `install(cx)` 先 `Theme::set_palette(Mode::Dark, Some(...))`，再 `Theme::set_mode_now(Mode::Dark)`：停止启动淡入，使窗口**首次绘制**即为 Geek Neutral，同时把自定义 palette 记录为后续深色目标。
- [`src/app/mod.rs`](../../src/app/mod.rs) 在字体注册后调用 `theme::install(cx)`，替换原来的 `Theme::set_mode_now(Mode::Dark)`。
- 侧栏既有 `Shell::toggle_theme` 不改逻辑：浅→深重新解析同一份自定义深色 palette，浅色仍走 Ely；字体覆盖、路由、页面状态和 Windows 标题栏同步路径不变。
- [`src/ui/tabs.rs`](../../src/ui/tabs.rs) 的拖拽 ghost 阴影由字面量 `rgba(0x000000)` 改为语义 `colors.shadow`。深色值不变（Ely 深色 shadow 为黑），浅色则使用 Ely 的 shadow 值；这是本次检查到的唯一绕开主题的旧暗色常量，Reader 其余测试色只在 `#[cfg(test)]` 中。

## 验证

全部 Cargo 操作通过 `node scripts/verify.mjs` 执行，以记录的 exit code 与日志为准：

| 检查（rebase 到 main `4157641` 后） | run | 结果 |
| --- | --- | --- |
| `cargo fmt --all -- --check` | `fbd6ce1e` | 通过 |
| `cargo clippy --locked --all-targets -- -D warnings` | `7fffab74` | 通过 |
| `cargo test --locked` | `c755becd` | 通过，全部 target 0 failed；主程序 1424 passed / 3 ignored，完整测试包含本任务 3 项主题用例 |
| `cargo build --locked --release` | `c94bc692` | 通过 |
| `git diff --check` / `git diff --cached --check` | — | 通过 |

rebase 前在 `78d5940` 基线上也执行过 targeted `cargo test --locked app::theme`（`18a54d1a`，3/3 通过）；rebase 后最终以 `c755becd` 的完整测试为准。

测试覆盖：

- `the_dark_palette_is_exactly_the_owner_selected_geek_neutral`：逐项断言 28 个替换 token、`selection` alpha、8 个 chart、16 个 ANSI、13 个 syntax 与 `glass`，并断言 7 个继承 token 仍等于 Ely 深色值。
- `the_surfaces_are_achromatic_and_step_cleanly`：`R == G == B`、`sunken < bg < surface < overlay < hover < active` 且每级至少 4/255，文字权重递减。
- `startup_and_a_light_round_trip_show_the_same_geek_neutral`：GPUI 测试平台安装后首帧即为 Geek Neutral；切浅色为 Ely light；再切深色恢复同一套值。

Windows 运行时与正式入口：

- `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/prepare-windows-runtime.ps1 -Destination target/release` 成功：首次 27 个文件写入，最终 release 上复跑为 27 unchanged / 0 written。
- 最终 release `target/release/githux.exe` 从本 worktree 启动，窗口标题为 `Githux`，进程保持运行；随后用 `CloseMainWindow` 关闭，无残留进程和 stderr。
- 正式 release 启动命令（本 worktree，`cargo run` 会自动准备 app-local 运行时）：

```powershell
cargo run --locked --release
```

## 已知边界与未覆盖

- **原生观感**：Windows 实际绘制、GPU/显示器差异、真实页面整体效果与深浅往返手感由负责人用正式入口验收；测试平台和进程启动/关闭不能替代。
- **关窗偶发 panic（既有）**：本机 release 用 `CloseMainWindow` 关闭时，基线与本改动都偶发 GPUI `RefCell already borrowed` panic（各 1/8 次采样），backtrace 指向既有 `Shell::new` 键拦截/窗口延迟回调，与本 palette 无关；本任务不修复该既有问题，也不把它记作 palette 验收通过。
- 产品没有高对比度/色盲安全开关；安装自定义深色 palette 后深色目标固定为本文/主题文档中的值。若以后接入这些设置，需要另行定义组合。
- 实验分支与本地对照资产未修改、未合入；本任务只提供正式应用需要的选定 palette。
