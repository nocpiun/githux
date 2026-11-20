# 界面表面阴影：盘点现有 elevation，补两个真正悬浮的位置

- 日期：2026-11-20；任务 ID：`ui-surface-shadows`；执行者：基层 Agent；分支 `feat/ui-surface-shadows`，base `10a663a`；PR 待创建。
- 目标与状态：实现中 → 待负责人人工验收。**本任务任何情况下都不自行合并、不开自动合并**，即使 CI 与审查全绿。

## 目标与范围

负责人要求给适合表达前后层级的正式界面增加克制的 `box-shadow`，单独 PR，必须人工验收。范围限定：复用主题 elevation/shadow；浅色与 Geek Neutral 都核对；同一阴影不叠两次；不给所有列表行、标题带加阴影；不改主题配色、交互、焦点命中或整体布局；只碰阴影及必要的绘制/裁剪上下文；与 #99 的焦点/标题带、侧栏选中动画分开。

## 现有 elevation 盘点（改动前的基线）

`rg -n "shadow|elevation" src/` 在 base `10a663a` 上只有四处：

| 位置 | 当前做法 | 判定 |
| --- | --- | --- |
| command palette 卡片（`src/ui/palette.rs`） | `theme.elevation(Elevation::Modal)` | 已正确：Ely 层级、跟随模式。不改。 |
| 侧栏 More 菜单面板（`src/ui/sidebar.rs`） | `theme.elevation(Elevation::Floating)` | 已正确。不改。 |
| 所有 tooltip 与 `Select`/popup 下拉 | Ely 内部各画一次 `Floating` | 主题层已覆盖，不新增第二层。 |
| 拖拽标签 ghost（`src/ui/tabs.rs`） | 手写 `BoxShadow { color: colors.shadow, offset: (0,4), blur: 12 }` | 颜色对了，但偏移/blur 是绕开主题词表的自定义值；浅色下仍按深色的厚重比例绘制。 |

结论：正式界面缺的不是“更多层”，而是**一处一致、可复用、随模式变化的取值入口**，加一个确实悬浮但没画的内容（分界圆按钮）。因此不改 palette、不改已有浮层层级。

## 改动与决定

1. 新增 [`src/ui/elevation.rs`](../../src/ui/elevation.rs)：项目自绘阴影的唯一入口。`floating(cx)` = `Elevation::Floating`；`attach(div, cx)` 是唯一挂载点，保证同一元素不会被叠两层。
2. 侧栏收起/展开圆按钮（`sidebar-toggle`）加 `attach`。它是固定列里唯一真正压在别的内容之上的控件：24px 圆钮跨在侧栏与 island 分界线上。
3. 拖拽 ghost 改走 `attach`，删掉手写的 `BoxShadow` 字面量。可见变化：深色仍取深色的 `Floating` 值，浅色改取 Ely 浅色同层值，不再借用深色比例；层数从“单层 0,4,12”变为主题的两层词表，和 Ely 自己的浮动控件一致。
4. 刻意**不加**阴影：列表行、标题带、页面 header、tab 条、分区卡片、Settings/Accounts 表单块。

### 层级选择的返工（Raised → Floating）

首版按“24px 小控件用浅层”给按钮用了 `Elevation::Raised`。之后在 150% DPI 原生帧上实测，它在按钮周边只造成 1–2 个灰阶变化：

| 探针 | 深色地面 | 按钮周边 `Raised` | 按钮周边 `Floating` |
| --- | --- | --- | --- |
| 侧栏（`bg #191919`） | 25 | 24–25（可辨 0 像素） | 24（整圈） |
| island（`surface #202020`） | 32 | 32 或未变，环内仅 728/2117 像素变暗 | 24–31，2117/2117 全部变暗 |
| 浅色侧栏 | 250 | 250 | 247–249 |

结论：`Raised` 在这么小的盒子上不构成“明确可见的新增”，不符合验收要求；两个表面统一使用 `Floating`，同尺寸无浮层的对照区仍保持完全不变（0 个像素变化），说明读到的是阴影而不是别的绘制副作用。返工记录见下节原生证据（`probe-dark.png` 与 `shadows-probe-dark.png` 的对比）。

### 未选择的候选

