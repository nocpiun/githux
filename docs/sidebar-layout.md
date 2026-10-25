# 侧边栏布局与交互

2026-10-25 sidebar-layout 实现。负责人要求真实窗口验收后再合并；当前实现及测试不构成人工视觉签字。

侧栏默认展开 232 logical px，可拖动到 200–360px；上限还受窗口剩余 island 至少 600px 的预算约束（880px 窗口最多 265px）。收起宽 56px，由 28px 共同方形控件尺度加 8px 左内缩组成（右侧由 island gap 补齐）。展开/收起用约 200ms ease-out 插值呈现宽度，sidebar、island 左边界、resize 命中带与圆按钮共用同一帧值；快速反向从当前呈现值继续，手动拖动和窗口变化中断动画，终点恢复本次运行最后选择的宽度并按当前窗口限制，遵循现有 reduced-motion。宽度、收起状态和滚动仅属 `Shell` 的窗口局部状态，没有磁盘偏好或账户数据变更。参数集中在 `src/ui/geometry.rs` 的 SIDEBAR/REPO/ACCOUNT 常量，状态与限制在 `src/ui/sidebar_state.rs`。

侧栏背景、字体及控件使用 Ely 中性色主题。行背景从客户区左缘内缩 8px；行右侧与 island 间的 8px gap 补齐对称外留白。底部入口距离客户区底部也是 8px。仓库 viewport 与账号行之间独立留 12px。展开时账号头像用 Ely Md（32px），行高 48px；仓库行内左 padding 为 4px。收起时 Home/Settings、仓库、账号和 More 使用同一 28px 方形命中背景并居中，账号头像与仓库同用 Ely Xs（20px）；展开过程连续插值头像与行高，不留下宽矮矩形或高大账号块。Profile 页面中的 Ely 大头像不因此改变。头像和长标识在有界行内显示，完整名字保留为 tooltip/可访问名称。

Home/Settings、仓库和 Profile 的活动行从客户区可绘制左缘 x=0 开始绘制 2px 宽中性指示条，左上/左下圆角为 0，只有右上/右下 2px 小圆角，上下各留 2px，因此高度跟随当前行（包括动画中间高度）。它处在行背景外，没有交互命中区，不覆盖窗口 resize 规则。仓库指示线与行一起在独立 viewport 内滚动及裁剪。Ely NavItem 无关闭内建 accent indicator 的 API，侧栏模块因此按原控件的主题与键盘行为组合行，避免双线；四个全局入口仍使用自身活动态。

标题栏以下分界区域垂直中央的 24px 圆按钮负责收起/展开，分界 gap 提供 8px 横向 resize 命中带；无需拖圆按钮。GPUI drag 捕获负责跨命中带拖动，最大/最小 clamp、释放、Esc、失焦和窗口尺寸变化终止拖动。圆按钮独立命中，不启动 resize；拖动不改变路由。原生窗口边缘缩放和标题栏实现没有改变。

收起时 Home/Settings 保留图标，仓库保留 namespace avatar，账号保留头像；标题、计数、姓名和 ID 隐藏，完整 tooltip/可访问名仍在。仓库区域独立滚动，键盘到达屏外仓库时沿用 `reveal_row` 最小滚动。收起/展开不导航、不重置 repo/page 滚动。

底部四入口在收起模式使用 Ely OverflowMenu，收起时按钮 tooltip 与可访问名称均为 “More”；菜单内仍有 Notifications、Issues、PRs、New，各有图标与标签，当前全局路由在菜单项上标记。New 继续 disabled。Ely 负责鼠标、方向键、Enter/Space、Esc、浮层窗口避让和焦点交还。侧栏专用透明 dismissal shield 阻止外点关闭同时点击背后页面。窗口 resize/maximize/restore 时通过焦点离开关闭菜单；展开时菜单节点离树并回到壳层焦点。展开模式四个图标保留各自 tooltip。没有引入创建资源动作或新页面。

验证与预览边界见 [开发日志](devlog/2026-10-25-sidebar-layout.md)。多标签任务 PR #16 与本任务独立开发，本分支不包含或复制其未验收代码；集成冲突和组合验证由主 Agent 协调。
