# 阅读器公共 Motion 接线

日期：2026-10-28；任务 `reader-motion`；分支 `fix/reader-motion`，base `913eb5e`。本轮交付独立 reader 组件和开发示例，PR 保持 `pending` 等负责人原生验收，不合并、不启用 auto-merge。

## 行为与边界

- Source 构造时保存包装真实 `UniformListScrollHandle::base_handle` 的 `Motion<ScrollHandle>`；MarkdownView 构造时保存包装真实 `ListState` 的 `Motion<ListState>`。wheel overlay、Bar、drag hooks 和 frame tick 使用同一实例/clone，不复制 easing、几何或 Fade。
- 公共 wheel 路径新增兼容的 `WheelAxes`/`wheel_overlay_with`：旧单轴映射保持不变；reader 明确选择双轴或仅纵轴。双轴保留原生 x/y，Shift 仅在原生 x 缺失时映射 y；Markdown 不把横向输入变成纵向。统一使用 owner 时钟，precise Pointer、Touch 和 reduced-motion 直接写目标，离散 wheel 约 130ms 缓动。
- 3px、两端圆、透明度、corner 避让和 140ms fade 保持公共 Style/Bars 原值。正文选择/链接/Copy/Ctrl+A、源码 gutter、inline code、fence 标签、表格字号和虚拟化没有另建路径。
- thumb 接管、程序 offset 写入、选择按下、Markdown reset/adopt、ReaderView::show、Esc、窗口失活/隐藏以及 retained reader 离开布局均取消旧目标。可见期 keyed state 的 release 清理隐藏，entity release 清理 dispose。新增兼容的 `ReaderView::cancel_interactions(cx)` 供账户上下文失效的宿主调用；reader 自身不依赖账户模块。
- 实际代码核对：本地 Markdown 的 code/table 是换行布局，没有内层横向 ScrollHandle 区域；本轮没有新增虚构目标或重设布局。公共嵌套 overlay 仍复用原有 should_handle_scroll/捕获阶段路径。账户 Motion 由并行 account-shell 交付，此处不宣称其已合并或验证。

## 验证记录

受控时间的真实 GPUI 事件测试新增于 `src/ui/reader/motion_tests.rs`，首次完整通过 7 项：源码与 Markdown 离散 wheel 在中间帧推进并落点、停止请求帧；双轴/原生 x/Shift 与 precise/Touch；reduced-motion；程序跳转/正文替换/Esc/失活；选择按下/账户宿主失效/窗口隐藏；真实 thumb 拖动与区域外释放后的旧目标清理；两实例隔离与 retained reader 移出布局。既有布局/选择用例显式采用 reduced-motion，平滑用例单独启用受控时钟，避免墙钟快慢改变布局断言。

首轮回归发现并修正：无焦点时元素按键监听收不到 Esc，现采用可见期且限定窗口的 keystroke interceptor；横轴 fixture 使用明确窄视口确认真实 overflow。未放宽偏移断言。

已执行并通过：`cargo fmt --all -- --check`、`cargo check --locked --all-targets -j 1`、严格 `cargo clippy --locked --all-targets -j 1 -- -D warnings`、`cargo test --locked -j 1`（526 主程序 + 310 账户集成 + 130 reader 集成）、`cargo build --locked --example reader -j 1`、`git diff --check`。各重命令设置 `CARGO_BUILD_JOBS=1`，使用本 worktree 独立 `target/`。另行执行 `cargo build --locked -j 1` 通过，核对了不带 test-support 的产品配置；示例仍只由 `examples/reader.rs` 开发入口构建，没有新增产品按钮或默认 bin。

原生 Windows 预览已运行：`target/debug/examples/reader.exe`，PID 29760，窗口 1672×1196 物理像素。经 Win32 实际鼠标点击切换 Markdown → Source → Large source、纵向 wheel 与水平 wheel，PrintWindow 前后帧确认 Markdown 内容向上移动、Source 长行水平移动、Large source 从首行推进到第 5 行附近，并显示原有细条；表格字号、inline code 与 gutter 的真实渲染可见。截图保存在本地 `.agents/task/`，不提交工具产物。此处验证了原生输入送达与最终滚动位置，没有采集高速中间帧，不将其写成负责人手感验收。预览保留供负责人查看。

GPUI 测试事件不经过 Windows 原生输入分发。最终真实窗口滚轮手感、触控板、多 DPI 和负责人视觉验收单独记录，不以测试平台或进程响应代替。
