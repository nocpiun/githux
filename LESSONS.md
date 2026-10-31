# 项目教训

记录 Githux 实际遇到的问题，供所有 Agent 在任务开工时阅读并应用相关条目，交付时评估是否需要补充。每条包含规则、原因、证据与适用范围；有新证据时新增或完善旧条，没有新增无需凑条目。尚未发生的问题写入设计风险，不伪造复盘历史。执行要求见 [Agent 工作流](docs/agent-workflow.md)，维护分工见 [共享文档索引](docs/README.md)。

## L001：要重新包含子目录，先让父目录能够被遍历

- **规则**：调整 `.gitignore` 后，用 `git check-ignore` 和 `git ls-files` 检查实际效果，不能只凭规则文本判断。
- **原因**：曾用 `.agents/` 忽略父目录，再尝试放行其中的 skills，子目录仍然无法正常纳入 Git。现在忽略 `.agents/*`，单独放行 `.agents/skills/`。
- **证据**：2026-10-21 PR 审查 skill 落地时实际修正，见 [当前忽略规则](.gitignore) 和 [开发记录](docs/devlog/2026-10-21-shared-workflow.md)。这是初始未提交工作区的记录，没有可引用的历史 commit。
- **范围**：仓库忽略规则与共享资源发布。`.agents/` 下的私有记忆、`.agents/design/` 和 `.agents/output/` 仍保持本地。

## L002：模拟验证要写清边界

