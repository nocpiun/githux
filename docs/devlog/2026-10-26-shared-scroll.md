# 统一滚动条：公共组件与 workspace-tabs 迁移

- 日期：2026-10-26；任务 ID：shared-scroll；执行者：基层 Agent。
- 目标与状态：把散落在 workspace-tabs 与 content-reader 分支里的滚动条几何、拖动与淡入淡出收拢成项目内唯一实现，并以负责人指定的 reader 视觉标准接入 main 上已合并的标签条。**实现与本地验证完成，进入 PR 流程**；sidebar-layout / content-reader 的消费者接线由各自任务后续进行。
- 交付范围：`src/ui/scroll/**`（新）、`src/ui/tab_scrollbar.rs`（删除）、`src/ui/mod.rs` 最小接线、`src/ui/geometry.rs` 常量归属调整、对应 GPUI 测试、`docs/scrolling.md`。不改 Ely/GPUI/Rust 版本、不改 Cargo 缓存、不修改 sidebar 与 reader 的 worktree。
- 基线：`84b029e63b9ca28dfc9d1abf7d8aed7795684c0e`（main，含 PR #16 workspace-tabs）。

## 改动与决定

**唯一实现。** `src/ui/scroll/` 分成四个小文件：`mod.rs` 放 `Style`（唯一视觉参数）、`Placement`（放置，与样式分开）、单帧绘制 `Bar` 与区域状态 `Bars`；`geometry.rs` 放 `Thumb`；`fade.rs` 放 `Fade`（140 ms smoothstep、反向从已绘制值继续、无常驻 timer、`reduce_motion` 时跳变）；`target.rs` 放 `ScrollTarget` 契约与 `gpui::ScrollHandle` 实现。`src/ui/tab_scrollbar.rs` 整份删除，没有保留转发或第二份绘制/拖动/Fade。标签条自身的 `TAB_STRIP_INSET`/`TAB_HEIGHT`/`TAB_SCROLLBAR_BAND`/`TAB_TOOLBAR_WIDTH` 仍属 `tabs`/`geometry`，不再有 `TAB_SCROLLBAR_THICKNESS` 与 `TAB_SCROLLBAR_FADE_MS`：band 由 `scroll::THICKNESS + 2` 推出，视觉参数只有一处。

**视觉。** 采用负责人指定的 reader 标准并取代旧标签条参数：3 logical px、`rounded_full`、无实体轨道、`fg` 0.20（闲置）/0.38（thumb hover）、140 ms 渐变、最小 thumb 24 px。旧 tabs 的 `min_thumb` 来自 Ely 主题（远大于线宽），本轮统一为 24，因此长标签条上的 thumb 更短、可辨但仍是细线。预留 band 仍为 5 px，所以标签位置、高度、toolbar 与 Refresh 间距没有任何变化；没有把 reader 的 8 px track 盒子套到 5 px band 上。

**目标适配。** `ScrollTarget` 覆盖 viewport bounds、当前 offset、max offset、指定轴写入、drag begin/end 与重绘。实现只依赖 `gpui::ScrollHandle` 的公开方法；`UniformListScrollHandle::base_handle` 是公开字段，可直接作为目标，`ListState` 的 `scrollbar_drag_started/ended` 通过包装转调即可（未在本轮接线）。`set_axis_offset` 一行改指定轴，另一轴原样保留——修掉了旧 tabs 拖动把 `y` 归零的写法。

**横纵共用与身份。** 几何、绘制、拖动只写一次，`Axis` 决定长边方向；拖动身份是 `(owner, region name, axis)` 三元组。同一 owner 下纵向与横向、或两个不同区域都不会互相驱动。区域各自持有 `Bars`，没有全局 hover/drag 单例。

**拖动与可见性。** thumb 拖动通过 `Bars::drag` 进入可见状态（并转调目标的 `drag_started`/`drag_ended`），平台 drag 结束（含在区域外释放）由渲染循环 `Bars::settle` 清理并回到真实 hover；标签重排改用 `Bars::hold`，所以“所有类型的 drag”不会被误认成 thumb 拖动。窗口失活通过 `Shell` 的 `observe_window_activation` 同时结束标签重排与 thumb drag，并清除可见性；关闭标签时也会释放 `hold`。

**几何边界。** `Thumb::new` 对视口/范围为 0、非有限输入、track 短到没有可移动距离都返回 `None`（不绘制、不拦截输入）；content 短于视口时不显示；range 缩小或窗口变大按 `-offset/reach` 重新 clamp；拖动中 range 变化用“按下时 thumb 比例 + 指针位移”重算，不会跳或越界。总长度只用三个标量，不遍历虚拟列表。

**placement 与样式分离。** `Placement::BAND` 给标签条（无 inset、命中区=线宽、贴 start 侧）；`Placement::overlay(inset, hit)` 给内容区（贴尾侧、命中区可宽于线宽）；`clear_of(other, gap)` 在双轴时让出角落，长度按实际 track（`viewport - 2*inset - corner`）算，不是整视口。`Band` 用今天唯一的消费者；overlay/clear_of 为已文档化的后续接入契约，带 `#[allow(dead_code)]` 并在注释里写明原因（L006 的最小范围做法）。

