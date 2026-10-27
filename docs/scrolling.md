# 统一滚动与滚动条

更新：2026-10-27。状态：公共滚动条已实现并由应用标签条（workspace-tabs）实际接入；sidebar-layout 与 content-reader 仍各自保留旧实现，尚未接线。当前基线使用官方 Ely `ely-core`（固定 `30c66fe1d5d23db769a6c834b771d054d7f288cf`）与 GPUI / platform `0.3.10`。新版 GPUI `ListState` 已有真实适配和测试；`MarkdownDocument` 的私有内层列表仍不能从外部接入，见下文。

本主题是滚动条外观、几何、拖动、淡入淡出与 `ScrollMotion` 边界的唯一维护位置。视觉基准来自 content-reader 当前实现（负责人 2026-10-26 明确以它为准）：细、两头圆、半透明、出现/消失渐变、无实体轨道。

## 源码位置与职责

| 位置 | 职责 |
| --- | --- |
| [`src/ui/scroll/mod.rs`](../src/ui/scroll/mod.rs) | 唯一的外观（`Style`）、放置（`Placement`）、单帧绘制（`Bar`）与区域状态（`Bars`） |
| [`src/ui/scroll/geometry.rs`](../src/ui/scroll/geometry.rs) | 唯一的 thumb 长度/位置/拖动换算（`Thumb`） |
| [`src/ui/scroll/fade.rs`](../src/ui/scroll/fade.rs) | 唯一的淡入淡出状态（`Fade`） |
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
| GPUI `ListState` 原生 scrollbar API | **已直接复用** | `ScrollTarget for ListState` 使用公开的 `viewport_bounds`、`scroll_px_offset_for_scrollbar`、`max_offset_for_scrollbar`、`set_offset_from_scrollbar` 和 `scrollbar_drag_started/ended`。原生 adapter 是纵向，不把横向假称为已支持。 |
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

## 与滚轮平滑（ScrollMotion）的边界

本轮不改变标签条已验收的滚轮手感，也不启用任何全局惯性。未来 `ScrollMotion` 与滚动条保持职责独立：

- 滚轮/触控板：离散 wheel 走 130 ms ease-out，precise（高精度）delta 直通；同一帧内一次输入只消费一次，标签条的 `Shift`+wheel 与原生横向 delta 映射保持现状，不新增二次转换。
- 滚动条 thumb 拖动、区域 reveal 和程序化跳转必须即时生效，并在开始时取消在途平滑动画。`ScrollTarget::drag_started` / `drag_ended` 就是给 `ScrollMotion` 挂取消钩子的位置。
- 平滑状态与淡入淡出状态互相独立；动画结束即停止请求帧（`Fade::is_animating`），`App::reduce_motion()` 为真时渐变跳变、不请求帧。

## 动画与输入边界（已实现）

- 只在 `Bars::is_animating()` 为真时请求动画帧；静止后没有常驻 timer。
- 指针离开区域淡出；本区域 thumb 拖动期间保持可见，平台 drag 结束（含在区域外释放）后回到真实 hover。
- 窗口失活（`observe_window_activation`）清理标签重排与 thumb drag，并立即清除 hover/可见性。
- 完全隐藏时不构建任何元素，所以不会拦截点击、文本拖选或链接命中。

## 消费者接入

### 已接入：应用标签条

`Shell::tab_strip` 用 `Placement::BAND` + `Style::DEFAULT` 渲染标签条滚动条，`Bars` 由 `Shell` 持有；`hover` 来自标签条本身，`hold` 来自标签重排，`drag` 来自本区域拖动。原有滚轮/`Shift`+wheel 映射、边缘自动滚动、标签排序/关闭/切换、会话独立 scroll/focus、固定 toolbar 与 Refresh 间距均未改变。

### 未接入：sidebar-layout

侧栏仓库组目前是 `ScrollHandle` + 既有滚动。接入步骤：

