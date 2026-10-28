# 统一滚动与滚动条

更新：2026-10-28。状态：公共滚动条与公共 wheel motion 已实现。标签条、仓库 viewport 和已合并的 content-reader 使用统一 Bar/Bars；标签条、仓库、每会话页面、palette 使用同一 `Motion<T>` / 输入路径。本轮 content-reader 已接入真实源码双轴与 Markdown ListState 的公共 wheel motion，原生交互验收待负责人完成；account-shell 的账户迁移是独立并行任务，其接线与验收随对应 PR 交付。当前基线使用官方 Ely `ely-core`（固定 `30c66fe1d5d23db769a6c834b771d054d7f288cf`）与 GPUI / platform `0.3.10`。阅读器已改用本地 Markdown 组件及公开的真实 `ListState`，不再受上游 `MarkdownDocument` 私有列表限制。

本主题是滚动条外观、几何、拖动、淡入淡出与 `ScrollMotion` 边界的唯一维护位置。视觉基准来自 content-reader 当前实现（负责人 2026-10-26 明确以它为准）：细、两头圆、半透明、出现/消失渐变、无实体轨道。

## 源码位置与职责

| 位置 | 职责 |
| --- | --- |
| [`src/ui/scroll/mod.rs`](../src/ui/scroll/mod.rs) | 唯一的外观（`Style`）、放置（`Placement`）、单帧绘制（`Bar`）与区域状态（`Bars`） |
| [`src/ui/scroll/geometry.rs`](../src/ui/scroll/geometry.rs) | 唯一的 thumb 长度/位置/拖动换算（`Thumb`） |
| [`src/ui/scroll/fade.rs`](../src/ui/scroll/fade.rs) | 唯一的淡入淡出状态（`Fade`） |
| [`src/ui/scroll/motion.rs`](../src/ui/scroll/motion.rs) | 唯一的约 130ms wheel easing、轴选择、输入适配与 thumb/程序滚动协作（`Motion<T>`、`wheel_overlay`） |
| [`src/ui/scroll/target.rs`](../src/ui/scroll/target.rs) | 真实滚动目标契约（`ScrollTarget`）与 `gpui::ScrollHandle`、`gpui::ListState` 实现 |
| [`src/ui/tabs.rs`](../src/ui/tabs.rs) | 标签条自身布局；只保留 `TAB_*` 布局常量与预留 band |
| [`src/ui/geometry.rs`](../src/ui/geometry.rs) | `TAB_SCROLLBAR_BAND = scroll::THICKNESS + 2`，其余标签间距/高度仍在原处 |

`src/ui/tab_scrollbar.rs` 已删除。不得再新增第二份项目自绘的几何、拖动或 Fade 实现；缺口应在此模块补齐，而不是在消费者里复制。

## 固定上游复用评估（2026-10-27）

逐项读取了 Ely 固定提交 `30c66fe1d5d23db769a6c834b771d054d7f288cf` 的 `layout/scroll.rs` 与 `layout/scroll/source.rs`，以及 GPUI 0.3.10 的列表 API。上游已提供不少真实能力，但没有公开到足以保持本项目已确认外观和布局的组合：

