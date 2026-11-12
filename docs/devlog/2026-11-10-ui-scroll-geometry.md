# 列表 viewport 与滚动条通道

- 日期：2026-11-10；续作：2026-11-11（Asia/Shanghai）。任务：`ui-scroll-geometry`，执行者：Codex。
- 原始 UI 反馈：7（首行裁切）与 9（Home 条覆盖底色/文字），不是 GitHub issue 自动关闭声明。
- 状态：PR #84 保留 `pending`，按负责人纠正继续返工；`cc387aa` 删除视觉留白的结果已被拒收。最终原生验收和合并仍由负责人/协调者处理。

## 保留必需留白的再次返工（2026-11-12）

负责人明确：Files 上下 7px、页底 8px、Home/Feed 标题及 Partial 提示间距是必需行为。上一轮 `cc387aa` 虽扩展了 clip，却删除了视觉间距，不能以当时 CI/Codex 或中间 paint 测试代替验收。以下旧记录保留为被拒方案的历史，不再代表当前约定。

先补正向端点断言，在 `cc387aa` 上 run `5e5e018d-f6b2-44c6-a739-6580e37600bb` 如预期失败：Home 首背景 y=56 与 header.bottom=56 相接；长列表末背景到面板边缘 gap=0，均缺少必需 8px。新测试要求正常/放大字体、宽/窄、短/长/空数据、Complete/Partial、键盘首尾与 scrollbar maximum 同时保持留白；原中间偏移 quad/content-mask 探针保留。

当前布局将视觉间距和绘制 mask 分开：

| 表面 | 起点/末尾视觉间距 | 中间偏移的真实 clip |
| --- | --- | --- |
| Home/Feed | 首项 envelope 顶部 8px；Partial 提示前后各 8px；末尾原生 content padding 8px | 固定 header/实际提示下沿至面板底边 |
| Files | 原生 uniform-list 上下 content padding 7px；左右对齐及 bar track 上下 7px 保留 | 完整纵向 pane |
| 全局 Issues/PR | 保留原 filter/status 间距，无 status 时两段 8px、有 status 时其后 8px；末尾 content padding 8px | 固定区下沿至页底/真实分页控件上沿 |
| Notifications | 保持原起点；无分页控件时末尾 content padding 8px，有控件时保留控件后的页底 8px | 固定区下沿至页底/真实分页控件上沿 |

`PAGE_BOTTOM` 恢复语义值 8；未恢复会收窄 clip 的外层固定 padding。GPUI List 的 top style padding 会影响每个 logical anchor，因此只给第一项布局 envelope 加起点间距，真实 56px 行不变，业务 count/index 没有 spacer 行。底部采用原生 padding，追加不改变旧末项的高度。`InsetListTarget` 补足原生 scrollbar maximum 漏算的 bottom padding，写入走公开 `scroll_by`，拖动冻结继续委托原生 hooks；专门覆盖尚未测量末项就拖到最大位置，避免留白被漏算。该用例还暴露 GPUI 在首次宽度确定时丢弃 uniform hints、导致最大范围仅覆盖已测量前缀的问题；运行路径按新宽度/数量从已测量项补齐估值，保留已有测量和 anchor，不全量测量。测试不再专门补值，直接验证真实 view 的未测量末项。Feed 预取回归也改为小步真实 wheel 进入末三项区域：将第 37/40 项直接定位到顶部会触发原生末尾回填，旧断言依赖 unknown-height 的 `None` 才通过；新路径仍要求到真正末尾前只发一次追加请求，没有改变分页实现。

uniform-list 虽支持 content padding，其可见尾范围仍按 padded height 计算。共享 helper 在范围尾后最多多渲染一个真实行，布局高度和 native item count 不变；最后仅露出 3px 的真实行必须有实际 quad、继承完整 clip 并能点击选择。Notifications 的键盘 reveal 延至 motion tick 后，未测量目标以业务索引交给原生列表布局，避免按 End 时按旧高度丢失目标；已测量目标继续 nearest reveal，方向键选中可见行不额外跳动。

最终本地 runner 已通过：114 项相关组件回归（`a9d349ec-4202-4958-92a2-a2fc5e1d6d6e`）、1 项正式 Shell 组合回归（`817035c2-c115-433f-a230-bae2af0f8a58`，含中间 paint mask、Home/Feed、Files、sidebar 与 reveal）、fmt（`d9a03032-cb79-4543-9e59-6e39c11d69d1`）及 all-target clippy（`3ecd947f-0d51-4e82-9318-817502bbff05`）。聚焦的无测试补值 thumb-max 回归亦通过（`382b1504-e9a7-46f0-850a-a9db0719ddbc`）。检查按实际修改/失败集中收尾，全部通过共享 runner，dev/test debug=0、incremental=0；构建槽等待与执行时长分别保存在收据。

