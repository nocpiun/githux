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
- `Shell::close_tab` 关闭被拖拽的 tab 时（如 Ctrl+W）也调用 `cx.stop_active_drag(window)`：关闭路径原本直接清 `self.tab_drag`，不结束平台拖拽，ghost 会继续跟随一个已被删除的 tab。
- 源 tab 仍用 `.when(dragging, |tab| tab.opacity(0.45))` 保留为源占位，drop marker 逻辑、落点算术、边缘自动滚动、owner guard 与关闭/激活行为全部未改动。

## 验证

聚焦与回归（均通过共享 runner `node scripts/verify.mjs -- cargo ...`，每次运行落盘 `.agents/task/runs/<id>.json`）：

- `cargo check --locked --all-targets`：通过。
- `cargo clippy --locked --all-targets -- -D warnings`：通过。
- `cargo fmt --all -- --check`：通过。
- `cargo test --locked --bin githux`：1227 项全部通过，含既有 tab 拖拽/排序/取消/边缘滚动回归与新增 `a_drag_shows_a_ghost_that_follows_the_grab_offset_and_clears`、`closing_a_dragged_tab_clears_the_ghost`。
- 新增回归断言：拖拽前无 ghost；拖拽中 ghost 尺寸等于 `TAB_WIDTH × TAB_HEIGHT`、指针保持在 ghost 内、指针移动 40px 时 ghost 同步移动 40px（固定抓取偏移）；drop 到最后一个 tab 之后顺序变为 `[home, b, a]` 且 ghost 消失；Escape 取消（真实 `CancelTabDrag` action）后 ghost 消失、顺序不变；关闭被拖拽的 tab 后 ghost 立即消失、顺序不残留旧索引。

未覆盖/边界（不宣称已做视觉验收）：GPUI 测试平台不驱动 Windows 原生光标字形、真实指针抓取/手感与窗口失焦下的平台拖拽结束；这些属于负责人手动验收范围。光标为“默认箭头”的断言依据是 GPUI 0.3.10 源码：drag 光标样式取自拖起元素的 `mouse_cursor` 并由 `Window::draw` 在 drag 视图绘制时写入窗口；本实现通过 `set_active_drag_cursor_style(Arrow)` 显式覆盖，手势结束后 `stop_active_drag`/mouse-up 路径会清空。

## 后续

2026-11-11：负责人已验收正常拖拽画面并授权合并（[协调者交接](https://github.com/nocpiun/githux/pull/80#issuecomment-6787852285)），本轮继续处理账户切换的正确性修复，不要求重复视觉验收。最终合并证据保存于 PR 与任务 delivery-state。

## 2026-11-11：账户发布时清理真实拖拽

- PR #80 机械 rebase 到 `328960d`，当前 main 基线 `8cad76b`；已比对三个已验收 UI 文件 blob 均未因 rebase 改变。本地旧 head `6d53195` 与既有 runs/review 证据保留。
- [审查 P2](https://github.com/nocpiun/githux/pull/80#discussion_r4534304077) 成立：`Accounts` 异步发布触发 `Shell::sync_accounts` → `cancel_account_scoped_view_state`，此前只清 Shell reorder，GPUI payload/ghost/光标仍携带旧账户标题图标。
- 账户清理现在取出该 Shell 当前 payload 的存活令牌，再用保存的 Window handle 在 observer 后的 effect 中停止平台拖拽。`TabDrag::drop` 永久失效自己的令牌，故旧清理即使晚到也不能停止后来或其他窗口的拖拽。未持有本 Shell payload 的账户发布不取消全局其他拖拽。契约见 [正式页面生命周期](../shell-page-integration.md#页面生命周期)。
- 新增 `account_publication_clears_the_live_tab_ghost`：真实 `Accounts::notify` 走正式 observer/`sync_accounts`，指针输入先建立实际 GPUI ghost；A→B 清除平台 drag、cursor style 和 ghost，B 只有自己的 Home，不含 A 的 tab；B→A 恢复原会话而不恢复 ghost；另开真实拖拽后投递旧 payload 的清理，确认新 ghost/平台 drag 保留。
- 验证范围：账户 snapshot 使用既有测试 fixture setter 注入；账户投影、observer、workspace 与 GPUI 鼠标拖拽均为生产路径，不涉及真实 GitHub 登录写入或 Windows 原生输入。负责人此前的正常视觉验收保持有效。
- 聚焦回归通过：run `73e52e48`；反向验证临时省略平台清理后，run `fa9e3583` 在账户切换后的 `!cx.has_active_drag()` 断言失败，确认用例能捕获原缺陷；随后恢复修复。重跑全套的理由为新 main 基线组合及本轮实际生命周期改动。
- 最终检查：fmt check、all-targets clippy `-D warnings` 通过；完整 `cargo test --locked --no-fail-fast` 的 bin 1231、Home 98、Profile 287、Reader 252、Reader images 781、Repository 877、Overview 44 全部通过。独立 Notifications target（不包含 Shell/tabs）并发 fixture 两次分别失败：首次 localhost bounded-body 请求返回 Network 而非 ResponseTooLarge（`ec33f86c`），隔离重跑通过（`b31403be`）；完整重跑时该用例通过，但 cache reopen 用例返回 InUse（`0e149e7d`）。保留原始失败；Notifications 串行检查 589/589 通过（`334076a0`），native `cargo build --locked` 通过（`c819f8df`）。没有修改非本任务网络/缓存代码。
- 可复用教训候选：预览加入可见内容后，应审计所有清除宿主交互状态的路径；异步生命周期清理必须绑定具体手势身份，不能仅检查“当前有拖拽”。本次不并发修改共享 LESSONS。
