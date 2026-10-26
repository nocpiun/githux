# 侧边栏布局与交互

2026-10-25 sidebar-layout 实现，含负责人真实窗口反馈后的两轮修订。负责人要求真实窗口验收后再合并；当前实现及测试不构成人工视觉签字。

侧栏默认展开 232 logical px，可拖动到 200–360px；上限还受窗口剩余 island 至少 600px 的预算约束（880px 窗口最多 265px）。收起宽 56px，由 28px 共同方形控件尺度加 8px 左内缩组成（右侧由 island gap 补齐）。展开/收起用约 200ms ease-out 插值呈现宽度，sidebar、island 左边界、resize 命中带与圆按钮共用同一帧值；快速反向从当前呈现值继续，手动拖动和窗口变化中断动画，终点恢复本次运行最后选择的宽度并按当前窗口限制，遵循现有 reduced-motion。宽度、收起状态和滚动仅属 `Shell` 的窗口局部状态，没有磁盘偏好或账户数据变更。参数集中在 `src/ui/geometry.rs` 的 SIDEBAR/REPO/ACCOUNT/INDICATOR 常量，状态与限制在 `src/ui/sidebar_state.rs`。

侧栏背景、字体及控件使用 Ely 中性色主题。行背景从客户区左缘内缩 8px；行右侧与 island 间的 8px gap 补齐对称外留白。底部入口距离客户区底部也是 8px。仓库 viewport 与固定 footer 之间用一条 1px、低对比度 `colors.border` 细线分隔；线自带上下对称留白（各 8px，`FOOTER_DIVIDER_SPACE`），收起/展开两态视觉等距，不随仓库滚动。展开时账号头像用 Ely Md（32px），行高 48px；仓库行内左 padding 为 4px。收起时 Home/Settings、仓库、账号和 More 使用同一 28px 方形命中背景并居中，账号头像与仓库同用 Ely Xs（20px）；展开过程连续插值头像与行高，不留下宽矮矩形或高大账号块。Profile 页面中的 Ely 大头像不因此改变。头像和长标识在有界行内显示，完整名字保留为 tooltip/可访问名称。

仓库标题精确为 `Repositories (N)`，括号内是当前列表长度（`Shell::repositories().len()`），不使用中点分隔，不硬编码；收起态隐藏标题的行为保留。

Home/Settings、仓库和 Profile 的活动行从客户区可绘制左缘 x=0 开始绘制 3px 宽中性指示条：左上/左下圆角严格为 0，只有右上/右下 4px 小圆角；高度为行高的约 2/3（`SIDEBAR_INDICATOR_HEIGHT_RATIO`），并 clamp 到 20–24px，垂直居中，因此随当前行（含动画中间高度）变化。它处在行背景外，没有交互命中区，不覆盖窗口 resize 规则。仓库指示线与行一起在独立 viewport 内滚动及裁剪。Ely NavItem 无关闭内建 accent indicator 的 API，侧栏模块因此按原控件的主题与键盘行为组合行，避免双线；四个全局入口仍使用自身活动态。

账号区不是整行 button：外层 `account-row` 不 hover 点亮、不响应空白右侧；只有头像 + 显示名 + `@ID` 组成的紧凑身份控件（`account-identity`）打开 Profile，保留可访问名称、Tab 焦点与 Enter/Space。身份控件本身也没有整块 hover 背景：悬停时只显示 pointer 光标，并让 `@ID` 出现下划线。展开时右侧另有独立 emoji 状态按钮（占位示例 💻 / `Coding`），hover tooltip 为 `Coding`，点击不导航 Profile 且停止冒泡；本轮无编辑弹窗、无网络、无写状态。收起时状态按钮隐藏，状态文字并入头像 tooltip。账号外层活动 indicator 仍按上面的贴边规则，不把状态操作标成 Profile 点击。

标题栏以下分界区域垂直中央的 24px 圆按钮负责收起/展开，其水平中心落在 island 的左边框线（`width + ISLAND_GAP + ISLAND_BORDER_WIDTH/2`），垂直中心对齐 island 自身高度中点（扣除底部 inset）；两个轴都从与 island 相同的几何常量推导，动画、resize、最大化和窗口化都跟随。分界 gap 提供 8px 横向 resize 命中带，无需拖圆按钮；热区 hover 或拖动时显示 2px 前景色 sash 竖线（约 140ms ease-out 淡入淡出，可逆续接，遵循 reduced-motion），纵向只在 island 可见圆角/inset 之内，不铺满侧栏，也不遮挡中部 toggle。GPUI drag 捕获负责跨命中带拖动，最大/最小 clamp、释放、Esc、失焦和窗口尺寸变化终止拖动。圆按钮独立命中，不启动 resize；拖动不改变路由。原生窗口边缘缩放和标题栏实现没有改变。

收起时 Home/Settings 保留图标，仓库保留 namespace avatar，账号保留头像；标题、计数、姓名和 ID 隐藏，完整 tooltip/可访问名仍在。仓库区域独立滚动，键盘到达屏外仓库时沿用 `reveal_row` 最小滚动（并取消在途滚轮缓动）。收起/展开不导航、不重置 repo/page 滚动。

底部四入口（展开态）各自使用一条低对比度 Ely 中性色细边框，保持横向四等分、间距与圆角一致，不抢眼。收起模式使用 Ely OverflowMenu，按钮 tooltip 与可访问名称均为 “More”；菜单内仍有 Notifications、Issues、PRs、New，各有图标与标签，当前全局路由在菜单项上标记，New 继续 disabled。Ely 负责鼠标、方向键、Enter/Space、Esc、浮层窗口避让和焦点交还。侧栏专用透明 dismissal shield 阻止外点关闭同时点击背后页面。窗口 resize/maximize/restore 时通过焦点离开关闭菜单；展开时菜单节点离树并回到壳层焦点。没有引入创建资源动作或新页面。

## 平滑滚动

`src/ui/scrolling.rs` 提供最小共享滚动适配：离散鼠标滚轮累加进目标偏移，按真实时间用 Ely `ease_out_cubic`（约 130ms）缓动；触控板/高精度像素滚动直接应用，不做二次惯性；边界 clamp 到 GPUI 的 `[-max, 0]`，内容变短时同样收敛；无动画时不请求帧，遵循 reduced-motion。透明 `wheel_overlay` 在捕获阶段拦截指针所在区域，只消费一次并抑制 GPUI 内建的逐事件跳变；键盘 reveal、路由重置和 palette 光标移动会取消在途缓动。

已接入区域：侧栏仓库 viewport、island 页面 viewport、palette 结果列表。每个区域各有独立 `SmoothScroll`/`ScrollHandle`，互不串扰。拖动滚动条 thumb 仍由 `set_offset` 直接映射，下一次 tick 会识别外部偏移变化并取消缓动；嵌套区域只消费一次。未合并的 workspace-tabs 横向滚动不在本分支，未宣称覆盖，其接入接口就是同一个 `wheel_overlay` + `SmoothScroll`。

验证与预览边界见 [开发日志](devlog/2026-10-25-sidebar-layout.md)。多标签任务 PR #16 与本任务独立开发，本分支不包含或复制其未验收代码；集成冲突和组合验证由主 Agent 协调。