- **规则**：报告脚本或 Agent 流程验证时，分开记录离线测试、模型场景推演和真实端到端结果。
- **原因**：审查等待脚本的模拟用例可以验证 SHA、分页和超时逻辑，却不能确认真实 Bot 如何表达“无问题”，也不能证明合并流程已经跑通。
- **证据**：2026-10-21 的 [等待脚本测试](.agents/skills/githux-pr-review/scripts/wait-review.test.mjs) 已有 17 项离线用例；真实 PR 验证尚未执行。属于本次搭建中确认的验证边界，不是线上事故记录。
- **补充证据**：Hello 的真实 Windows 窗口收到“鼠标可用、Tab/Enter/Space 无反应”的反馈，但原实现与显式焦点候选修复都能通过 GPUI 测试平台的导航/激活测试。测试没有经过 Windows 原生输入分发，不能将其当作缺陷已复现或已修复的证据，见 [键盘修复记录](docs/devlog/2026-10-21-bootstrap-gpui-ely.md#键盘反馈与本地候选修复)。
- **补充证据（外部 API 形状）**：不能凭记忆或旧版文档断言第三方 API 的字段/取值，也不要把自己写出的假设当成测试基准。2026-10-30 的 `home-feed-focus` 首轮把 `PullRequestEvent.action` 说成“没有 `merged`”，并据此写了“`action=merged` 必须被拒绝”的回归；官方事件类型文档与公开响应都包含 `action=merged`，且该 payload 的 `pull_request` 只有 `url/id/number/head/base`，没有作者。合成 fixture 恰好按错误假设构造，所以测试全绿而真实 Feed 一条真实合并都取不到。修复办法是先按官方文档核对并抓取一份无凭据公开响应形状（记录 API 版本与观测字段），再据此写 fixture 与回归，并把“离线回归”与“真实公开数据核对”分开报告。见 [开发日志](docs/devlog/2026-10-30-home-feed-focus.md#数据源更正)。
- **范围**：PR 流程、UI 测试、外部 API 适配与其他外部集成。后续真实验证通过后补充证据，不改写之前的验证事实。

## L003：按键归属要在按键绑定之前决定

- **规则**：浮层里需要与文本输入共存的按键（Enter、方向键、Esc），用 `intercept_keystrokes` 在绑定匹配之前拦截，并显式检查是否需要放行；不要依赖卡片上的冒泡监听。
- **原因**：Ely 文本输入的 `Enter` 绑定会发出提交事件并停止传播，卡片上的 `on_key_down` 收不到它；不检查组合态又会把输入法候选确认的 Enter 当成“运行高亮项”。
- **证据**：2026-10-22 外壳布局任务中，自绘 command palette 第一版把 Enter 放在卡片冒泡监听里，测试显示高亮行不执行；改为拦截器后通过。组合态分支用 `EntityInputHandler::replace_and_mark_text_in_range` 构造 marked range 验证，真实 Windows IME 消息顺序仍未验证，见 [开发日志](docs/devlog/2026-10-22-shell-layout.md#验证)。
- **范围**：任何把按键同时分配给文本输入和命令的浮层、菜单或编辑器外壳。测试平台上的组合态用例不能替代真实输入法验收。

## L004：跨检查平台的行为不能挂在平台分支上

- **规则**：属于已确认产品行为（位置、存在性、状态归属）的 UI，不要只按主题或编译目标的平台分支绘制；平台差异只能用在确实因平台而异的部分（例如窗口按钮、系统标题栏），并各加一侧用例。
- **原因**：`code-check` 在 Windows 运行时，把搜索入口画在 `Platform::Windows` 分支下的外壳在 CI 上看似通过；main 将该检查移到 Ubuntu 后，Linux 上同一选择器消失，14 项外壳测试里 10 项失败。
- **证据**：2026-10-22 外壳布局 PR #8 同步 main（含 PR #7 的 CI 迁移）时实际发生；修复为所有平台绘制同一条品牌 + 搜索入口、仅窗口按钮保留 `cfg!` 分支，见 [开发日志](docs/devlog/2026-10-22-shell-layout.md#与-main-的-ci-变更组合)。
- **范围**：任何 `code-check` 会跨平台运行的工程，以及“只有某个平台提供”的能力。先做真实行为归属判断，再决定条件编译。

## L005：取消令牌的“默认未取消”不能靠丢弃 sender 表达

- **规则**：判断“请求是否被取消”时，不要让“所有 sender 被丢弃”等同于“已取消”。用一个长期存活的 keepalive sender，或显式区分 `Closed` 与 `Cancelled`；并给“永不应解析”的路径写一条超时断言测试。
- **原因**：`CancellationToken::default()` 第一版只保留 `watch::Receiver`，sender 在返回时就被丢弃。`Receiver::changed()` 立即返回 `Err(Closed)`，实现把它当作取消信号，于是**每一个未显式取消的请求都被判成 `Cancelled`**：12 个真实用例全部失败，包括 `GET /user` 登录。这个缺陷在只测“显式 cancel”的用例里完全看不出来。
- **证据**：2026-10-23 `pat-data-service` 实现中实际发生，`cargo test` 报 `GitHub(Cancelled)`；修复为 token 内持有 `Arc<watch::Sender<bool>>` 保持通道打开，并新增 `default_never_cancels_and_dropped_source_does_not_cancel` 用例（用 `timeout` 断言不解析）。
- **范围**：任何基于 channel 关闭语义实现取消/关闭状态的异步组件，包括网络层、后台执行器与后续的写队列。测试必须覆盖“没有取消时不会误取消”这一侧，而不只覆盖“取消会生效”。

## L006：模块级 `dead_code` 允许要写在最内层、并写明原因

- **规则**：为“已实现但调用方尚未接入”的模块屏蔽未使用警告时，把 `#![allow(dead_code)]` 放在该模块的 `mod.rs` 顶部并附一句原因，不要放在 crate 根；同时仍要让真正的未使用 import/变量警告保持可见。
- **原因**：R2 切片先实现 `auth/network/github/data/domain/diagnostics`，UI 任务才接入。crate 级 `#![allow]` 会连同将来真实的死代码、未使用导入一起闭嘴；模块级允许把“尚未接入”这个已知边界限制在明确范围内，接入后直接删掉即可。
- **证据**：2026-10-23 `pat-data-service` 的模块注释与 allowance；同期用 `cargo clippy -D warnings` 确认其余代码没有隐藏警告。
- **范围**：分层交付、服务层先于 UI 落地的情形。允许本身不能替代对代码是否真的被调用的判断，也不能用来掩盖本应删除的实现。

## L007：安全判定与真正的连接方必须用同一个 URL 解析器

- **规则**：判断“这个 URL 是否可以去凭据”时，不要自己拆 scheme/host/userinfo。用与 HTTP 客户端**完全相同**的解析器（这里就是 `reqwest::Url`），并比较 scheme、host 与 port；解析失败一律视为不可信。
- **原因**：自写解析器必然与连接方在某处不一致。WHATWG 对特殊 scheme 把 `\` 当作路径分隔符，所以 `https://evil.example\@api.github.com/user` 实际连到 `evil.example`；按“最后一个 `@` 之后是 host”的手写规则却会得出 `api.github.com`，于是把 PAT 发给了攻击者。第一版修掉“路径里的 `@`”后仍然复现，说明逐条补特例不可收敛。另外同 host 不同端口也是不同源，不能因为 host 相同就放行。
- **证据**：2026-10-23 `pat-data-service` 的第二轮审查发现 `extract_host`/`Authority` 两版手写解析器都有缺陷；改为 `reqwest::Url` 后，`github::client::tests::path_content_does_not_make_a_host_trusted` 覆盖 12 种畸形/敌意 URL 全部通过。测试同时断言“绝对 URL 不带 Authorization”，而不只看布尔判断。
- **范围**：任何把秘密注入请求的安全判定——认证头、cookie、签名参数、webhook 回调校验。也提醒：安全相关的测试要覆盖“真正发出的请求”，不只是被测函数的返回值。

## L008：并发正确性测试必须真的能并发

- **规则**：验证“临界区是否被序列化”时，测试必须提供真实的并发交错（多线程 runtime、两个独立任务），并让被测组件在交错发生时可观测（例如统计同时进入临界区的次数）。单线程 `join!` 或顺序 `await` 无法暴露缺失的锁。
- **原因**：为“两个并发 replace 不会让存储与注册表不一致”写的第一个测试用 `tokio::join!` 跑在 current-thread runtime 上，两个 future 实际上不会在阻塞的临界区内交错；把提交锁**删掉**测试依然通过，等于没有验证任何东西。改成 `flavor = "multi_thread"` + 双 `spawn` + 记录 `save` 并发数的 store 后，删锁稳定失败、留锁稳定通过。
- **证据**：2026-10-23 `pat-data-service` 第二轮审查修复中的实际反复；测试 `data::tests::overlapping_replacements_are_serialized`。为此在 `[dev-dependencies]` 增加 `tokio` 的 `rt-multi-thread`，生产 runtime 保持 current-thread。
- **范围**：锁、原子状态机、单写入者约束、请求合并/去重等一切“同时发生才出错”的逻辑。写完测试要反向验证：去掉修复后测试必须失败。

## L009：审查完成、线程关闭和可合并必须分别判断

- **规则**：本轮 clean 与历史线程 resolved 是不同维度；目标分支实时 HEAD 要读 ref，不把 PR base.sha 或 CI 元数据当作实际测试基线。实现见 [完整流程](docs/pr-review-flow.md)。
- **原因**：PR #14 的本轮 clean 收据与历史未解决线程同时存在，旧脚本继续等不存在的新行内评论；PR #17 只读核验又发现 PR base.sha 仍为 a57e56c，而 main ref 已到 9125427。
- **证据**：[PR #14 收据](https://github.com/nocpiun/githux/pull/14#issuecomment-6392497811)、[PR #17 的旧 CI](https://github.com/nocpiun/githux/actions/runs/40158048254)；真实 checkout merge 的 parents 与目标 ref 核验及回归记录见 [开发日志](docs/devlog/2026-10-25-review-timeout-retry.md)。
- **范围**：审查等待器和并行 PR 集成；不能用更多轮询、重触发或关闭所有线程掩盖维度混淆。

## L010：交互预览的 fixture 必须支持反复操作

- **规则**：可交互预览按请求与资源身份应答，覆盖重复打开、返回、切换与重试；预览与行为测试共用 fixture。未建模请求明确报告开发配置错误，只有显式错误场景才返回相应产品错误。仓库预览的当前契约见 [开发预览与 fixture](docs/repository-browser.md#开发预览与-fixture)。
- **原因**：repository example 最初用一次性响应队列 `pop_front()`，仅首屏请求有数据，耗尽后统一返回 404；正常后续点击被误报为资源或权限问题。测试与预览各用一套 transport，测试通过没有暴露预览中的缺口。
- **证据**：[返工记录](docs/devlog/2026-10-28-repository-browser.md#本轮返工背景)及其“示例可完整、反复浏览”“预览场景与验收共用数据”；当前 [共享 fixture](src/bin_support/repository_fixture.rs) 的 `the_same_resource_can_be_read_repeatedly_and_in_any_order`、`scripted_failures_fire_once_so_a_single_retry_recovers`、`an_unmodelled_request_is_a_development_error_not_a_permission_result` 分别覆盖反复读取、一次失败后恢复和开发诊断（599），[页面行为测试](src/ui/repository/tests.rs) 覆盖嵌套目录、同文件重复打开与分支往返。预览与测试入口分别见 [example](examples/repository.rs) 和 [test](tests/repository.rs)。
- **范围**：有返回、重复读取、异步请求或重试的交互预览与验收工具。一次性队列可用于刻意限定顺序的测试，不能冒充可自由操作的预览数据源；合成 fixture 的实网验证边界仍按 [L002](#l002模拟验证要写清边界)。

## L011：裁切要核对实际绘制节点与设备像素边界

- **规则**：图像圆角要核对图片绘制节点实际收到的 radii；滚动裁切要覆盖完全离屏、部分可见、小数偏移及不同 scale，使用不透明高对比源图核对绘制结果。针对空 mask 的防护只跳过真实交集宽或高为零的绘制，保留部分可见内容；账户头像的接入背景见 [账户接入](docs/account-ui-preview.md#正式-shell-的账户界面2026-10-28-反馈后重做)。
- **原因**：头像最初仅在外层设 `rounded_full().overflow_hidden()`，图片节点仍露出方角。补图片节点圆角后又确认另一缺陷：GPUI 0.3.10 将不相交祖先 mask 钳成零面积，设备坐标取整可能把小数零高度扩为一像素，离屏头像及 tinted frame 在 viewport 下方露出细条。仅检查样式和布局尺寸无法识别这两处绘制问题。
- **证据**：[头像形状修正](docs/devlog/2026-10-28-account-shell.md#负责人反馈轮二头像形状修正)记录图片节点 `.rounded_full()` 与原生角像素核验；[裁切防护与失败复现](docs/devlog/2026-10-28-account-shell.md#裁切防护与失败复现)记录关闭防护时回归失败、150% 原生探针修复前 46 个亮色图片像素到修复后 0。[生产渲染器](src/ui/avatar.rs) 的 `VisibleAvatar::paint` 保护整个 frame，`offscreen_images_never_reach_paint_with_an_empty_fractional_mask` 覆盖两个不相交祖先 mask、1.0/1.5 scale、小数偏移及延迟图片；[avatar example](examples/avatar.rs) 提供离线原生探针。
- **范围**：GPUI/Ely 图片、滚动容器与祖先 mask 的绘制排查。空 mask 根因限定于日志所核验的 GPUI 0.3.10，不推广为所有框架均有此缺陷；原生 100% 缩放、手动拖动与动画手感未因此完成验收，验证边界按 [L002](#l002模拟验证要写清边界)。

## L012：同一页面的组件拆分要保留联合验收

- **规则**：按 [任务拆分工作流](docs/agent-workflow.md#角色派发与并行)指定页面组合的唯一执行者、联合预览和整页验收场景；独立组件探针验证局部行为，交付结论须说明实际组合的验证范围。
- **原因**：repository-overview 最初独立于 browser 交付，独立示例下方使用模拟阅读区，真实 browser 未挂载该组件。分别验证无法证明摘要、Tabs、文件树与真实 reader 同页时的空间分配、分支切换与局部失败行为。后续将两任务合并，由 browser 单一执行者统一组合和验收。这是共享记录中的集成缺口；负责人对密度和布局的新选择不作为技术故障归因。
- **证据**：[overview 初始结果与未接入边界](docs/devlog/2026-10-28-repository-overview.md#未执行--后续)、[任务合并与统一页面集成](docs/devlog/2026-10-28-repository-overview.md#合并任务与统一页面集成由-browser-执行)及 [browser 组合记录](docs/devlog/2026-10-28-repository-browser.md#owner-合并-browser--compact-overview统一页面)记录 PR #37 由统一 PR #35 取代。组合后的 [联合回归](src/ui/repository/extras_tests.rs) 中，`scoped_retry_keeps_source_and_sibling_groups` 验证 extras 失败/重试保留 reader，`overview_reads_once_and_branch_changes_keep_the_embedded_entity` 验证分支切换不倍增 extras 请求；日志另记录桌面与 880×560 的原生同页检查。当前联合入口与组件探针定位见 [统一页面契约](docs/repository-browser.md#统一页面与-overview-服务组合)。
- **范围**：同页多个组件的并行实现、预览与交接。组件内部仍可独立开发和测试；联合 example 的验证不能推导正式 Shell 已接线，原生证据仍遵循 [L002](#l002模拟验证要写清边界)。

## L013：删掉宿主约束不等于拿到了消费方能力

- **规则**：页面内嵌一个可滚动/虚拟化组件并希望它参与外层页面流时，必须先确认该组件提供自然高度测量与布局通知的 API，并在同一改动中接上；只删掉宿主的固定高度/overflow 只会让内容塌缩或消失。若消费方能力尚未合入，明确标记该项未完成，不要以"已移除固定高度"作为完成证据。
- **原因**：Profile README 上一轮删除了自身 280px 固定高度与 overflow 容器，但 ReaderView 根仍是 `size_full()`、MarkdownLayout 只有有界视口，父容器自然高度与内部虚拟列表不匹配，正文区域变成空壳。真实窗口只看到空白 README，而"移除固定高度"这一步本身通过了编译与既有测试。缺陷根因是宿主约束与消费方能力必须成对交付。
- **证据**：2026-10-29 第二轮交付记录把该项标为未完成；2026-10-30 在 main 合入 Reader `MarkdownLayout::Flow` 与 `on_layout_changed`（PR #45，`9522eee`）后，Profile 设置 Flow 才真正显示正文，并由 `flow_readme_reports_its_whole_document_and_outer_scroll_reaches_later_sections` 断言文档真实高度、文末位置与仅外层滚动可达后续 section。见 [开发日志](docs/devlog/2026-10-30-shell-pages-profile-flow.md)。
- **范围**：把虚拟化/自滚动组件（Markdown reader、列表、编辑器）嵌入更大滚动页面的接线；也适用于"先删父级限制"的重构。回归测试应断言内容真实尺寸与最末内容位置，而不只断言容器被移除。

## L014：可选内容缺失不是错误卡片

- **规则**："有就显示、没有就算了"的可选区块，其确认缺失/不可见应作为正常缺省：不渲染错误文案、不显示 Retry、不保留占位或空行间距，必要时整段省略。诊断语义（缺失 vs 不确定 vs 真实失败）留在内部状态，可见呈现只与影响相称：加载用动画，真实可操作失败才用一个小 warning 图标加 tooltip/无障碍名称，并且只在确实存在重试回调时提供重试。不要写服务实现解释给用户。
- **原因**：负责人连续反馈 Profile README 把正常缺省渲染成故障：先是 "This section was not found or is not visible." 文本面板，再是 "Unavailable · Retry" 式大错误块。这类呈现既误导用户、占用可省略的空间，又把"缺失"和"网络/认证/限流失败"混成同一种可见状态，掩盖真正可操作的失败。根因不是文案，而是把可选缺省错误地当成必须显式呈现的错误。
- **证据**：2026-10-30 `profile-readme-polish` 把 README 的 `SectionState::Unavailable` 改为整段省略（`src/ui/profile/render.rs` 的 `render_readme` 返回 `None`），真实 `SectionState::Error` 改用 `compact_status::failed` 单个 warning 图标；同名仓库 404 在 `src/app/pages/profile.rs` 归入缺省。回归 `confirmed_readme_absence_omits_the_whole_optional_section`、`actionable_readme_failure_keeps_a_compact_mark_not_absence`、`ready_to_absent_to_another_account_never_reuses_the_old_reader`。见 [开发日志](docs/devlog/2026-10-30-profile-readme-polish.md)。
- **范围**：外部数据驱动的可选区块（个人 README、附加资料、可选摘要）。本条目不授权把真实失败静默成成功缺省，也不要求所有 section 都省略；判断依据是该内容对页面是否必需，以及失败是否可操作。

## L015：虚拟列表的未知高度不能当成内容耗尽

- **规则**：依据虚拟列表位置触发翻页时，要区分尚未测量与真实短列表；使用已测量行边界或逻辑索引，并在布局完成后核验。不能仅凭 scrollbar 最大偏移为零/很小就认定已到尾部。首屏少量内容与过滤空页也须有自动续读路径，不能依赖用户再次滚动或无关定时器。
- **原因**：GPUI 列表首次布局及变宽会清除未测量行的高度提示。Feed 的“剩余滚动距离”此时只包含已测量部分，可能把长列表当短列表、提前耗尽源页；相反，在布局前只检查一次可能让短首屏等待下一次通知才续读。
- **证据**：2026-10-31 `feed_prefetches_before_the_end_but_does_not_drain_a_tall_initial_page` 在用最大偏移判断的实现上失败；修正为尾部行位置/逻辑锚点与布局后检查。详情见 [Feed 开发日志](docs/devlog/2026-10-30-home-feed-focus.md)。
- **范围**：虚拟化列表的自动翻页、预加载与首屏填充；测试应同时覆盖长列表、少量可见内容、全过滤空页和真实末页。
