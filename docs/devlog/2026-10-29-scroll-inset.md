# 统一滚动条尾侧间隙修复（scroll-inset）

- 日期：2026-10-29；任务 ID：scroll-inset；执行者：基层 Agent。PR/Issue 链接在创建后补于 PR 记录。
- 目标与状态：修复统一 scrollbar 可见线离容器尾侧边缘偏远（图六 Shell 仓库侧栏约 6.5 logical px），保持细、圆端、半透明、淡入淡出与可用的命中区；**实现与本地验证完成，进入 PR/审查流程**。

## 改动与决定

触发条件：`Placement::overlay(inset, hit)` 原先把 3 px 可见线**居中**放进 `hit` 宽的命中条，再整体内缩 `inset`，因此实际可见间隙为 `inset + (hit - THICKNESS) / 2`。侧栏、树、Profile、Markdown/source、Home 等统一使用 `overlay(4, 8)`，可见间隙 6.5 px；Profile heatmap 用 `overlay(3, 8)`，为 5.5 px。

结果行为：把 `inset` 定义为**可见线到尾侧边缘的间隙**，命中条从尾侧边缘向容器**内部**展开（`Edge::End` 用 `right_0`/`bottom_0` 定位 track，可见线再用 `right(inset)`/`bottom(inset)` 定位）。因此：

- 所有内容区调用点改为 `overlay(1.5, 8.0)`：可见间隙 1.5 px，命中条仍为完整 8 px（`right-8..right` / `bottom-8..bottom`），可见线位于内侧 1.5–4.5 px。
- `Placement::BAND`（标签条）保持 `inset = 0`、`hit = THICKNESS`、`Edge::Start`，可见线与命中区都贴 band 起点，标签条专属布局不变。
- `Placement::clear_of(other, gap)` 按新的命中轨道重算角落避让，并只统计一次双方共享的尾侧间隙；双轴 corner 仍保留 2 px。
- `Edge::End` 的命中条宽度取 `max(hit, inset + thickness)`，保证更大的 `inset` 也不会让可见线落在命中条之外。

主要维护位置与完整调用点审计见 [统一滚动](../scrolling.md#尾侧间隙审计2026-10-29)。未改 Shell indicator、账户逻辑、Profile 布局、Reader 的 layout/image/HTML 实现、Ely/GPUI 版本或 Cargo.lock。

## 验证

- 代码/几何回归：`src/ui/scroll/tests.rs` 新增 `visual_gap_is_independent_of_the_inward_hit_area`，断言可见线到区域尾侧为 1.5 px、命中条为 8 px 且贴边，并按在命中条内侧（可见线之外）仍能真实拖动纵/横轴；`the_two_tracks_do_not_meet_in_the_corner` 更新为新的 corner 期望。
- 原生 Shell 等价预览：本地一次性开发者 example（不进入提交、不读真实账户/PAT）用真实 `Shell` + 合成 `/user` `/user/repos`（80 行）运行；144 DPI（150%）下逐列测量 `repo-scrollbar`，修复前 10 物理 px（约 6.67 逻辑 px），修复后 2 物理 px（约 1.33 逻辑 px，标称 1.5）；真实 thumb 拖动使滑块下移约 120 物理 px，wheel 继续移动，hover 淡入正常；两个显示器结果一致。截图与脚本在本地 `.agents/output/`、`.agents/task/`（均被忽略）。
- 100% 逻辑尺度由测试平台断言（1.5 px）；本机两个显示器均为 144 DPI，没有第二个原生 DPI 可比，已在文档中标注。
- 未覆盖：真实触控板惯性、真实 IME、macOS/Linux 原生窗口；`cargo test` 使用 GPUI 测试平台，不替代原生输入手感验收。

## 后续

- 需负责人手动验收：已加 `pending`，不自动合并。
- 若审查要求进一步收紧其他调用点的数值，按本文件的可见间隙/hitbox 语义修改，不改变标签条 band。

## 可复用教训候选（交协调者汇入 LESSONS，本任务不直接写）

- **浮层“边距”参数必须区分可见线与命中区**：当命中区比可见线宽时，若把 `inset` 实现为“命中条整体内缩”，可见线会被再居中推远 `(hit-thickness)/2`，测量出的间隙与配置值不一致。把 `inset` 定义为可见线到尾侧边缘的间隙、命中条从边缘向内展开，能让配置值等于可见结果，同时保留命中区；审查 UI 间距时同时量可见元素与 hitbox，不能只看 CSS/布局参数。证据：本轮 `overlay(4,8)` 实际 6.5 逻辑 px，改成可见间隙语义后为 1.5 px；原生像素测量与单测一致。
