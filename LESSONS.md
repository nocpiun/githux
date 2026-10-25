# 项目教训

记录 Githux 实际遇到的问题，供所有 Agent 在改动前阅读。每条包含规则、原因、证据与适用范围；没有实际发生的故障写入设计风险，不伪造复盘历史。维护方式见 [共享文档索引](docs/README.md)。

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
- **范围**：PR 流程、UI 测试与其他外部集成。后续真实验证通过后补充证据，不改写之前的验证事实。

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