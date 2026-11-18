# 横向 tab 下划线的移动/宽度过渡与非 Reader 链接光标

- 日期：2026-11-15；任务 `ui-navigation-interactions`；分支 `fix/ui-navigation-interactions`，基于 main `812e5fc`；交付 `pr`，First Release #1。
- 目标：负责人问题 5（活动横向下划线在 tab 之间平滑移动并改变宽度）与问题 4（Markdown 之外的链接光标）。状态：实现与本地检查完成，待 PR 审查与负责人原生验收。

## 复现（改代码之前）

- 问题 5：`probe_reproduce_underline_jump` 在基线的仓库页点击 `Issues` 之后只画一帧，`repository-tab-active-code` 已为 `None`，`repository-tab-active-issues` 直接出现在目标位置（x=91, w=105，Code 为 x=20, w=63）。即一项消失、另一项出现，没有移动也没有宽度过渡。原因：下划线是**被选中 tab 自己的绝对定位子元素**（`src/ui/repository/view.rs`、`src/ui/settings/layout.rs` 窄态、`src/ui/profile/render.rs` 的 Overview 条各写一份）。
- 问题 4：`probe_link_hit_area` 在基线测得通知行的仓库链接元素宽 **740px**（行 920px），点击名字右侧空白（x=756）仍然导航（`navigations_from_blank=1`）。链接自身带 `flex_1`，pointer、下划线 hover、tooltip 与点击一起被拉到整行剩余宽度。

## 改动与决定

- 新增 `src/ui/tab_underline.rs`（`ui::tab_underline`）：一份共享呈现。每个 tab 放一个不注册命中区的 `probe`（`canvas` + `absolute().inset_0()`）上报自身真实绘制边界，tab 条再用 `layer` 在该边界上画 2px 宽、贴 tab 底边的下划线。切换 tab 时插值 `left` 与 `width`（`ely_core::motion::ease_out_cubic`，180ms）。
  - 首次测量直接采纳（不从原点滑入）；布局事实（resize、字体/DPI、count pill 改变标签宽度、tab 条横向滚动）在同一个 tab 上直接采纳，不滑一段 tab 没走过的路径；切换 tab 时从**当前插值位置**出发，快速连选不回退。
  - reduced motion 立即落位；只有移动中才 `request_animation_frame`（paint 内请求 + 各页面 render 用 `is_animating` 兜底），静止不请求。
  - 只做呈现：tab 的间距、role、焦点、命中区、点击、tooltip、滚动与 `scroll_to_item` 都不进入该模块。Settings 窄态把下划线层放在滚动条的兄弟位置并 `overflow_hidden`，使被裁切的 tab 无法把线画到条外。
- 接入三处：仓库页内容 Tabs（`repository/view.rs::tab_strip`）、Settings 窄态分类 Tabs（`settings/layout.rs`）、Profile Overview 条（`profile/render.rs`，唯一一项，几何不变；原来占位的 `border_b_2` 改为同高的 2px 底 padding，由共享层画在 tab 实测底边上）。
- 问题 4 修复（本人责任区）：
  - 通知行的仓库链接：`flex_1` 移到外层非交互 slot，链接自身 `min_w_0`/`max_w_full`/`truncate`，pointer、hover、tooltip、click 回到名字文字盒。
  - `src/ui/work_item_row.rs`：只有 `on_row_click` 为 `Some`（全局 Issues/PR 列表，整行就是点击目标）时给行加 `cursor_pointer`；Home 这类只有文字链接的行不加，空白仍保持默认箭头。
  - Profile 贡献图单元格（`Role::Button` + 点击选中某天）补 `cursor_pointer`。
- 审计清单（未改他人区域）：Home/Feed 的 owner/name/actor/title/Read more 与 `work_item_row` 的 `Role::Link` 已有 pointer，且宽度按文字（`src/ui/home/row_tests.rs` 已覆盖）；仓库页 Tabs、文件树行/chevron、Profile 仓库卡片与 website 链接均已一致。`reader/`（reader-typography 归属）与侧栏（sidebar-quick-actions 归属）只读审计：Reader Markdown 链接另行处理；侧栏 `account-login` 只在下划线时使用身份控件整体的 pointer，与“点击身份控件”一致，无需改动。`repository_overview/summary.rs`（repository-summary-polish 归属）中 Releases 版本与 contributor 头像链接已有 pointer，本任务未发现缺失，未编辑该文件。未发现其他“光标落在文字外”的链接。

