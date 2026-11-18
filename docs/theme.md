# 正式深色主题

更新：2026-11-18。状态：**已实现，等待负责人原生观感验收**。主题 token 的权威实现是 [`src/app/theme.rs`](../src/app/theme.rs)，启动接线在 [`src/app/mod.rs`](../src/app/mod.rs)；本任务记录见[开发日志](devlog/2026-11-18-geek-neutral-theme.md)。

## 模式与安装

正式应用启动时通过 Ely 公开 API 安装深色 palette，再立即显示深色：

- `Theme::set_palette(Mode::Dark, Some(geek_neutral()), cx)` 设置深色目标；
- `Theme::set_mode_now(Mode::Dark, cx)` 停止启动淡入，使窗口**首次绘制**就是最终颜色。

侧栏的 Theme 入口仍走现有 `Shell::toggle_theme` / `Theme::set_mode`：切到浅色使用 Ely 的 `Palette::light(false)`，切回深色重新解析同一份 Geek Neutral。字体（Noto Sans / Google Sans Code）、焦点/选区行为、页面状态、Windows 原生标题栏配色同步和既有主题淡入都不变。启动默认仍是深色。

## 取代与继承

实现以 `Palette::dark(false)` 为底，显式覆盖下表全部 token；未列出的 `on_media`、`backdrop`、`media_backdrop`、`shimmer`、`shadow`、`paper`、`ink` 保持 Ely 深色值。`glass` 使用新的 `bg` 加 Ely 原有 `0xb8` alpha，因此模糊浮层跟随新地面而不是透出旧灰。

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
| `accent` / `accent_hover` / `on_accent` | `#a371f7` / `#b98cff` / `#0d0d0d` |
| `success` / `success_subtle` | `#3fb950` / `#1b2f21` |
| `warning` / `warning_subtle` | `#e3b341` / `#382f1c` |
| `danger` / `danger_subtle` | `#ff6b6b` / `#3a2321` |
| `info` / `info_subtle` | `#58a6ff` / `#1a2b3d` |

`chart` 8 色依次为 `#6cb6ff`、`#56d4dd`、`#e3b341`、`#f2788f`、`#bc8cff`、`#3fb950`、`#ff9e64`、`#7ee0ff`。`ansi` 16 色为 `#333333`、`#ff7b72`、`#3fb950`、`#e3b341`、`#58a6ff`、`#bc8cff`、`#39c5cf`、`#b8b8b8`、`#525252`、`#ff8b85`、`#56d364`、`#f0cc6a`、`#79c0ff`、`#d2a8ff`、`#56d4dd`、`#ededed`。

`syntax` 13 个 token：`keyword #ff7b72`、`string #a5d6ff`、`number #79c0ff`、`comment #8b949e`、`function #d2a8ff`、`type_name #7ee787`、`constant #ffa657`、`property #79c0ff`、`tag #7ee787`、`attribute #ffa657`、`operator #c9d1d9`、`punctuation #8b949e`、`variable #ededed`。图表等消费 `chart` 的现有组件随语义 token 自动换色；仓库语言条使用独立、固定的 GitHub Linguist 数据色（main #92），不由主题覆盖。作者图片与外部内容颜色同样不进入主题覆盖。

## 边界

- 本任务只落负责人选定的 **Geek Neutral**。实验分支的候选切换器、对照 UI、布局改版、主题偏好持久化和新增 Settings 不在正式产品中，也不依赖实验 worktree 路径。
- 浅色 palette、字体、间距、滚动、tab 几何与页面生命周期不变。
- 正式产品没有高对比度或色盲安全开关；安装自定义深色 palette 后，深色目标固定为本文的值，不叠加 Ely 的高对比度深色变体。若以后接入这些设置，需要另行定义它们与 Geek Neutral 的组合。
- 精确 token 与浅→深往返由 `cargo test --locked app::theme` 覆盖；Windows 原生绘制观感、GPU/显示器差异和真实页面整体效果由负责人用正式入口验收，不由测试平台推断。

## 正式启动

在本 worktree 运行（`cargo run` 会自动准备 app-local Windows 运行时）：

```powershell
cargo run --locked --release
```

也可用 `cargo run --locked` 验证 debug 构建；两者都从正式 binary 入口启动，不是 example 预览。