| 上游能力 | 可否直接用于本项目 | 事实与最小边界 |
| --- | --- | --- |
| `Scrollbar::new(id, source, axis)` | 不能直接替换 `Bar` | 可接 `ScrollHandle`、`ListState`、`Rc<dyn ScrollModel>`；但绘制固定主题 6px、固定 overlay 与 2× 厚度 track、颜色 0.22/0.42，没有 style/placement setter。 |
| `Scrollbar::presence(f32)` | 只能接收显隐数值，不能替代 `Fade` | 它能驱动外部算出的 opacity，但 `auto()` 是活动立即显示、idle 900ms 后按 320ms 线性淡出，不能保持 hover 进入/离开各 140ms smoothstep、快速反向从当前值继续。 |
| `ScrollArea` | 不能替代 tabs viewport | 它重组内容、溢出与滚动行为，并把固定 overlay 条和自身 activity 状态一起组合；tabs 的预留 band、固定 toolbar、滚轮映射、reveal 与排序是已验收外壳职责。 |
| `ScrollSource::Handle/List/Model` | 作为上游概念核对过，不能直接当本模块目标 trait | `offset/place/set/end` 方法均为 crate 私有；外部调用方不能仅复用这些方法来获得几何/写入。`ScrollModel::place/set/offset` 公开，可供未来自定义内容 owner 实现自己的模型，但不能读取 `MarkdownDocument` 的私有列表。 |
| 上游 thumb 圆角与 GPUI drag | 已沿用的框架能力 | 上游本身使用 `rounded_full` 与 GPUI `on_drag`/`on_drag_move`，说明基础能力足够；Githux 的额外 owner/region/axis、drag hooks、外部释放/取消清理仍需本地实现。 |
| GPUI `ListState` 原生 scrollbar API | **已直接复用** | `ScrollTarget for ListState` 使用公开的 `viewport_bounds`、`scroll_px_offset_for_scrollbar`、`max_offset_for_scrollbar`、`set_offset_from_scrollbar` 和 `scrollbar_drag_started/ended`。列表只有纵向，横向调用被忽略而不是 panic，也不把横向假称为已支持。 |
| GPUI `UniformListScrollHandle::base_handle` | **已直接复用** | 它是公开的真实 `ScrollHandle`，覆盖纵/横两轴；不需要复制列表几何。 |

因此本 PR 不关闭、也不因上游出现同名控件就宣称所有局部实现必须永久保留。最小保留边界是：3px / 0.20 / 0.38 外观、band/overlay/corner placement、140ms 可逆 fade、独立区域与另一轴保留、drag 生命周期、隐藏不拦输入，以及对应测试。普通滚动目标与原生列表生命周期则直接复用公开 API，不修改全局 theme/rem、缩放/裁剪容器、缓存、fork 或 vendor。

## 外观参数（已实现）

| 参数 | 值 | 说明 |
| --- | --- | --- |
| 可见粗细 | 3 logical px | 圆角端点（`rounded_full`），无实心轨道 |
| 最小 thumb 长度 | 24 px | 极长内容仍可抓取；可移动距离不足时见边界处理 |
| 闲置透明度 | `fg` 0.20 | 无 tracks 背景 |
| thumb hover | `fg` 0.38 | 仅指针在 thumb 上时加深 |
| 淡入/淡出 | 140 ms smoothstep | 反向从当前已绘制值继续 |
| 命中区 | 由 `Placement::hit` 给出 | 可宽于可见线宽；完全隐藏时不构建元素 |

两轴、所有区域共用同一 `Style::DEFAULT`。区域差异只允许出现在 `Placement` 与区域自身的布局常量里。

## 状态归属

- 每个区域各自持有一个 `Bars`（渐变、已知的 thumb drag 轴）和一个真实的滚动目标句柄。没有全局单例、没有跨区域共享的 hover/drag。
- `Bars` 的输入只有四种：`hover`（指针进出区域）、`hold`（其它交互期间要求保持可见，例如标签重排）、`drag`（本区域自己的 thumb 拖动开始/结束，并转调目标的 `drag_started`/`drag_ended`）、`cancel`（区域关闭或窗口失焦时立即清理）。一处 `Bars` 同时只记录一个拖动轴，第二个轴开始会被忽略。
- 拖动身份包含 owner（窗口内实体 ID）、区域名与轴：同一 owner 下的纵向与横向、或两个不同区域，都不会互相驱动。`set_axis_offset` 只改指定轴，另一轴偏移原样保留。
- 可见性 = 指针在区域内，或本区域 thumb 正在被拖动，或显式 `hold`。标签重排用 `hold`，不伪装成 thumb drag，因此“所有类型的 drag”不会被识别成本区域 thumb 拖动。
- 当前 owner 由 `Shell::tab_strip_owner` 提供；未来消费者使用各自的实体 ID。

## 单帧接口

