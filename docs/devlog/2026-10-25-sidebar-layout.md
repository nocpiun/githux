# 侧边栏布局、resize 与 collapse

- 日期：2026-10-25；任务：sidebar-layout；分支 feature/sidebar-layout；派发基线 262a013。
- **manual acceptance required / DO NOT MERGE。** 负责人要求完成实现和审查后保持 PR OPEN，等待真实窗口验收；禁止 auto-merge。人工验收未完成。没有已知产品争议，不把人工验收伪记成 arbitration。
- 多标签 PR #16 与本任务独立开发。本分支未合入、未复制其代码；本预览没有多标签不代表删除该功能。最终集成由主 Agent 协调。

## 实现

行为与尺寸的主要契约在 [侧栏布局](../sidebar-layout.md)，参数位于 `src/ui/geometry.rs`。默认 232px，展开 200–360px，收起 56px；窗口预算保留 island 600px，880px 窗口时上限 265px。本次运行记住最后展开宽；折叠不导航、不重置两条滚动句柄。只有仓库 viewport 滚动。

行背景左内缩与 footer 底内缩统一 8px，右侧 island gap 补齐 Profile 对称外留白。repo/footer 独立间隔 12px。账号头像由 24px 增至 32px，账号行 48px；repo 行左内缩由 8px 改 4px。Home、Settings、仓库、Profile 的活动线移至背景之外客户区左缘，仓库行与指示线一起裁剪。Ely NavItem 没有关闭内建 indicator 的接口，因此侧栏内组合等价主题/导航行为的行，没有改 Ely 依赖。

分界 8px 带使用 GPUI drag，圆形 24px 收起按钮单独命中，位于标题栏下 body 中央。处理 clamp、离开命中带、释放、Esc、失焦与窗口变化。收起菜单复用 Ely OverflowMenu，四项含图标、标签和活动态，New 继续禁用。测试发现 Ely 外点关闭仍会激活背后行，侧栏补透明 dismissal shield 阻止穿透；窗口变化让菜单焦点离开而关闭。

没有改原生标题栏、字体、workspace、palette 核心、后端或多标签实现。

## 验证

本机 Windows，Rust 1.98.1。`cargo fmt --all -- --check`、`cargo check --locked --all-targets`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo test --locked`（177 passed）、`cargo build --locked`、`git diff --check` 全部通过，最终结果在 PR/交接记录绑定提交。新增 `src/ui/sidebar_tests.rs` 通过 GPUI 测试平台实际绘制 Shell，覆盖：

- 客户区左/下 inset、Profile 至 island 的对称外留白、12px repo/footer 间隔、48px 账号行、活动线在行外并居中；
- 200/360px 越界 clamp、拖出命中带、释放后停止、Esc 取消，拖动不改变 Home 路由；
- 收起 56px、展开恢复 320px、路由及 repo 滚动保持；
- 880×560 预算限制为 265px、island 至少 600px、footer 在窗口内、收起后键盘聚焦第 124 项会 reveal；
- 真实 Ely 菜单四项、New 禁用、鼠标和键盘导航、Esc、外点击关闭且不导航、窗口 resize 关闭与存在焦点返回目标；
- 选中仓库行滚出 viewport 后 indicator 随行移动，footer 与页面滚动保持不动。

测试平台不经过 Windows 原生输入/IME，不能替代最终人工验收，也不通过颜色镜像断言宣称美观。

## 真实预览

独立 worktree debug：`D:\githux-sidebar-layout\target\debug\githux.exe`。在本 worktree 运行 `cargo run --locked` 使用既有 runner 自动准备 Windows App SDK；本轮已准备 app-local runtime。只启动/关闭本 worktree 的预览进程，没有修改全局环境。

本机 150% DPI 下检查了展开默认布局、32px 头像与两行文字、长 namespace/repo 截断、124 条 fixture、Profile 活动态、行外 indicator；实际点击收起/展开、菜单向上展开与 Esc，实际拖动到最大/最小宽并释放，Profile 导航正常。最后一版真实预览还确认外点击菜单关闭后没有导航穿透，随后展开及 Profile 导航可用；最小/矮窗口及全套菜单行为以测试平台为主要证据；原生 caption、多 DPI 跨屏、中文 IME 完整验收仍需负责人复测。

本轮截图诊断曾因 PowerShell 未启用 DPI awareness 导致 GetWindowRect 与 CopyFromScreen 坐标不一致，只截到左上部分，误以为 footer 越界。修正截图工具 DPI awareness 后完整窗口证明 footer 正常；相关临时根容器实验和诊断 I/O 已移除，没有以该误判修改原生 caption。

最终真实窗口视觉和全部手感由负责人验收。重点：外 inset 与 Profile 两侧留白是否舒适、repo/avatar 左移量、圆按钮位置/命中、边缘窗口 resize 不受活动线影响、菜单焦点与中文 palette 组合输入。PR 与 CI/Codex 最新提交结果只记在 PR/交接，避免为后补结果制造额外未经审查提交。
