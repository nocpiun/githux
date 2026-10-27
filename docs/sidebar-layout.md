# 侧边栏布局与交互

2026-10-25 sidebar-layout 实现，含负责人真实窗口反馈后的两轮修订。负责人要求真实窗口验收后再合并；当前实现及测试不构成人工视觉签字。

侧栏默认展开 232 logical px，可拖动到 200–360px；上限还受窗口剩余 island 至少 600px 的预算约束（880px 窗口最多 265px）。收起宽 56px，由 28px 共同方形控件尺度加 8px 左内缩组成（右侧由 island gap 补齐）。展开/收起用约 200ms ease-out 插值呈现宽度，sidebar、island 左边界、resize 命中带与圆按钮共用同一帧值；快速反向从当前呈现值继续，手动拖动和窗口变化中断动画，终点恢复本次运行最后选择的宽度并按当前窗口限制，遵循现有 reduced-motion。宽度、收起状态和滚动仅属 `Shell` 的窗口局部状态，没有磁盘偏好或账户数据变更。参数集中在 `src/ui/geometry.rs` 的 SIDEBAR/REPO/ACCOUNT/INDICATOR 常量，状态与限制在 `src/ui/sidebar_state.rs`。

侧栏背景、字体及控件使用 Ely 中性色主题。行背景从客户区左缘内缩 8px；行右侧与 island 间的 8px gap 补齐对称外留白。底部入口距离客户区底部也是 8px。仓库 viewport 与固定 footer 之间用一条 1px、低对比度 `colors.border` 细线分隔；线自带上下对称留白（各 8px，`FOOTER_DIVIDER_SPACE`），收起/展开两态视觉等距，不随仓库滚动。水平方向与行背景同一 lane：距客户区左缘 8px、距 island 边仍是 gap，因此线段两端在“列宽 + island gap”的可见侧栏范围内左右对称；只在列宽内做 `px(8)` 双侧内缩会相对该可见范围左偏约 4px。展开时账号头像用 Ely Md（32px），行高 48px；仓库行内左 padding 为 4px。收起时 Home/Settings、仓库、账号和 More 使用同一 28px 方形命中背景并居中，账号头像与仓库同用 Ely Xs（20px）；展开过程连续插值头像与行高，不留下宽矮矩形或高大账号块。Profile 页面中的 Ely 大头像不因此改变。头像和长标识在有界行内显示，完整名字保留为 tooltip/可访问名称。

仓库标题精确为 `Repositories (N)`，括号内是当前列表长度（`Shell::repositories().len()`），不使用中点分隔，不硬编码；收起态隐藏标题的行为保留。

Home/Settings、仓库和 Profile 的活动行从客户区可绘制左缘 x=0 开始绘制 2px 宽中性指示条（`SIDEBAR_INDICATOR_WIDTH`）：左上/左下圆角严格为 0，只有右上/右下 4px 小圆角；高度为行高的约 2/3（`SIDEBAR_INDICATOR_HEIGHT_RATIO`），并 clamp 到 20–24px，垂直居中，因此随当前行（含动画中间高度）变化。它处在行背景外，没有交互命中区，不覆盖窗口 resize 规则。仓库指示线与行一起在独立 viewport 内滚动及裁剪。Ely NavItem 无关闭内建 accent indicator 的 API，侧栏模块因此按原控件的主题与键盘行为组合行，避免双线；四个全局入口仍使用自身活动态。

账号区不是整行 button：外层 `account-row` 不 hover 点亮、不响应空白右侧；只有头像 + 显示名 + `@ID` 组成的紧凑身份控件（`account-identity`）打开 Profile，保留可访问名称、Tab 焦点与 Enter/Space。身份控件本身也没有整块 hover 背景：悬停时只显示 pointer 光标，并让 `@ID` 出现下划线。展开时右侧另有独立 emoji 状态按钮（占位示例 💻 / `Coding`），hover tooltip 为 `Coding`，点击不导航 Profile 且停止冒泡；本轮无编辑弹窗、无网络、无写状态。收起时状态按钮隐藏，状态文字并入头像 tooltip。账号外层活动 indicator 仍按上面的贴边规则，不把状态操作标成 Profile 点击。