```rust
let bar = scroll::Bar {
    target: &scroll_handle,                       // impl scroll::ScrollTarget
    id: scroll::BarId { owner, name: "reader-scrollbar-y", axis: Axis::Vertical },
    placement: scroll::Placement::overlay(4.0, 8.0),
    style: scroll::Style::DEFAULT,
    opacity: bars.opacity(now_ms),
    on_drag: on_thumb_drag,                       // Rc<dyn Fn(Axis, bool, &mut Window, &mut App)>
};
let element = bar.render(cx);
```

| 项 | 契约 |
| --- | --- |
| `ScrollTarget::viewport_bounds` | 内容视口（不是整块区域）的四边，供几何与拖动坐标使用 |
| `ScrollTarget::offset` / `max_offset` | 负方向 offset / 非负 reach；与 GPUI 一致 |
| `ScrollTarget::set_axis_offset` | 仅设置指定轴，值已 clamp，另一轴不变 |
| `ScrollTarget::drag_started` / `drag_ended` | `ScrollHandle` 默认空实现；`ListState` 实现直接转调原生手势钩子 |
| `ScrollTarget::redraw` | 默认 `window.refresh()` |
| `Placement::BAND` | 标签条：无 inset、命中区等于线宽，画在预留 band 顶部 |
| `Placement::overlay(inset, hit)` | 内容区：贴尾侧（纵向在右、横向在下）浮层 |
| `Placement::clear_of(other, gap)` | 双轴同时显示时在角落让出对方轨道；长度按实际 track 计算，不是整视口 |

`Thumb::new` 的边界：视口或范围为 0、非有限值、track 过短（移动距离 ≤ 0）时返回 `None`，不绘制、不拦截输入；range 变小或窗口变大时 thumb 位置随 `-offset/reach` 重新 clamp，拖动中 range 变化以“按下时 thumb 比例 + 指针位移”重算，不会跳到端点外。长度计算只用 `viewport`、`reach`、`track` 三个标量，不遍历或测量列表行。

## 公共内容平滑（已实现）

`scroll::Motion<T>` 包装真实 `ScrollTarget + Clone`，横纵轴各有独立目标/呈现/时间。它不依赖 Shell、repo、固定 region 名或某一种句柄。`ScrollHandle` 两轴可用；UniformList 用公开 `base_handle`；`ListState` 只支持纵向，`supports_axis` 令横轴 wheel/写入/drag 安全 no-op，并保留其原生 `scrollbar_drag_started/ended`。不重建列表，也不全量 measure_all。

- 离散 wheel 累加目标，约 130ms `ease_out_cubic`；新输入从当前呈现位置继续，反向不会回到旧起点。范围收缩同时 clamp 存储目标；静止后 `tick` 返回 false，不启动 timer。
- precise Pointer（触控板像素）与 Touch（含 GPUI 自带动量）即时应用，不追加缓动。reduced-motion 同样直接落点；另一轴 offset 保留。
- `wheel_overlay(id, axis, motion)` 是统一的区域输入路径：Horizontal 优先原生 x，否则用 y；Vertical 反之；Shift+wheel 已由 Windows 提供的横轴 delta 不再转换。只有该区域有 reach 才消费，捕获阶段 stop propagation 防止 GPUI 内建 jump 把同一事件加第二遍。
- 透明输入层没有点击/拖动命中处理；将其放在相应 viewport 内、公共 Bar 之前。嵌套公共区域按内层优先消费一次；modal/独立原生输入面板用自己的 occluding hitbox 取得输入所有权，公共路径沿用 GPUI `should_handle_scroll`，不穿透该 hitbox。不要把整个页面的输入层跨过标签条、PAT 字段或其它独立滚动面板。
- `Bar.target` 与 wheel 必须使用**同一个 `Motion` 实例或其 clone**。thumb 的 start/write/end 转调原生目标 hook，同时取消旧 wheel 目标；thumb 拖动期间不积累本轴竞争 wheel。`Bars::settle` 处理外部释放，`cancel_region` 用于失活/卸载；不把所有 GPUI drag 都当本区域 thumb。
- reveal、键盘导航、路由/会话切换、排序边缘自动滚动、程序跳转先 `cancel()` 或调用 motion 的 `set_offset/set_axis_offset`，即时写原句柄。程序 reveal 可能早于新范围布局，不能用前一帧 max-offset 把新标签裁掉；新布局由原生 target clamp。直接修改原句柄的外部变化也会在 tick 被采用，但明确调用取消可避免小幅改动与旧目标竞争。
- owner 每帧用同一时间源 `tick(now, reduced)`，仅 true 时请求帧；关闭/失活/隐藏会话取消 motion。没有后台动画任务，最后一个 clone 释放即释放状态。内容 motion 与 `Bars` 的 140ms smoothstep fade 分开，各按自己的生命周期推进。

