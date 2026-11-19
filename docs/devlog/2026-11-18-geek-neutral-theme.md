# 正式深色主题：Geek Neutral

- 日期：2026-11-18（2026-11-19 负责人反馈轮更新）；任务 ID `geek-neutral-theme`；分支 `feat/geek-neutral-theme`；任务 base `78d59400112225a069332e1c6c2dfef4d79b5f3b`，PR 交付前已 rebase 到 main `4157641`；反馈轮先 rebase 到 `684c84f`，随后 main 合入 #91（`154b7f4`）的共享 tab 下划线，交付前再次 rebase 到 `154b7f4` 并重新核对、重新验证组合。
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

## 2026-11-19 负责人反馈轮：恢复 accent 家族

负责人在 PR #96 head `108d4ad` 上指出三处可见回归，它们是同一个根因：第一版把对照实验的紫色 `accent`/`accent_hover`/`on_accent`（`#a371f7`/`#b98cff`/`#0d0d0d`）当成了产品强调色，而这三个 token 在正式应用里承担多个旧角色。

1. **Feed “Read more”**：`src/ui/home/rows.rs` 的链接文字读 `colors.accent`，因此紫色覆盖把它从原正文白变成了紫色。恢复继承后即为 `#f3f1f0`，与 #96 之前一致。
2. **活动页内 tab 下划线**：`src/ui/tabs.rs`（拖拽插入标记）、`src/ui/repository/view.rs`、`src/ui/settings/layout.rs`（窄态分类）、`src/ui/profile/render.rs`（Overview 底边）都消费 `colors.accent`。恢复继承即取回原色。main 在反馈轮期间合入 #91，把这些下划线统一为 `src/ui/tab_underline.rs` 的一份共享呈现（实测边界、移动与宽度插值）；该模块同样把 `theme.colors.accent` 交给 `layer(...)`，所以恢复 token 后共享 band 自动取回原色，不需要改 #91 的实现。
3. **紫色 active/selected 样式**：Accounts “✓ Current”标记（`src/ui/accounts/form.rs` 的图标与文字）与 “Verify and add” 主按钮（Ely `ButtonVariant::Primary` = `accent` 填充 / `on_accent` 文字 / `accent_hover` 悬停）恢复原值。

处理方式：`geek_neutral()` 不再给这三个 token 赋值，改为从 `Palette::dark(false)` 继承；模块注释、`docs/theme.md` 与测试同步说明，并写明 “不要再加回紫色”。同一轮还核对了其余 `accent` 消费方——command palette 底部 2px 标记、Notifications 未读圆点、`work_item_row` 的 Merged 状态色、`badge` 的 Accent tone、窗口关闭按钮文字（`on_accent`）与 `chrome` 的关闭前景——它们都由同一 token 驱动，因此同时回到原值，没有逐个改成新的第二强调色。`chart`/`syntax`/`link`/`focus`/语义色、碳灰表面与中性行底色、浅色 palette 全部保持不变。

被否定的旧断言：第一版 `the_dark_palette_is_exactly_the_owner_selected_geek_neutral` 把 `accent #a371f7` 等三项当成 “Geek Neutral 必须精确匹配”的候选值；现在这三项移入 “继承 Ely 深色” 的断言组，并额外写死 `#f3f1f0`/`#d9d7d5`/`#252525`，防止 `Palette::dark` 未来的变化静默改掉可见颜色。

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

- `the_dark_palette_is_exactly_the_owner_selected_geek_neutral`：逐项断言 25 个替换 token、`selection` alpha、8 个 chart、16 个 ANSI、13 个 syntax 与 `glass`；断言 10 个继承 token（含 `accent` 三值）仍等于 Ely 深色值，并把 `accent` 三值写死为 `#f3f1f0`/`#d9d7d5`/`#252525`。曾把紫色临时放回 `geek_neutral()` 验证该项失败（`4e78e6e4`）。
- `the_surfaces_are_achromatic_and_step_cleanly`：`R == G == B`、`sunken < bg < surface < overlay < hover < active` 且每级至少 4/255，文字权重递减。
- `startup_and_a_light_round_trip_show_the_same_geek_neutral`：GPUI 测试平台安装后首帧即为 Geek Neutral；切浅色为 Ely light；再切深色恢复同一套值。

