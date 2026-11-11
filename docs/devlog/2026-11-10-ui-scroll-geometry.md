# 列表 viewport 与滚动条通道

- 日期：2026-11-10；续作：2026-11-11（Asia/Shanghai）。任务：`ui-scroll-geometry`，执行者：Codex。
- 原始 UI 反馈：7（首行裁切）与 9（Home 条覆盖底色/文字），不是 GitHub issue 自动关闭声明。
- 状态：实现与本地检查；负责人原生 Windows 验收待完成。提交日志时尚未创建 PR，最终检查与审查收据以此分支的 GitHub PR 为准。合并执行者为协调者。

## 原始反馈 7 返工：负责人拒收后的坐标修复

前轮 `bd73a9f` 的首尾/reveal 验证和字号修复没有证明原始 padding 遮挡消失；负责人已撤回验收。继续使用同一 PR #84/worktree/session，恢复 `pending`，不修改 `blocked`，合并仍由协调者执行。先前通过的通道与字号修复保留。

实际原因是 host 在 native viewport 之外保留固定纵向空白，框架再按缩小后的 bounds 建立 paint mask：Home 的 `render_panel_with` 有 header 下 `.gap_2()`，`HomeView::render` 有页底 `.pb_2()`；Files 在 `tree_column` 的列表外层 `.p(7)` 内建立 clip，同时 bar region 上下也缩进 7px。`List` 和 `uniform_list` 本身正确裁切到各自 bounds，`ScrollTarget` 没有额外减 padding；此前只相对这些 bounds 核验，漏掉了 header/外部面板边缘。

默认字号、中间偏移下，渲染场景的实测几何（logical px）：

| 场景 | 当前候选的真实 paint clip | 正确边界 |
| --- | --- | --- |
| Home 顶部 | y=64；header 底边 y=56 | y=56，去掉固定 8px 带 |
| Home 底部 | bottom=992；外面板 bottom=1000 | bottom=1000 |
| Files 顶部 | y=230.5；header 底边 y=223.5 | y=223.5 |
| Files 底部 | bottom=1389；pane bottom=1396 | bottom=1396 |

对应反例来自实际 `window.painted_quads()` 的非透明行背景、继承 `content_mask` 与边缘探针，而不是自建滚动公式：树上下/ Home 底部反例 run `d297a422-f5be-40ff-add0-fadc6ff9f2d0`；Home 上下在真实连续 wheel 建立中间位置后反例 run `4741e6df-9d3f-4d19-a2d3-98957debe036`。普通半行只应被真实边界裁切，不应再多丢失 7/8px。

实现删除 Home 固定 header gap；共享 `page_layout::PAGE_BOTTOM` 从 8 改为 0，保留给 Home/global/Notifications 使用的最小接口并纠正其说明，不以 unused 例外掩盖失效定义。Files 仅保留左右 7px，native viewport 和 track 共用完整纵向 pane。全局 Issues/PR 的标题、filter 和非空 status 组成固定区，避免空 status 节点和根 flex gap 在列表前留下带；全局列表与 Notifications 同时去掉页底 8px。实际追加/分页控件仍占自身可见空间。没有改行内外观，不添加 padding 补偿 offset、负 margin 或绘制遮盖。Sidebar 的原生 Div 内容路径沿用原边界与 reveal。

专用回归检查正常/放大字体、宽/窄布局与显示缩放，在中间偏移处核对真实 quad 的可见矩形是否覆盖紧邻 header/面板边缘的半像素探针；Home/Feed 在原空白带位置发送 precise wheel，核验只进入所属真实列表一次。原首尾、thumb、append 与列隔离回归保留；它们不再被用作原始 padding 问题已解决的唯一证据。约 `0.00004px` 的显示缩放坐标换算误差只在共享边缘比较容许 `0.01px`，不放宽实际 paint 探针。

本轮本地验证：同步 #83 后的 180 项相关组件回归通过（`1e49c5e9-c20f-44c6-8336-4b4e40ede7fc`），正式 Shell paint-mask 回归通过（`a2fd0cf5-52a5-4997-8c74-4519a1f39120`）。零底部 token 接口补齐后，最终 10 项聚焦回归（`7e087670-8f62-49a5-b08b-65b330254e17`）、正式 Shell（`79972c06-fd13-42d7-a53c-1ed15a35a594`）、fmt 和 `cargo clippy --locked --all-targets -- -D warnings` 均通过；全部 Cargo 操作由 `node scripts/verify.mjs -- cargo ...` 执行，debug/test debug=0、incremental=0。当前提交的云端 CI/Codex 与 debug 构建收据仍放 PR/本地交付状态。

Home/Feed 的 `Partial` 提示还存在独立 `.mb_2()`；追加反例 `8ef10972-6d01-4e85-9a3e-5f34fc682640` 在 56px 行基线上测到提示底边 y=126、native clip y=134。删除该外部 margin，保留提示本身的背景和内部 padding；完整与 Partial 状态共用同一边界原则。补充后全部 50 项 Home 回归（`7afc3d8b-b577-4430-988e-154faf4e22a5`）、正式 Shell（`670ab8f3-bd45-4c1f-aed8-f639c5ba0e31`）、fmt 与 all-target clippy（`d428b309-5d12-44c7-857f-e704dbe7a363`）通过。旧 head `25ce210` 的等待器为此主动停止；新 head 必须重新 push、触发审查和验证 CI。

最终基线同步到 `f9e31a0`（#83 已接受的 56px 行）；行样式不由本任务修改。cache-first-paint 的 app/pages、app/accounts 与 repository 加载/model/extras/locator 不在写入范围。现有 release GUI PID33288 不终止、不覆盖；新验收物使用 debug 路径。最终检查、当前 head CI/Codex 与构建收据以本轮 PR/本地交付状态为准。测试平台只证明绘制几何和模拟输入，原始截图与 Windows 原生视觉仍待负责人重新验收，不能仅凭测试通过宣称问题已获接受。

## 前轮改动与决定（历史，原始反馈 7 已被拒收）

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