最小接线（真实 ScrollHandle、UniformList base_handle 或 ListState 均可作为 target）：

```rust
// owner 构造时保存，不能每帧重新创建。
let motion = scroll::Motion::new(real_handle.clone());
let mut bars = scroll::Bars::new();
// render: 一次采样，每帧仅在未完成时请求后续帧。
if motion.tick(now, reduced) { window.request_animation_frame(); }
// viewport 内：内容 -> wheel_overlay -> Bar，保持 Bar 的 thumb 命中。
let input = scroll::wheel_overlay("reader-y", Axis::Vertical, motion.clone());
// Bar { target: &motion, ... }；on_drag 调 bars.drag(&motion, axis, active, now_ms, reduced)。
// 程序化 reveal / 键盘跳转之前：
motion.cancel();
// 失活/关闭时：
scroll::cancel_region(&mut bars, &motion);
```

`wheel_overlay_with(id, WheelAxes, motion, now)` 在同一公共输入路径上提供明确轴归属和 owner 时钟。`Mapped(axis)` 保留原有单轴消费者的主轴/备用轴映射；`Both` 保留原生 x/y，Shift 仅在没有原生 x 时将 y 交给横轴；`Vertical` 不把 x 转成 y，并阻止原生纵向列表把 Shift+y 再当纵向输入。双轴 precise 输入在同一事件中各应用一次。旧 `wheel_overlay` 接口保持不变。账户消费者的后续迁移由 account-shell 独立交付，本任务不修改其它 worktree。

## 动画与输入边界（已实现）

- 只在 `Bars::is_animating()` 为真时请求动画帧；静止后没有常驻 timer。
- 指针离开区域淡出；本区域 thumb 拖动期间保持可见，平台 drag 结束（含在区域外释放）后回到真实 hover。
- 窗口失活（`observe_window_activation`）清理标签重排与 thumb drag，并立即清除 hover/可见性。
- 完全隐藏时不构建任何元素，所以不会拦截点击、文本拖选或链接命中。

## 消费者接入

### 已接入：应用标签条

`Shell::tab_strip` 用 `Placement::BAND` + `Style::DEFAULT` 渲染标签条滚动条，`Bars` 由 `Shell` 持有；`hover` 来自标签条本身，`hold` 来自标签重排，`drag` 来自本区域拖动。滚轮/`Shift`+wheel 的轴映射保留，离散输入现走同一公共 Motion；边缘自动滚动、标签排序/关闭/切换、会话独立 scroll/focus、固定 toolbar 与 Refresh 间距均未改变。

### 已接入：sidebar-layout 仓库 viewport

`Shell::repo_bars` 独立持有仓库组的显隐与 thumb drag；hover 绑定真实 `repo-scroll-region`，纵向 `BarId` 为 `repo-scrollbar`，使用 `Style::DEFAULT` 与 `Placement::overlay(4, 8)`。条在透明 wheel overlay 之后绘制，不改变列表行高、账号/footer 的位置。固定导航、账号、More 和底部四入口不参与滚动或共享该 `Bars`。

目标是 `scroll::Motion<ScrollHandle>` 的 `ScrollTarget` 适配，所有 viewport/offset/max-offset 与写入均转到原来的真实 `repo_scroll`。`drag_started`、每次 `set_axis_offset`、`drag_ended` 取消旧的 130ms wheel 目标；thumb 拖动期间 wheel 不建立竞争目标，释放后不弹回。内容平滑与 tabs/page/palette 共用同一公共实现，保留 precise Pointer / Touch 直通。