**未做。** 不改标签滚轮/`Shift`+wheel 手感、不新增全局惯性、不实现 `ScrollMotion`（只在 [统一滚动](../scrolling.md) 记录其与滚动条的边界）；不改 sidebar resizer；不改 reader 业务、Markdown/Ely 接口；不重建 Shell/workspace；不为调试入口改正式主程序。

## 验证

本机 Windows x86_64-pc-windows-msvc，工具链 1.98.1（`CARGO_BUILD_JOBS=1`）：

```sh
cargo fmt --all -- --check
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked          # 357 项
cargo build --locked
git diff --check
```

新增的公共模块测试（真实 GPUI 测试窗口自建探针，不复制业务页面）：

- `no_overflow_paints_nothing_and_leaves_the_content_clickable`：0 range 与短内容都不绘制，区域中心点击仍落到内容。
- `the_list_bar_binds_the_real_virtualized_handle`：竖直 thumb 3 px；拖动改变 `UniformListScrollHandle` 的真实 `base_handle` 并在 `max_offset` 内。
- `each_axis_and_region_moves_only_itself`：先拖列表纵轴只动列表；再拖宽区域纵轴只改 y；最后拖横轴只改 x 并保留 y。
- `the_two_tracks_do_not_meet_in_the_corner`：双轴 track 在角落互相让位，长度按实际 track。
- `a_shrinking_range_keeps_the_drag_inside_the_track`：拖动中内容变短后，后续 move 与释放都在新范围内。
- `cancelling_drops_the_drag_and_the_visibility`：取消后 drag 状态、动画与绘制全部清理。
- `only_the_region_that_was_released_loses_its_drag`：真实按下/移动/在区域外释放，只结束本区域 thumb drag。
- `style_and_placement_are_separate_from_the_band`：band 仍为 `3 + 2`，placement 不复用样式参数。
- `bars_report_drag_hooks_once_per_axis_and_cancel_clears_everything`：同一 `Bars` 只接受一个轴的拖动，start/end 各转发一次，`cancel` 清空可见性并补发 end；`set_axis_offset` 保留另一轴。
- `Fade`/`Thumb` 单元：反向连续、无空转帧、reduced-motion 跳变、退化输入、最小 thumb 与 clamp。

标签条既有关键用例全部保留并通过：滚轮与 `Shift`+wheel 只动标签条、reveal、真实鼠标拖拽排序与缝隙投放、边缘自动滚动（含指针静止）、中键关闭与左键/Enter 激活、toolbar 固定、hover 淡入淡出与反向连续、拖动 pin、外部释放后淡出、`close` 后 clamp、会话独立 scroll/focus。新增 `losing_window_activation_clears_both_drags` 与 `reduced_motion_shows_the_bar_without_animating`。

**真实 Windows 窗口的本次自测（不是负责人验收）：** 144 DPI、窗口 1942×1241 物理像素，打开 11 个标签让标签条溢出，逐项截图并取像素：

- 指针停在标签条上（未压住滑块）：滚动条带在标签行下方，滑块 4 物理像素高（`3 × 1.5` 取整）、`(84,83,83)` 对 `(44,44,44)` 岛内背景（= `fg` 20% 叠加）、长 861 像素贴右端、无实体轨道。
- 指针压住滑块：同一行像素变为 `(120,119,119)`（≈ `fg` 38%），即 thumb hover 加深生效。
- 真实鼠标拖动滑块 600 物理像素后松开：滑块从 `1010..1871` 移到 `510..1371`，标签条内容随之前移（首标签被裁切），页面与侧栏未动。
- 拖动后把指针移出标签条、以及未拖动时直接移出：对应行的四个像素都回到 `(44,44,44)`，即淡出到完全不绘制，没有留下透明轨道。

快速反向的中间帧、不同 DPI、以及缩放到 1200×800 后的采样没有取得可判读证据（该次截图指针落点不确定），按未覆盖处理。`Shift`+wheel 与触控板手势本轮未在真机重测。

**未覆盖 / 待真人验收（不据此宣布通过）：**

- 负责人对 3 px 线在 100%/150%/200% DPI 下的可辨度、hover 进出与快速反向、drag 结束后的淡出、不同窗口宽度下 thumb 比例的观感确认。自动化与本次截图只证明几何、状态、偏移和像素值，不证明观感与帧率。
- 触控板横向手势与真实 `Shift`+wheel 分发（沿用既有实现，未改逻辑，本轮未在真机重测）。
- sidebar / reader 接线后的组合效果与 Markdown 列表（接口未就绪，另立任务）。
- macOS/Linux 未做原生验收。

## 后续

1. 公共模块是唯一维护点：sidebar-layout 与 content-reader 只做接线，接口缺口回到本模块修补；不复制第三份滚动条。[统一滚动](../scrolling.md) 已写明两边的具体接入步骤。
2. Markdown 需要 `MarkdownDocument` 内部 `ListState` 的最小访问器/facade（且只有纵向），另立任务并走 Ely 接口决策，不在本轮实施补丁。
3. `ScrollMotion`（离散 wheel 130 ms ease-out、precise 直通、thumb drag/reveal/jump 即时并取消在途动画）只记录契约，未实现。
