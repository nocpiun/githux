# Profile Overview 展示组件与独立预览

- 日期：2026-10-28；任务 ID：profile-view；执行者：基层 Agent。
- PR：[#36](https://github.com/nocpiun/githux/pull/36)。目标：在 committed base `913eb5eb650c83eeb4c9b5c5cf422e5c26720506` 上交付 UI-only Profile Overview、合成预览、测试与 host adapter 文档。
- 状态：**视觉返工中**。负责人否决旧交付；下列旧验证是历史，不代表新版验收。共享 Reader Flow API 待协调；blocked 保留，不添加 pending、不合并。

## 改动与决定

新增 `src/ui/profile/**`，只消费现有 Profile 领域数据、PreparedContent 与明确 contribution range。实现公开资料/initials、ReaderView README、Pinned/Popular、贡献热力图与 typed actions；没有 Shell/routes/workspace/app/data/domain/reader/scroll 改动，也没有网络、PAT、存储或头像下载。

Pin 语义保持服务分类：GitHub Popular 只在无 pin 且 showcase 明确为 Popular 时显示；gist-only/mixed/truncated/unclassified 不转 Popular，实际 gist pin 仅显示提示。Partial、denied、真实空和 missing 不互相冒充。所有更新以 target + account binding + generation 校验；`bind` 作废旧内容、Reader 和动作。

贡献图按明确区间校验日期，区分 zero/missing/unknown level/partial，支持闰年、键盘与 hover 同日数据、共享横向滚动。长 README 使用既有 Reader 有界区域；880×560 的完整一年自然宽度可容纳，较窄窗口获得真实横向范围。

主要契约与后续 host 职责见 [Profile Overview](../profile-view.md)。生产集成尚需 fresh service-context 校验、同名 PUBLIC 仓库根 README 发现与准备、滚动一年 range、Shell mounting 与导航处理；预览不是完成这些工作的证据。

## 验证

在本 worktree 独立 `target/`，重 Cargo 命令均经 `D:/githux/.agents/astra/run-additional-cargo.ps1`（命名互斥、`CARGO_BUILD_JOBS=1`）执行，没有共享或删除其他 target，没有真实凭据或 GitHub 请求。

已通过的最终候选工作区检查：

- `cargo fmt --all`；随后以 `--check` 复核。
- `check --locked --all-targets`。
- `clippy --locked --all-targets -- -D warnings`。
- `test --locked`：主 binary、account、reader、profile assemblies 全部通过；组合树主 binary 538、account 321、profile 149、reader 131 项全部通过；本任务包含 18 项 profile 行为/模型测试，其余为复用 Reader/scroll/domain 测试。
- `build --locked` 与 `build --locked --example profile`。
- `git diff --check`。

测试覆盖 target/account/binding 改变、旧响应、partial GraphQL、缺字段、pin 顺序与分类、真实空/denied、Reader action、typed repository、闰年/年界、zero/missing/unknown level、tooltip/focus/keyboard、长内容与横向范围。一次长范围用例最初把同一天重复 366 次，模型正确保留一个观察并拒绝重复；修正 fixture 为真实连续日期后覆盖 53 周。880px 下 53 周占 742px、没有虚假 overflow；600px 下验证正 reach 与范围内滚动。

原生 Windows 预览已独立启动，标题 `Githux · Profile Overview preview`、进程 Responding；`PrintWindow` 成功取得 1000px 与最小 880×560 请求尺寸的真实渲染帧。最小尺寸帧显示 initials、name/login/bio、两列有界 public metadata 与 README 顶部，场景 toolbar 横向滚动不裁切。曾据帧修正 metadata 单行裁切，并新增 GPUI 几何回归。页面 wheel 在测试平台验证实际 scroll offset 与后续 section 位移。尝试用合成原生 wheel 截取下方 section 未取得滚动后的帧，因此不声称下半部视觉或 Windows 原生输入已经验收；启动/Responding 也不单独作为 GUI 通过证据。

## 审查修复

PR #36 首轮 `aba07c7` Codex 提出两项可直接修复问题：page thumb 外部释放后未 `settle`，导致 Motion 继续吞 wheel；热力图左右/上下步长与竖向 week column 的视觉方向相反。已分别补上释放清理与左右 7 天、上下 1 天，并在 `page_wheel_reaches_sections_below_the_viewport` 加入真实 thumb drag→release→wheel 回归，键盘用例显式区分 Right=下一周与 Down=下一天。两项均无需产品/架构裁决，永久 `arbitration_ever_required=false` 保持不变；等待新 head 的 CI/review，仍不自行合并。

第二轮 `16304ab` review 指出横向溢出时键盘选中日未 reveal；已经共享 Motion 滚动显示选中的 week，新增 600px 下 End/Home 的真实 offset 与 day detail 验证。自行检查同时补齐 rebind 更换焦点句柄、替换 README 后拒绝保留旧 Reader 的回调，以及 header/repository 点击的 request 复查，并有 stale-reader/focus 回归。

同步 main `342e72d`（已接受 PR #32 reader-motion、PR #33 page-data），无冲突；没有修改这些 owners 的实现。随后对组合树重新执行全部检查。README 发现 helper 的已合入接口已链接至主题文档，正式挂载仍待集成。

第三轮 `014478d` review 指出 page motion 为 true 时 `||` 短路跳过 heatmap tick；改为同一时间采样下先分别 tick，再合并 animation 标记。长范围回归同时启动两个离散 wheel 动画，断言 page 仍动画期间两区域真实 offset 均推进。

第四轮 `cb61c40` review 指出 repository 两列 grid 的上下键仍为一步；已改为上下两项，左右仅在当前行移动，边界保持选中，并扩展 repository typed-action 用例覆盖四仓库、Down/Up/Right 与行边界。

## 后续

- 原执行者跟踪 PR #36 最新 head 审查/CI 并交回协调者；任务属 First Release milestone #1，保留 `pending` 等负责人验收，不自动合并。
- 负责人验收真实 Windows 预览：880×560 与缩窗、长元数据、README 选择/滚动、pin hints、全年图、鼠标/键盘 focus、外链不自动打开。
- 正式集成由后续 host adapter 完成，不将 fixture 当真实账户数据。

## 视觉返工（负责人否决旧版后）

实际读取原始 04-profile.png（忽略目录），按内容区重建：扁平横向 84px initials、name/login 上下、紧凑公共 metadata 与真实 website 文本；轻量 Overview active 栏；README 单边框和真实文件身份；四张紧凑链接/visibility/description/stats 卡片；两列/窄屏单列与对应键盘步长；贡献图 Monday-first、月份真实日期对齐、token 绿色 ramp、missing/unknown 独立、Less/More 图例。开发控件改成清晰单 Select，正常默认不再展示工程文案。生命周期、generation、typed actions、pin 语义继续保留。

自然 README 尚未实现。现有 ReaderView/MarkdownView size_full + 自有虚拟 ListState/Motion 不提供外部自然流。已提交共享 ReaderLayout::Flow 最小 API 建议与影响文件/测试边界至 PR 协调评论；未改共享 Reader/scroll，也没有通过字符估高或换一个固定框冒充完成。因此永久 arbitration history 改为 true/coordinator，旧 false 仅表示当时审查轮次的历史；当前不具备最终视觉验收条件。

本地新版实际 native frame：144 DPI；1100×800 client=1650×1200 physical，880×560 client=1320×840。截图存 task 忽略目录：rework-1100-800.png、rework-lower.png、rework-minimum.png、rework-minimum-lower.png。Win32 wheel 消息实际使页面滚到下方四卡与完整贡献图；Win32 点击 README Projects 与第一张卡分别报告正确 Reader External 和 RepositoryTarget(id=1)，End 键选中最后一天且显示真实日期/count；这是 Windows 消息输入验证，非人工鼠标/触控板验收。未以 PrintWindow 上半截当整页通过。整页密度仍被旧 Reader 空框阻塞。

新增 GPUI 行为覆盖 name/login 上下、资料边界、卡片两列/单列边界和键盘、月份跨年/闰年星期、真实 website 点击动作。既有身份/旧响应/Reader stale callback/partial/pin/scroll 回归保留。最终同步 main 8de1959（已接受 Home/Feed PR #34），无冲突；组合树 fmt/check/clippy -D warnings、完整 suite 538 + 321 + 54 + 152 + 131、product/example build 与 diff check 均通过。最终 head/CI/review 与 preview PID 放 PR/本地交接，不为补 SHA 再造提交。