外部释放只 settle 仓库 `Bars` 已记录的拖动；Esc、窗口失活和尺寸变化会清理仓库条与平滑目标，tabs 的重排/横轴拖动不被当成仓库 thumb drag。resizer sash 仍是独立的分界高亮，不纳入滚动条。组合测试覆盖真实内容拖动、待完成 wheel 的取消、区域隔离、快速 hover 反向、收起/矮窗口与清理；最终外观与真机手感仍待负责人验收。

### 已接入：content-reader（源码 / Markdown）

PR #27 已合并。源码视图使用真实 `UniformListScrollHandle` 的 `base_handle`，Markdown 使用本地 `MarkdownView::list_state()` 的真实 `ListState`；两者通过 `Bar` / `Bars` 共用外观、几何、拖动与 Fade，没有额外自绘 scrollbar。源码视图保留双轴及右下角避让，Markdown 的 `ListState` 适配仅支持纵向。模块归属和验收边界见 [只读阅读器](content-reader.md#滚动条共享核心)。

本轮 reader-motion 将 Source 的真实 `base_handle` 包装为持久 `Motion<ScrollHandle>`，MarkdownView 的真实列表包装为持久 `Motion<ListState>`。Bar 与 wheel 使用同一实例；reader 的已有时钟同时驱动 wheel tick 和独立 fade，仅未完成时请求后续帧。Source 的 Shift/原生 x 与 y 分别归属横纵轴，Markdown 只接纵轴。当前本地 Markdown 的 code/table 采用换行布局，没有内层横向 ScrollHandle；没有为迁移新增假目标或改变表格字号/代码样式。

thumb hook、程序 set-offset、选择按下、reset/adopt、正文替换、Esc、窗口失活/隐藏以及 keyed 元素状态释放会清理旧目标；真实 ListState 测量与 drag hooks 保留。宿主失效账户上下文时可调用兼容新增的 `ReaderView::cancel_interactions(cx)`。保留实体但不再渲染的宿主由 GPUI keyed-state release 清理，dispose 另有 release hook。GPUI 受控事件测试与本机原生预览的证据边界见 [本轮日志](devlog/2026-10-28-reader-motion.md)，最终手感仍须负责人验收。

## 验证现状

- 公共模块对真实 GPUI 测试窗口自建的探针覆盖：无溢出时不绘制且不拦截输入、纵向拖动改变真实 `UniformListScrollHandle::base_handle`、真实非均匀 `ListState` 的原生拖动与结束清理、双轴与双区域互不干扰、角落避让、拖动中 range 缩小仍 clamp、取消后清理、平台释放后只结束本区域拖动、`Placement` 与 `Style` 分离。
- 标签条既有关键用例（滚轮、`Shift`+wheel、reveal、拖拽排序、边缘自动滚动、关闭/切换、toolbar 固定、hover/拖动淡入淡出、外部释放淡出）全部保留并通过。
- 历史真实 Windows 窗口自测（2026-10-26 旧依赖 head，144 DPI，1942×1241 物理像素，11 个标签）：闲置 `(84,83,83)`、thumb hover `(120,119,119)` 对 `(44,44,44)` 背景，滑块 4 物理像素高且无实体轨道；一次真实拖动把滑块从 `1010..1871` 移到 `510..1371` 并移动标签条；移出后该行回到背景色。这些是旧 head 的自测证据，不代替迁移后 head 的预览，更不是负责人视觉验收。
- 迁移后 head（`683b69e`，官方 Ely 固定提交 + GPUI 0.3.10）本机 `fmt` / `check --locked --all-targets` / `clippy -D warnings` / `test --locked`（359 项）/ `build --locked` / `git diff --check` 全部通过；`cargo run --locked` 启动成功且 `PrintWindow` 取到真实渲染帧，但桌面处于锁屏、输入未送达，未取得迁移后 head 的交互像素。
- 2026-10-28：负责人已分别验收 sidebar-layout 与 content-reader。本文此前的上游列表接口阻塞已由本地 Markdown 组件解除；两者同步后的编译与测试记录在侧栏开发日志。真实 IME、多 DPI、触控板及正式主窗口中的端到端组合仍需综合验收。
