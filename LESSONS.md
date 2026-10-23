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
