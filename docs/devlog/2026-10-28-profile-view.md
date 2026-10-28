# Profile Overview 展示组件与独立预览

- 日期：2026-10-28；任务 ID：profile-view；执行者：基层 Agent。
- PR：待创建。目标：在 committed base `913eb5eb650c83eeb4c9b5c5cf422e5c26720506` 上交付 UI-only Profile Overview、合成预览、测试与 host adapter 文档。
- 状态：实现与本地验证完成，待 PR 审查、CI 与负责人原生窗口验收；本任务不合并。

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
- `test --locked`：主 binary、account、reader、profile assemblies 全部通过；profile assembly 含 140 项（其中 13 项 profile 行为/模型测试，其他为复用 Reader/scroll/domain 测试）。
- `build --locked` 与 `build --locked --example profile`。
- `git diff --check`。

测试覆盖 target/account/binding 改变、旧响应、partial GraphQL、缺字段、pin 顺序与分类、真实空/denied、Reader action、typed repository、闰年/年界、zero/missing/unknown level、tooltip/focus/keyboard、长内容与横向范围。一次长范围用例最初把同一天重复 366 次，模型正确保留一个观察并拒绝重复；修正 fixture 为真实连续日期后覆盖 53 周。880px 下 53 周占 742px、没有虚假 overflow；600px 下验证正 reach 与范围内滚动。

原生 Windows 预览已独立启动，标题 `Githux · Profile Overview preview`、进程 Responding；`PrintWindow` 成功取得 1000px 与最小 880×560 请求尺寸的真实渲染帧。最小尺寸帧显示 initials、name/login/bio、两列有界 public metadata 与 README 顶部，场景 toolbar 横向滚动不裁切。曾据帧修正 metadata 单行裁切，并新增 GPUI 几何回归。页面 wheel 在测试平台验证实际 scroll offset 与后续 section 位移。尝试用合成原生 wheel 截取下方 section 未取得滚动后的帧，因此不声称下半部视觉或 Windows 原生输入已经验收；启动/Responding 也不单独作为 GUI 通过证据。

## 审查修复

PR #36 首轮 `aba07c7` Codex 提出两项可直接修复问题：page thumb 外部释放后未 `settle`，导致 Motion 继续吞 wheel；热力图左右/上下步长与竖向 week column 的视觉方向相反。已分别补上释放清理与左右 7 天、上下 1 天，并在 `page_wheel_reaches_sections_below_the_viewport` 加入真实 thumb drag→release→wheel 回归，键盘用例显式区分 Right=下一周与 Down=下一天。两项均无需产品/架构裁决，永久 `arbitration_ever_required=false` 保持不变；等待新 head 的 CI/review，仍不自行合并。

## 后续

- 协调者创建/跟踪 PR；任务属 First Release milestone #1，保留 `pending` 等负责人验收，不自动合并。
- 负责人验收真实 Windows 预览：880×560 与缩窗、长元数据、README 选择/滚动、pin hints、全年图、鼠标/键盘 focus、外链不自动打开。
- 正式集成由后续 host adapter 完成，不将 fixture 当真实账户数据。