标题栏以下分界区域垂直中央的 24px 圆按钮负责收起/展开，其水平中心落在 island 的左边框线（`width + ISLAND_GAP + ISLAND_BORDER_WIDTH/2`），垂直中心对齐 island 自身高度中点（扣除底部 inset）；两个轴都从与 island 相同的几何常量推导，动画、resize、最大化和窗口化都跟随。分界 gap 提供 8px 横向 resize 命中带，无需拖圆按钮。sash 只在指针位于热区、或一次有效 resize 手势进行中时显示；手势从按下到释放全程保持可见（即使指针短暂跑出正在移动的旧命中带），释放后立即按真实 hover 决定淡出或继续。淡入淡出约 140ms ease-out、可逆续接并遵循 reduced-motion。sash 左侧锚点固定为原 2px 线的中心位置，只向右加宽，右缘与 island 外框左缘重合；上下端仍与 island 外框等高，toggle 在其上层且命中独立。竖线的纵向范围与主页面 island 的完整外框等高：顶端对齐 island 顶边，底端对齐 island 底边（命中带本身用 `ISLAND_INSET` 作为底部内缩，与其上层 island 的底部留白同源，不再叠加第二个 inset），因此不铺满侧栏、也不进入标题栏或窗口底部，且随展开/收起、宽度变化、矮窗口、最大化和窗口化一起跟随。中部的圆 toggle 在其上层绘制，线可被它遮挡但不会覆盖按钮。GPUI drag 捕获负责跨命中带拖动。展开态向左拖动先连续缩小并在 `SIDEBAR_MIN_WIDTH`（200px）停靠，再继续越过 `SIDEBAR_DRAG_COLLAPSE_THRESHOLD`（当前实现起点 32 logical px，可调）才直接折叠；收起态从 rail 起拖，向右越过同一阈值才直接展开为记住的展开宽度（按窗口预算 clamp）。阈值以本次手势的按下点为基准，模式切换会重设基准，因此一次手势不会因布局跳变反复切换；未过阈值释放则维持最小宽度/rail。最大 360px 与窗口内容预算不变。释放、Esc、失焦与窗口尺寸变化终止拖动并清理高亮。圆按钮独立命中，不启动 resize；拖动不改变路由。原生窗口边缘缩放和标题栏实现没有改变。

收起/展开圆按钮不使用可见边框：点击或键盘聚焦时不再绘制焦点边框，键盘焦点由既有圆形背景表达，按钮尺寸、位置、图标、背景与命中范围不变；点击按钮不会启动 resize。

收起时 Home/Settings 保留图标，仓库保留 namespace avatar，账号保留头像；标题、计数、姓名和 ID 隐藏，完整 tooltip/可访问名仍在。仓库区域独立滚动，键盘到达屏外仓库时沿用 `reveal_row` 最小滚动（并取消在途滚轮缓动）。收起/展开不导航、不重置 repo/page 滚动。

