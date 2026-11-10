# 修复：拖动会话标签时的默认箭头光标与跟随指针的 ghost 预览

- 日期：2026-11-10；任务 ID：tab-drag-ghost；执行者：基层 Agent；base 0c61907。
- 目标与状态：实现中 → 待审查（实现、聚焦测试与文档完成，PR 待创建/合并按工作流）。

## 改动与决定

问题：`src/ui/tabs.rs` 的 `on_drag` 在开始重排后返回 `gpui::EmptyView`，因此拖动时 GPUI 的 drag view 无内容、指针下没有 ghost；同时 drag 光标样式来自拖起 tab 元素自身的 `cursor_pointer`，拖动中显示为手指而非默认箭头。

修复（src/ui/tabs.rs / src/ui/mod.rs / src/ui/tests.rs）：

- 拖起 tab 的 `on_drag` 构造 `TabGhost`（`impl Render`）：固定 `TAB_WIDTH × TAB_HEIGHT`、与源 tab 相同的 `pl/pr/gap/rounded`、图标 + `Ellipsis` 标签 + 保留的关闭按钮空槽，用 `surface` 背景、`border` 描边与一个投影区分“被拎起”。它不注册任何命中/拖放/悬浮处理，只参与绘制，因此不会截获 drop；GPUI 在 `pointer - cursor_offset` 处绘制，天然保持按下时的抓取偏移。
- 拖起 tab 的 `on_drag` 回调里 `window.defer` 后 `cx.set_active_drag_cursor_style(CursorStyle::Arrow, window)`：拖拽光标在整个手势期间保持默认箭头，手势结束随 GPUI 的 `active_drag` 清除。源 tab 保留 `cursor_pointer`（按下前仍是手指），所以无需把 `Arrow` 写进源元素的 `cursor` 样式。
- `TabDrag` payload 增加 `title`/`icon` 快照：ghost 显示按下时刻的标签与图标，不随拖拽中途的路由标题变化而变。
- `Shell::finish_tab_drag`（Escape/取消、失焦、关闭、渲染循环发现平台拖拽结束）在 `tab_drag.take()` 之外调用 `cx.stop_active_drag(window)`，让 Escape 取消或失焦时 GPUI 的平台拖拽（及其 ghost）也被结束，避免残留 ghost。这是本次修复的真实缺陷：旧实现只清 shell 状态，GPUI 的 `active_drag` 仍活到鼠标释放，导致取消后 ghost 残留。
- 源 tab 仍用 `.when(dragging, |tab| tab.opacity(0.45))` 保留为源占位，drop marker 逻辑、落点算术、边缘自动滚动、owner guard 与关闭/激活行为全部未改动。

## 验证

聚焦与回归（均通过共享 runner `node scripts/verify.mjs -- cargo ...`，每次运行落盘 `.agents/task/runs/<id>.json`）：

- `cargo check --locked --all-targets`：通过。
- `cargo clippy --locked --all-targets -- -D warnings`：通过。
- `cargo fmt --all -- --check`：通过。
- `cargo test --locked --bin githux drag`：42 项全部通过，含既有 tab 拖拽/排序/取消/边缘滚动回归与新增 `a_drag_shows_a_ghost_that_follows_the_grab_offset_and_clears`。
- 新增回归断言：拖拽前无 ghost；拖拽中 ghost 尺寸等于 `TAB_WIDTH × TAB_HEIGHT`、指针保持在 ghost 内、指针移动 40px 时 ghost 同步移动 40px（固定抓取偏移）；drop 到最后一个 tab 之后顺序变为 `[home, b, a]` 且 ghost 消失；Escape 取消（真实 `CancelTabDrag` action）后 ghost 消失、顺序不变。

未覆盖/边界（不宣称已做视觉验收）：GPUI 测试平台不驱动 Windows 原生光标字形、真实指针抓取/手感与窗口失焦下的平台拖拽结束；这些属于负责人手动验收范围。光标为“默认箭头”的断言依据是 GPUI 0.3.10 源码：drag 光标样式取自拖起元素的 `mouse_cursor` 并由 `Window::draw` 在 drag 视图绘制时写入窗口；本实现通过 `set_active_drag_cursor_style(Arrow)` 显式覆盖，手势结束后 `stop_active_drag`/mouse-up 路径会清空。

## 后续

- 创建 PR（目标 main，关联 First Release milestone #1，加 `pending` 标签），走 githux-pr-review 等待最新提交的 CI 与 Codex 审查；不自行合并。
- 负责人手动验收：真实窗口拖动时光标为默认箭头、ghost 跟随与图标/标签正确、取消/窗外释放无残留 ghost/光标。
