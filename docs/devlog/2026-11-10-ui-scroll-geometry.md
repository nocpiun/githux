# 列表 viewport 与滚动条通道

- 日期：2026-11-10；续作：2026-11-11（Asia/Shanghai）。任务：`ui-scroll-geometry`，执行者：Codex。
- 原始 UI 反馈：7（首行裁切）与 9（Home 条覆盖底色/文字），不是 GitHub issue 自动关闭声明。
- 状态：实现与本地检查；负责人原生 Windows 验收待完成。提交日志时尚未创建 PR，最终检查与审查收据以此分支的 GitHub PR 为准。合并执行者为协调者。

## 改动与决定

Home/Feed、全局 Issues/PR、Notifications 为原生列表保留固定 8px 尾部通道。3px 可见线距 pane 右缘 1.5px，距满宽行底色 3.5px；隐藏时保留通道，避免行内容宽度跳变。wheel 区域仍覆盖整个列，拖动仍由原生 list handle 驱动，Feed 测量后自动续读保留原路径。

文件树的固定 26px 行可裁切放大的文本：`font_scale=2` 时文字盒高 35px，首行 y=379.5，而文字 y=375，向上越出 viewport 4.5px。将 26px 改为最小高度，由 uniform list 测量完整单行文字。保留原有四侧 7px 留白、pane 右缘 2.5px 可见条和上下 bar region；没有用额外外边距补偿。原 padding 结构本身通过几何检查，没有保留无证据的 wrapper 简化。

没有修改共享行/行内链接、Reader、Profile、Shell 或 tab 文件。sidebar 保留现有窄轨道，通过正式 Shell 的展开/收起首尾 focus reveal 审计。共享行的固定高度与字号关系作为跨任务发现交给 ui-row-links；没有将未实测的 Home 字号假设宣称为原截图根因。

长期约定见 [滚动](../scrolling.md) 与 [Home](../home-view.md)。

## 验证

- 新 Home 回归放在 `src/ui/home/viewport_tests.rs`，不改原 Home 测试或共享行文件；覆盖短/长、双列/堆叠、原生首尾、Home/End、thumb 拖动、列隔离及隐藏通道稳定性。
- 树回归覆盖普通/400 文件、1180/560 宽、显示缩放 1/1.5 与字体缩放 2，核对 pane、native bounds、首行文字盒和末行可达。大字体用例给页头与内容足够高度；零高度 pane 不能作为“仍应有上下 7px 内容区”的有效断言。
- 正式 Shell 集成使用内存账户与离线 HTTP fixture，检查 Home/Feed 祖先边界、键盘首尾、内容链接 focus 不移动页头、树首尾和 sidebar 展开/收起 reveal。fixture 正确处理 bounded response，并显式 drain 测试平台的页面接收器。
- Home 旧满宽布局的反向回归明确失败：`1630bb70-d7f0-4416-bb46-779aa31554a9`，行右缘 x=574，8px thumb 命中区从 x=566 开始。
- 固定树行的放大字体回归先失败：`cb71ad4d-e60d-425f-bc7c-de4620ae2053`，35px 文字盒无法放入 26px 行。
- 集成 main 后，同期其余 94 项相关回归通过（包括 Home/Feed、共享 scroll、global/notification 和原树 inset 回归）；随后仅重跑调整过的树与正式 Shell 场景。最终 fmt、测试、all-target clippy、exe 构建及 CI/Codex 收据放 PR，避免为补自身 SHA 再增加文档提交。
- 最终 main@54c8afb 组合检查通过：`node scripts/verify.mjs -- cargo fmt --all`；`node scripts/verify.mjs -- cargo clippy --locked --all-targets -- -D warnings`；`node scripts/verify.mjs -- cargo test --locked --bin githux --test scroll_viewports -- ui::home::viewport_tests:: ui::repository::tests::tree_viewport ui::repository::tests::inset_tree production_scroll_viewports --test-threads=4`（4 + 5 项）。exe 与最终 CI/Codex 收据放 PR/本地交付状态，避免为补自身 SHA 再增加文档提交。
- 最终同步 main@54c8afb，已包含 #78 字体、#79 Profile、#80 tab ghost 和 #81 Reader；#80 落地主线后再补组合验证，没有自行拼接兄弟分支。

## 验收边界

已复现一种真实首行裁切触发条件，不等于已确认原截图采用同一字号设置。GPUI 测试平台不提供 Windows 原生视觉/输入验收；`pending` 保留，不能宣称用户已接受原始反馈 7 或 9。

在实际 exe 中检查 Files 首行、Home/End 与 thumb 首尾；窄/宽窗口核对标题下首行和底部末行。Home 选择条目后 hover 出条，确认底色在命中通道前结束；Feed 用短首屏和长列表核对自动续读与键盘首尾。Notifications、Issues/PR 与 sidebar 展开/收起作为同类检查。最终 exe、PR/head 与审查收据在任务状态和 PR 交付。