1. 引用 `src/ui/scroll`，不要复制几何或拖动。
2. 侧栏持有自己的 `Bars`，`hover` 绑定仓库组区域，`drag` 回调走 `Bars::drag(&handle, Axis::Vertical, …)`。
3. 用 `Placement::overlay(inset, hit)` 贴在仓库组右侧；`inset` 与侧栏内边距保持一致，不要挤压已验收行高。
4. 桌面其余固定行不参与滚动，也不共享该 `Bars`。
5. 侧栏的 resizer 是分界线高亮，不是滚动条，不纳入本模块。

### 未接入：content-reader（源码 / Markdown）

源码视图已有真实的 `UniformListScrollHandle`。后续消费者任务按以下清单接线（本 PR 不代替它完成）：

1. 统一先使用官方 `ely-core` 与 GPUI / platform 0.3.10，事件使用 `ScrollEvent`，普通鼠标/触控板测试来源明确为 `ScrollSource::Pointer`。
2. 区域持有自己的 `Bars`，纵向与横向各一个 `BarId`（同一 owner、不同轴与名字）。
3. 用 `handle.0.borrow().base_handle.clone()` 作为 `ScrollTarget`；不要建立外层空 `ScrollHandle` 伪造位置，也不要重建列表或 `measure_all` 长文。
4. 纵向用 `Placement::overlay`，横向用 `Placement::overlay(...).clear_of(&vertical, gap)` 让出右下角。
5. 删除 `reader/scrollbar.rs` 的独立 geometry/drag 与 `ui/scrollbar.rs` 的 re-export，改指向本模块。

如果消费者使用 GPUI 非均匀 `ListState`，可直接把该状态作为 `ScrollTarget`：本模块已实现纵向 offset/max offset、原生拖动开始/结束和内容高度冻结。`native_list_tests::native_list_state_drags_and_uses_its_drag_hooks` 用真实窗口和真实列表验证了它，不是理论适配。该实现不支持横向；需要横向的 Source 应继续使用 `UniformListScrollHandle::base_handle`。

**Markdown 的真实限制（固定新版源码复核）**：新版 `MarkdownDocument` 自带 `Scrollbar::auto()`，在 Preview/Visual 模式内部使用 `ScrollSource::List(self.list.clone())`，Source 模式使用编辑器的 `ScrollHandle`。但其私有 `list: ListState` 没有公开 accessor；上游 `ScrollModel` 虽允许自定义模型，也不能从外部取得这个列表的 viewport/offset/max offset 来构造模型。因此仅增加 `ely-documents` 依赖或调用 `ScrollModel` 不能把本模块接到 Markdown 内部列表，仍需一个公开最小 facade/accessor（另立任务讨论，不在本 PR 修改 Ely）。新版的 `ListState` 纵向原生 API 已就绪，但不能绕过该私有字段。

## 验证现状

- 公共模块对真实 GPUI 测试窗口自建的探针覆盖：无溢出时不绘制且不拦截输入、纵向拖动改变真实 `UniformListScrollHandle::base_handle`、真实非均匀 `ListState` 的原生拖动与结束清理、双轴与双区域互不干扰、角落避让、拖动中 range 缩小仍 clamp、取消后清理、平台释放后只结束本区域拖动、`Placement` 与 `Style` 分离。
- 标签条既有关键用例（滚轮、`Shift`+wheel、reveal、拖拽排序、边缘自动滚动、关闭/切换、toolbar 固定、hover/拖动淡入淡出、外部释放淡出）全部保留并通过。
- 历史真实 Windows 窗口自测（2026-10-26 旧依赖 head，144 DPI，1942×1241 物理像素，11 个标签）：闲置 `(84,83,83)`、thumb hover `(120,119,119)` 对 `(44,44,44)` 背景，滑块 4 物理像素高且无实体轨道；一次真实拖动把滑块从 `1010..1871` 移到 `510..1371` 并移动标签条；移出后该行回到背景色。这些是旧 head 的自测证据，不代替迁移后 head 的预览，更不是负责人视觉验收。
- 未覆盖：负责人对真实 Windows 窗口外观/DPI 的观感确认与动画流畅度、真实 IME 与触控板手势、sidebar 与 reader 接线后的组合效果、Markdown 列表（接口未就绪）。