底部四入口（展开态）各自使用一条低对比度 Ely 中性色细边框，保持横向四等分、间距与圆角一致，不抢眼。收起模式的 More 触发按钮不再有可见边框（保留 `border_1` + 透明色以维持 28px 盒模型与键盘 focus 环），按钮槽位自身用 flex 居中其 ellipsis，使图标落在 28px 方形正中而不是左上角；外框尺寸、位置、布局占位与命中范围不变，tooltip 与可访问名称均为 “More”；菜单内仍有 Notifications、Issues、PRs、New，各有图标与标签，当前全局路由在菜单项上标记，New 继续 disabled。菜单由侧栏自绘（用 Ely 主题 token：overlay surface、border、elevation 与 menu width），打开状态、焦点归属与按键由 `Shell` 拥有：点 More 打开面板并把焦点移入，方向键移动高亮、Enter/Space 执行、Esc 关闭并把焦点交回 More 按钮（可立即再按 Enter 重开）。自绘原因见开发日志：Ely `OverflowMenu` 的私有 focus takeover 会在收起动画重绘时被重新武装，导致 Esc 或窗口变化无法关闭面板。菜单打开期间 Tab 被消费，Ctrl+K 会先关闭菜单再开 palette；侧栏专用透明 dismissal shield 阻止外点关闭同时点击背后页面。窗口 resize/maximize/restore 与展开/收起都会关闭菜单；展开时菜单节点离树并回到壳层焦点。没有引入创建资源动作或新页面。

## 平滑滚动

`src/ui/scroll/motion.rs` 提供项目公共滚动平滑：离散鼠标滚轮累加进目标偏移，按真实时间用 `ely_core::motion::ease_out_cubic`（约 130ms）缓动；触控板/高精度像素滚动直接应用，不做二次惯性；GPUI 0.3.10 新增的触屏 `ScrollSource::Touch`（含手指抬起后 GPUI 合成的动量）也走直通，避免叠加第二层惯性；边界 clamp 到 GPUI 的 `[-max, 0]`，内容变短时同样收敛；无动画时不请求帧，遵循 reduced-motion。透明 `wheel_overlay` 在捕获阶段拦截指针所在区域，只消费一次并抑制 GPUI 内建的逐事件跳变；键盘 reveal、路由重置和 palette 光标移动会取消在途缓动。

内容平滑已接入：侧栏仓库 viewport、每会话 island 页面 viewport、palette 结果列表及横向 workspace tabs。每个区域各有独立 `Motion<ScrollHandle>`/真实 `ScrollHandle`，互不串扰；嵌套区域只消费一次。页面 `wheel_overlay` 只覆盖标签条以下的区域，不抢标签条的横向滚轮事件。精确 Pointer 与 Touch 直通，程序化 reveal 取消旧目标。

仓库 viewport 与应用级标签条现已接入 PR #29 的统一 [滚动条](scrolling.md)。侧栏持有独立 `repo_bars`，以纵轴 `BarId`、`Style::DEFAULT`、`Placement::overlay(4, 8)` 贴在仓库区域右侧，不挤压行高。样式、几何、拖动、140ms 双向可逆 fade 均来自 `src/ui/scroll/**`；`tab_scrollbar.rs` 已删除，不保留副本。仓库条 hover 绑定真实仓库区域，footer、账号与快捷按钮不参与；折叠/展开与窗口变化沿用真实 scroll viewport 重新计算。

`Motion<T>` 实现公共 `ScrollTarget`，仍读写原来的 `repo_scroll`。thumb 开始拖动和每次写入都取消平滑目标，拖动中不建立新的 wheel 目标，释放后继续从当前位置处理输入，防止拖后弹回；条绘制在透明 wheel overlay 之后，保留 thumb 命中。外部释放只 settle 本区域已记录的拖动，Esc、窗口失活与尺寸变化清理仓库条状态；tabs 的横轴、重排 hold 与侧栏 resizer sash 仍分别拥有自己的状态。

滚动条显隐与内容平滑是两个机制，同属 `src/ui/scroll`：`Bars` 负责显隐、`Motion<T>` 负责内容 offset。旧 `ui::scrolling` 已删除；tabs、repo、page、palette 都使用公共 motion，但 account-ui/content-reader 的独立工作树尚待同步接线，不能据接口可用宣称其 UI 已迁移。此前已合入的 workspace tabs、More、sash、分割线、身份与 indicator 约束保留。验证与预览边界见 [开发日志](devlog/2026-10-25-sidebar-layout.md)。