- **island 本体**：它是窗口里唯一的页面容器，再投影只会和 `bg` 地面争注意力，还会要求在 `RoundedClip` 之外改裁切上下文。
- **给已有弹层再加一层**：palette/菜单/tooltip 已经各有一层，再叠就是本任务禁止的“同一阴影叠两次”。
- **滚动条、tab 下划线**：不是前后层级，属于 #99 的焦点/选中表现，保持分开。

## 验证

所有 Cargo 检查走共享 runner（`node scripts/verify.mjs -- cargo …`），记录在 `.agents/task/runs/`：

- `cargo fmt --all -- --check`，通过（先跑 `cargo fmt --all` 修正）；
- `cargo check --locked --all-targets`，通过；
- `cargo clippy --locked --all-targets -- -D warnings`，通过；
- `cargo test --locked --bin githux`：1517 通过 / 0 失败 / 8 ignored（含新增 `ui::elevation` 3 项：两模式各自等于主题 `Floating`；两模式都画非 inset、可见模糊的层且深浅不等；`attach` 只挂一次主题层）；
- `cargo build --locked`、`cargo build --locked --example shadows`，通过。

### 原生像素证据

离线探针 `examples/shadows.rs` 用生产 `elevation::attach` 画两处真实几何（侧栏/island 分界面 + 携带中的标签），并安装与启动相同的 Geek Neutral dark palette；`GITHUX_SHADOWS_MODE=light` 切到浅色。150% DPI 下 `PrintWindow` 取真实帧：

- `.agents/output/shadows-probe-dark.png`、`.agents/output/shadows-probe-light.png`；
- 量化记录 `.agents/output/shadows-probe-pixels.json`：两模式整圈变暗（深色 island 2117/2117 像素暗于地面，浅色 2117/2117 变暗 1–10 级）；同尺寸无浮层对照环 4280 个采样点 0 个偏离地面；携带标签底边带 22–28（深色）/ 241–250（浅色），同高度的平放标签 30–32 / 253–255，说明只有 ghost 有投影。

### 正式 binary 的原生启动

`cargo build --locked` 后启动 `target/debug/githux.exe`（PID 17240，Responding，标题 `Githux`）：侧栏 `#191919`、island `#202020`、分界按钮 24px 命中盒与 4 边 inset 均与改动前一致；`PrintWindow` 截图 `.agents/output/shadows-dark-150.png` 中按钮周边可测得 48→24 的整圈暗环，标题带/页面 header/列表行区域仍是 `surface` 单一值 32，未出现额外层。

**未覆盖**：本机桌面会话拒绝把该窗口设为前台（`SetForegroundWindow` 返回 0，前台保持其他应用），也无法从锁屏状态送达真实点击，因此**没有**在正式 binary 里点 Sun/Moon 切到浅色的原生证据；浅色一侧来自同一套 `elevation` 角色在离线探针中的真实帧。原生观感（是否过重、边缘是否发灰、拖拽手感）仍需负责人验收。

## 渲染上下文与回归边界

- 阴影由 GPUI 在元素自绘阶段画在背景之前（`Style::paint` → `paint_drop_shadows`），只向外扩、不占布局；按钮 24px 盒子、命中范围、`occlude`、hover/focus 背景、ghost 的 `TAB_WIDTH × TAB_HEIGHT` 与抓取偏移都不变。
- 按钮位于 `sidebar_divider` 的绝对定位层，不在任何 `overflow_hidden` 祖先内被裁切；ghost 由 GPUI 作为拖拽视图单独绘制，也不受标签条 `overflow_hidden` 影响。两点决定了不需要新增裁剪上下文。
- 不改主题 token：`src/app/theme.rs` 的 `shadow` 仍继承 Ely 深色值，浅色仍是 Ely 浅色值；`app::theme` 现有精确 token 用例未受影响。

## 后续

- 推送后触发 `@codex review`，按 skill 核验当前 head 的 CI/审查，加 `pending` 等负责人按 `docs/theme.md#预览与验收` 在真实窗口做明暗对照。
- 负责人验收前不合并、不 auto-merge、不移除 `pending`；`blocked` 仍由负责人管理。
- 若负责人希望更克制，只需在 `elevation.rs` 换层级，不必改各调用点；但换回 `Raised` 等于放弃可见效果，应重新讨论。