## 验证

所有 Cargo 调用经 `node scripts/verify.mjs -- cargo ...`（共享构建槽），日志在 `.agents/task/runs/`。

- 复现：`8dd12bf6`（跳变，基线，FAILED 即复现）、`df69b56b`（行内空白仍导航，基线）。
- `src/ui/tab_underline.rs` 状态机：7 项（首帧采纳、移动+宽度插值、中断续接、reduced motion、resize/字体/badge 采纳、滚动采纳、未测量不画）— run `319a0337`。
- 视图层：仓库页 `the_active_tab_band_starts_on_its_tab_and_moves_and_resizes_between_tabs`（鼠标点击真实 tab、三帧位置与宽度、落位后不再动画）、`an_interrupted_tab_selection_continues_from_the_band_in_flight`、`reduced_motion_puts_the_band_on_the_selected_tab_in_one_frame`、`the_band_follows_a_resize_and_a_scale_change_without_sliding`（resize 与 1.5× 缩放后一帧内贴合 tab，不滑行）；Profile `overview_tab_band_covers_the_measured_tab_box`；通知 `the_repository_link_covers_the_name_and_not_the_row_blank_space`（含“点空白不导航”）。run `edea84eb`（5 passed）与加入 resize/scale 用例后的 run `e2f4393c`（4 passed）。
- Settings 窄态（`cargo test --locked --example settings`，该模块只在 example 测试目标里编译）：既有 `narrow_tabs_scroll_without_content_overflow_and_keyboard_reveals_last` 改为断言共享层落在被 reveal 的末项实测盒上，并新增键盘驱动的 `narrow_tab_band_moves_and_resizes_with_keyboard_selection`。run `c34c20ee`：含两项在内 9 passed。
- 全量：`cargo test --locked` run `3ea25c49`（各 target 合计 1412/116/668/392/349/910/1020/44/1413 passed，0 failed）、`cargo test --locked --example settings` run `253a7695`（1425 passed，含两项 Settings 用例）、`cargo clippy --locked --all-targets -- -D warnings` run `34312bf0`、`cargo build --locked` run `0137bab6`、`cargo fmt --all -- --check` run `c6342930`，均 exit 0。独立开发模块树 `src/bin_support/repository_modules.rs` 与 `profile_modules.rs` 挂上同一 `tab_underline` 文件（不复制实现），否则 `tests/repository.rs` 等 `#[path]` 装配缺模块。原生 Windows 光标字形、真实拖拽/输入与手感未经本人核验（GPUI 测试平台没有公开的 cursor 读取接口，pointer 相关结论是“命中盒几何 + 真实点击行为”层面的证据，不是光标字形证据，遵循 L002）。

## 负责人验收反馈后的返工：选中态不再改变 tab 宽度

负责人反馈：下划线动画没问题，但 inactive → active 时字重从 400 变 500 让 tab item 宽了几个像素，视觉明显。