反馈轮新增的三处可见回归覆盖（都读实际提交给渲染器的 quad，而不只比对常量）：

- `read_more_keeps_the_pre_theme_accent_text_color`（`tests/home.rs`）：截断预览出现 “Read more”，其文字色解析为继承后的 `#f3f1f0`，并显式断言不是 `#a371f7`。
- `the_active_tab_band_paints_the_theme_accent`（`tests/repository.rs`）：切到深色后读取 #91 共享 band 的 `tab_underline_rect()`，断言其对应 quad 的填充等于 `colors.accent`，切到 Issues 后仍成立；该值写死为 `#f3f1f0`。用 pin_clock 让 band 停稳，使上报矩形与提交 quad 描述同一帧。
- `narrow_tabs_scroll_without_content_overflow_and_keyboard_reveals_last`（Settings 窄态用例，`--example settings`）：在 #91 的 `category_underline_rect()` 上读取共享 band 的 quad，等于 `colors.accent`，同样写死 `#f3f1f0`。
- `the_current_mark_and_primary_button_keep_the_pre_theme_accent`（主程序测试）：`app::accounts::tests::setup` 先装产品深色 palette，断言 `accent`/`accent_hover`/`on_accent` 三值、表单范围内没有任何 `#a371f7` 填充，并确认启用的 Verify 主按钮确实画出继承后的强调填充；`account-current-badge` 仍在。

### 2026-11-19 Home 离线分页测试竞态（任务包 A）

PR #96 的 `Code check` 首次失败在既有测试 `app::pages::home::tests::online_cache_reopens_and_offline_reads_the_same_home_feed_and_older_page`（`left: ["first", "second"]` / `right: ["first"]`），与主题改动无关，属于 [L015](../../LESSONS.md#l015虚拟列表的未知高度不能当成内容耗尽) 记录的短首屏自动预取竞态；同类失败在 2026-11-10、2026-11-11 的开发日志中也已出现。

修复只改 `src/app/pages/home/tests.rs`，不动生产逻辑：

- 新增 `assert_cached_feed_prefix` / `is_legal_cached_feed_prefix`：离线首屏只能是缓存集合自身的有序前缀（`["first"]` 或 `["first", "second"]`），条目不得重复、不得乱序、不得出现集合外 id、不得为空页。
- `require_cached_continuation`：若自动预取尚未带出第二页，才手动 `load_more_for_test` 并等待 `["first", "second"]`。
- 三处同款断言改为该规则：`online_cache_reopens_...`（仍要求最终 `["first", "second"]`、Feed provenance 与 `saved description`）、`acceptance_cold_start_without_network_revalidation_replays_saved_home`、`network_failure_after_the_cached_paint_keeps_the_readable_snapshot`。
- 新增纯规则单测 `only_the_cached_ordered_prefix_is_a_legal_offline_first_screen`：防止规则被悄悄放宽（曾临时改回只接受 `["first"]` 验证它确实失败）。

验证（`node scripts/verify.mjs`）：

| 检查 | run | 结果 |
| --- | --- | --- |
| 三个用例各连续 5 次（`--test scroll_viewports -- --exact <name>`） | 15 个 run | 15/15 rc=0，每次 `1 passed; 0 failed` |
| 规则单测 `--exact` | `cecd4f5d` | 通过 |
| 规则被放宽时同一单测 | `d82c6116` | 失败（证明非恒真） |
| 整个 `--test scroll_viewports` | `64f57b3f` | 1480 passed, 0 failed, 3 ignored |

未覆盖：CI 真实负载下的重复稳定性仍以 PR 新提交的 CI 为准；本次未改生产代码。

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
