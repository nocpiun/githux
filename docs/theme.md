# 正式深色主题

更新：2026-11-20。状态：**已实现，等待负责人原生观感验收**。主题 token 的权威实现是 [`src/app/theme.rs`](../src/app/theme.rs)，启动接线在 [`src/app/mod.rs`](../src/app/mod.rs)，阴影/浮层的唯一取值入口是 [`src/ui/elevation.rs`](../src/ui/elevation.rs)（见[阴影与浮层](#阴影与浮层)）；本任务记录见[开发日志](devlog/2026-11-18-geek-neutral-theme.md)（含 2026-11-19 负责人反馈轮）与[阴影盘点](devlog/2026-11-20-ui-surface-shadows.md)。

## 模式与安装

正式应用启动时通过 Ely 公开 API 安装深色 palette，再立即显示深色：

- `Theme::set_palette(Mode::Dark, Some(geek_neutral()), cx)` 设置深色目标；
- `Theme::set_mode_now(Mode::Dark, cx)` 停止启动淡入，使窗口**首次绘制**就是最终颜色。

侧栏的 Theme 入口仍走现有 `Shell::toggle_theme` / `Theme::set_mode`：切到浅色使用 Ely 的 `Palette::light(false)`，切回深色重新解析同一份 Geek Neutral。字体（Noto Sans / Google Sans Code）、焦点/选区行为、页面状态、Windows 原生标题栏配色同步和既有主题淡入都不变。启动默认仍是深色。

## 取代与继承

实现以 `Palette::dark(false)` 为底，显式覆盖下面各表列出的 token；未列出的 `on_media`、`backdrop`、`media_backdrop`、`shimmer`、`shadow`、`paper`、`ink` 保持 Ely 深色值。`glass` 使用新的 `bg` 加 Ely 原有 `0xb8` alpha，因此模糊浮层跟随新地面而不是透出旧灰。

**`accent` / `accent_hover` / `on_accent` 也继承 Ely 深色值**（`#f3f1f0` / `#d9d7d5` / `#252525`），不采用对照实验里的紫色强调。这三个 token 同时承担活动页内 tab 下划线、Feed 的 “Read more” 链接与 PR 标记、Accounts 的 “✓ Current”、Settings 分类下划线和 Ely 主按钮填充，负责人要求这些保持主题改动前的颜色；继承 `Palette::dark(false)` 正好复现改动前的实际取值。不要只改其中一个去向，也不要重新引入第二种 accent 色。

### 表面与文字

| token | 值 | 用途 |
| --- | --- | --- |
| `sunken` | `#141414` | 代码井、下沉槽 |
| `bg` | `#191919` | 窗口地面、侧栏 |
| `surface` | `#202020` | island、卡片、阅读面 |
| `overlay` | `#262626` | 浮层、菜单 |
| `hover` | `#2c2c2c` | 指针悬停底 |
| `active` | `#333333` | 选中/按下底 |
| `border` | `#303030` | 1px 发丝线 |
| `border_strong` | `#474747` | 必须看清的规则线 |
| `fg` | `#ededed` | 正文 |
| `fg_muted` | `#b8b8b8` | 次要文字 |
| `fg_subtle` | `#9e9e9e` | 提示、占位 |
| `fg_disabled` | `#525252` | 不可用文字 |
| `tooltip_bg` | `#3a3a3a` | tooltip 底 |
| `tooltip_fg` | `#ededed` | tooltip 文字 |

灰阶层严格无彩（`R == G == B`），并保持 `sunken < bg < surface < overlay < hover < active`，每级同通道至少相差 4/255。

### 信号、代码与数据色

| token | 值 |
| --- | --- |
| `link` | `#6cb6ff` |
| `focus` | `#4d9fff` |
| `selection` | `#4d9fff`，alpha `0x4d` |
| `success` / `success_subtle` | `#3fb950` / `#1b2f21` |
| `warning` / `warning_subtle` | `#e3b341` / `#382f1c` |
| `danger` / `danger_subtle` | `#ff6b6b` / `#3a2321` |
| `info` / `info_subtle` | `#58a6ff` / `#1a2b3d` |

`accent`、`accent_hover` 与 `on_accent` 不在上表：它们继承 `Palette::dark(false)`，为 `#f3f1f0`、`#d9d7d5`、`#252525`，与主题改动前一致（见上一节）。受影响的现有消费方包括活动页内 tab 下划线（仓库内容 Tabs、窄态 Settings 分类、Profile Overview）、Feed “Read more” 与 PR 类型图标、Accounts “✓ Current”、Settings 分类下划线和 Ely 主按钮（如 “Verify and add”）。深色下这些值与 `fg` 相同，浅色仍使用 Ely light 的 accent 三值。

`chart` 8 色依次为 `#6cb6ff`、`#56d4dd`、`#e3b341`、`#f2788f`、`#bc8cff`、`#3fb950`、`#ff9e64`、`#7ee0ff`。`ansi` 16 色为 `#333333`、`#ff7b72`、`#3fb950`、`#e3b341`、`#58a6ff`、`#bc8cff`、`#39c5cf`、`#b8b8b8`、`#525252`、`#ff8b85`、`#56d364`、`#f0cc6a`、`#79c0ff`、`#d2a8ff`、`#56d4dd`、`#ededed`。

`syntax` 13 个 token：`keyword #ff7b72`、`string #a5d6ff`、`number #79c0ff`、`comment #8b949e`、`function #d2a8ff`、`type_name #7ee787`、`constant #ffa657`、`property #79c0ff`、`tag #7ee787`、`attribute #ffa657`、`operator #c9d1d9`、`punctuation #8b949e`、`variable #ededed`。图表等消费 `chart` 的现有组件随语义 token 自动换色；仓库语言条使用独立、固定的 GitHub Linguist 数据色（main #92），不由主题覆盖。作者图片与外部内容颜色同样不进入主题覆盖。

## 阴影与浮层

阴影只属于真的压在别的内容之上的表面。正式界面里 [`src/ui/elevation.rs`](../src/ui/elevation.rs) 是项目自绘阴影的唯一入口，`attach` 从 Ely 的 `Elevation` 表取 `Floating` 层，而不是自己写 offset/blur/颜色：

| 位置 | 元素 | 层级 |
| --- | --- | --- |
| 侧栏与 island 分界的收起/展开圆按钮 | `sidebar-toggle` | `Elevation::Floating` |
| 拖动中的会话标签 ghost | `tab-drag-ghost` | `Elevation::Floating` |

Ely 自己绘制的浮层继续各用一次它自己的层级：command palette 卡片是 `Modal`，侧栏 More 菜单与所有 tooltip 是 `Floating`，选择器下拉（仓库分支、Issues/PR 过滤）由 Ely 的 `Select`/popup 内部绘制。它们不属于本模块，也不额外叠加第二层。

颜色取自 palette 的 `shadow` token，因此深色 Geek Neutral 与 Ely 浅色各自使用本模式的值；标签 ghost 过去写死的 `0,4,12` 黑投影在浅色下会显得过重，现在两个模式都走 `Floating` 的 alpha 与偏移。

**未采用更浅的 `Raised`。** 首版按“小控件用浅层”选了 `Raised`，之后在 150% DPI 原生帧上实测：它只让按钮周边像素变化 1–2 个灰阶（深色 `#191919` 侧栏地面从 25 变到 24–25），窗口里看不出阴影，不满足“至少有明确可见的新增/改善”。改用 `Floating` 后同一区域可测得 32→24（深色）/ 255→245（浅色）的整圈暗环，同尺寸无浮层的对照区保持完全不变。证据见[阴影盘点](devlog/2026-11-20-ui-surface-shadows.md#原生像素证据)。

**刻意保持平面**的对象：列表行、标题带、页面 header、tab 条、分区卡片、Settings/Accounts 表单块。给每一行都加阴影会把一页内容读成一叠卡片，本任务不这么做。未列入上表的元素不应自行 `.shadow(...)`；新增悬浮表面时先在这里登记它的层级，并复用 `elevation::attach`。

形状与裁切无关：阴影只画在元素外框之外，不改尺寸、padding、命中区、焦点环或滚动几何。`cargo test --locked --bin githux ui::elevation` 校验两个模式下的层级、颜色差异，以及一次挂载不会叠两层。

### 预览与验收

正式 binary 已包含这两个表面，无需 example：

```powershell
cargo run --locked --release
```

1. 启动即见侧栏与 island 左边框之间的圆形收起按钮，默认深色下它带一圈投影；
2. 底栏第一个按钮（Sun/Moon）切到浅色，同一位置比对浅色投影，再切回深色；
3. 按住任一会话标签拖动，`tab-drag-ghost` 带投影跟手；松手后投影随 ghost 一起消失。

只想对比两套主题的阴影而不走 Shell 时，用离线探针（不读账户、不联网）：

```powershell
cargo run --locked --example shadows
$env:GITHUX_SHADOWS_MODE="light"; cargo run --locked --example shadows
```

测试平台只校验层级/颜色契约，探针只校验两模式下的绘制差异，都不判断原生观感；投影是否过重、是否抢眼、是否被裁断仍由负责人在真实 Windows 窗口验收。

## 边界

- 本任务只落负责人选定的 **Geek Neutral**。实验分支的候选切换器、对照 UI、布局改版、主题偏好持久化和新增 Settings 不在正式产品中，也不依赖实验 worktree 路径。
- 浅色 palette、字体、间距、滚动、tab 几何与页面生命周期不变；#91 的共享下划线呈现（一次测量、移动与宽度插值）与本主题无关，只是它消费的 `accent` 回到原值。
- 正式产品没有高对比度或色盲安全开关；安装自定义深色 palette 后，深色目标固定为本文的值，不叠加 Ely 的高对比度深色变体。若以后接入这些设置，需要另行定义它们与 Geek Neutral 的组合。
- 精确 token 与浅→深往返由 `cargo test --locked app::theme` 覆盖；Windows 原生绘制观感、GPU/显示器差异和真实页面整体效果由负责人用正式入口验收，不由测试平台推断。

## 正式启动

在本 worktree 运行（`cargo run` 会自动准备 app-local Windows 运行时）：

```powershell
cargo run --locked --release
```

也可用 `cargo run --locked` 验证 debug 构建；两者都从正式 binary 入口启动，不是 example 预览。