- 复现（先测量再改）：bundled `Githux UI` 在 14px 下 `layout_line("Pull requests")` 实测 `NORMAL = 84.952px`、`MEDIUM = 87.304px`。视图层回归 `tab_boxes_keep_their_geometry_when_the_active_tab_changes_with_bundled_fonts` 在基线实现上失败：`issues: 96px → 97px`（origin 未变，但宽度变化会把右侧 tab 整体推开）；窄态 Settings 同类用例实测 `Accounts: 87px → 86px`。
- 前一版探测一度显示“无变化”，原因是测量走了错误的字体：`RepositoryPage` 的测试装配在窗口建好后会再执行一次 `setup()`，把主题字体改回系统 fallback（TestTextSystem 各字重推进相同），且测试窗口根节点没有像正式 Shell 那样设置 `font_family`。修正为 `native_font_context()`（native DirectWrite + bundled faces）+ `ThemedRepositoryPage`/`ThemedHost` 根节点（与 `home/row_tests.rs` 的 `ThemedHome` 同一做法），先量出真实差异再改代码。
- 修复：`src/ui/tab_underline.rs` 新增 `TabUnderline::label(text, active, color)`，可见文字保持当前字重，下面叠一个 `h(px(0.0))`、`overflow_hidden()`、`opacity(0.0)`、`MEDIUM` 的透明占位文本，外层 `flex_col().items_center()`。占位文本没有 id/role/命中区/tooltip，不重复无障碍名称，只贡献推进宽度。仓库内容 Tabs 与窄态 Settings 分类条都改用它；Profile Overview 只有一项，宽度不变因此不需要占位；宽态 Settings 是 168px 固定栏 + 左对齐文字，不涉及流式宽度，未改动；`src/ui/tabs.rs` 的顶部会话标签（固定 `TAB_WIDTH`、拖动 ghost/箭头光标）按要求完全未动。
- 同时 `git fetch origin main` 后 `git merge --no-edit origin/main`（`4e75ad8`，含 #90 bash 超时与 #93 侧栏/主题），`src/ui/mod.rs` 的相邻改动没有冲突，#93 的侧栏/主题支持与本任务的 `tab_underline` 注册同时保留。

- Codex 第二轮 P3 成立：`TabUnderline::label` 第一版在可见文字上又显式设置了 `text_color`，压过了 tab 自身的 `.hover(|style| style.text_color(fg))`，未选中 tab hover 时文字不再变亮。已去掉该颜色参数，可见文字与占位都继承 tab 的颜色（选中/未选中色与 hover 色都由 tab 提供）。GPUI 测试平台没有查询绘制颜色的接口，这一项按代码路径核对并在本日志记录边界，不用不可靠的断言冒充证据。

## 验证（返工轮）

- 两项新几何回归在去掉占位层时分别失败（仓库 `96px → 97px`、窄态 `87px → 86px`），恢复后通过：run `d39ca100`（仓库）、`b01e7b92`（窄态）。
- 既有下划线用例一并重跑 `e7ebd0da`：5 passed（移动/宽度插值、中断续接、reduced motion、resize+缩放、单 tab 几何）。
- 合并 origin/main `47ae188`（#92/#95）后重验：clippy `d1c2361b`、`cargo test --locked` `cbab9344`、`--example settings` `00f892de`、fmt `3ee45813`、release build `b16e7766`，均 exit 0；去掉标签颜色参数后再跑 clippy `304bfe2b`、`cargo test --locked` `f7898559`、`--example settings` `9bdf07f0`、fmt `70f516a2`，均 exit 0。
- 全量（返工轮首次）：`cargo test --locked` `aaf8fce3`（1416/116/668/392/349/910/1022/44/1417 passed，0 failed；首跑 `04dfa4b0` 有 1 项与本任务无关的 `reader_images` 5 秒超时，单跑 `5657f136` 与复跑均通过）、`cargo test --locked --example settings` `bbb7223a`（1430 passed）、`cargo clippy --locked --all-targets -- -D warnings` `f0039e2c`、`cargo fmt --all -- --check` `95f520c9`、`cargo build --locked --release --bin githux` `92415e48`，均 exit 0。
- 负责人用 release 做视觉验收，因此额外执行 `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/prepare-windows-runtime.ps1 -Destination target/release`（27 unchanged，0 written）。

## 后续

- PR 关联 First Release #1 并加 `pending`，交负责人原生验收：三处下划线移动/宽度手感、减少动态效果开关、窄窗 tab 滚动时的裁剪、通知行/全局列表/贡献图的光标与点击范围。
- 未覆盖：真实 Windows 原生光标字形、混合 DPI 屏幕上的实际手感、真实指针拖动/输入；`cargo test` 不执行 example 内的 Settings 用例，需显式 `cargo test --locked --example settings`。
- 教训候选（供协调者汇总）：链接的 `flex`/`w_full` 放在**交互元素自身**会同时扩大 pointer、hover、tooltip 与 click，四者必须一起校验；“点击空白仍导航”可以作为这种错位的可执行回归。横向下划线的测量要用绘制期真实边界（`canvas` 的 paint/prepaint bounds），不要用样式常量或索引算术推断。