初轮本地 Rust/app 基线为 `f9e31a0`。等待 `f426f93` 审查期间，main 先合入 #87（工作流/runner），再合入 Reader SVG #82（`8c63ce7`）。`f426f93` 的 Codex 已 clean，但等待器据实返回 `base_changed`；随后将已发布 main 合入任务分支，保留 `cc387aa`、`f426f93` 提交历史，不手工修改 Reader/image 实现。相对 main 的 PR 差异仍为本任务 16 个文件。

新基线组合验证通过 16 项针对性 viewport/Shell 场景（`b9967e99-ed57-44ff-8aed-de6f1063423c`）、fmt check（`c9a81ade-0677-45d2-9b4d-13e47c20aa7a`）和 all-target clippy（`2f578879-f6de-40c7-9b64-b6b35940ea8b`），覆盖首尾留白、中间 mask/input、未测量尾部最大拖动及 Files 的真实尾行点击。没有因基线变化重复无关测试。最终 clean-head 预览、CI/Codex 结果放 PR 和本地交付状态，避免为补自身 SHA 再造未审查提交。`pending` 与负责人管理的 `blocked` 均保留；旧 exe/旧收据和测试平台结果都不代替本轮 Windows 原生验收。不终止、不覆盖用户启动的 app 窗口。

## Codex 补充：物理 track 与内容 viewport 的几何分离

`df18f4a` 的新审查指出 inset Bar wrapper 缩短了物理 track，但 thumb 仍按完整 viewport 长度计算。接受该项 P2；实际控件回归 `31167488-ed6f-44f5-ad22-b814fd880740` 先失败：最大偏移时 thumb.bottom=806.5、track.bottom=790.5，越界 16px。直接写 native target 最大 offset 的测试不足以证明实际控件和鼠标映射正确。

共享 `Placement::with_track_insets` 同时驱动轨道布局与 thumb 长度/位置，Home/Feed、global、Notifications、Files 的 Bar 区域保持完整 viewport，不再另用缩进 wrapper。拖动计算读取事件提供的实际 hit track 长度；内容 viewport/reach、首尾留白、窄通道及样式不变。新增实际鼠标回归覆盖普通 thumb 与 24px 最小 thumb、宽/窄与放大字体，断言末端贴合物理 track，并拖到可见底部后核对真实数据末尾和 8px 留白。

聚焦回归通过（`440a7466-b177-40d7-b28f-685554693e3a`），受影响的共享滚动/页面及 Shell 共 45 项组合回归通过（`de78ff10-af41-4e69-865d-4a9138a86a03`），fmt check（`9f0fbbaa-5a4a-4485-a24d-9f7da283a2fb`）和 all-target clippy（`9b7986af-e8b1-4e10-886b-32ea1e3f94ea`）通过。没有待裁决争议；保留既有协调者合并归属及标签，最终 head 仍须重新 CI/Codex 和负责人原生验收。

## 原始反馈 7 前次返工（历史：cc387aa 的留白删除已被拒收）

前轮 `bd73a9f` 的首尾/reveal 验证和字号修复没有证明原始 padding 遮挡消失；负责人已撤回验收。继续使用同一 PR #84/worktree/session，恢复 `pending`，不修改 `blocked`，合并仍由协调者执行。先前通过的通道与字号修复保留。

实际原因是 host 在 native viewport 之外保留固定纵向空白，框架再按缩小后的 bounds 建立 paint mask：Home 的 `render_panel_with` 有 header 下 `.gap_2()`，`HomeView::render` 有页底 `.pb_2()`；Files 在 `tree_column` 的列表外层 `.p(7)` 内建立 clip，同时 bar region 上下也缩进 7px。`List` 和 `uniform_list` 本身正确裁切到各自 bounds，`ScrollTarget` 没有额外减 padding；此前只相对这些 bounds 核验，漏掉了 header/外部面板边缘。

默认字号、中间偏移下，渲染场景的实测几何（logical px）：

| 场景 | 历史外层 inset 的真实 paint clip | 正确 clip 边界（仍须另保留内容留白） |
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
