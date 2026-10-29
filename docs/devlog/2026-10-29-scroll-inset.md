# 统一滚动条尾侧间隙修复（scroll-inset）

- 日期：2026-10-29；任务 ID：scroll-inset；执行者：基层 Agent。[PR #48](https://github.com/nocpiun/githux/pull/48) 当前目标为 `feature/shell-pages`（叠加 [PR #42](https://github.com/nocpiun/githux/pull/42)），保留 `pending`，不合并、不 auto-merge。
- 目标与状态：修复统一 scrollbar 可见线离容器尾侧边缘偏远（图六 Shell 仓库侧栏约 6.5 logical px），保持细、圆端、半透明、淡入淡出与可用的命中区；**实现与本地验证完成，进入 PR/审查流程**。

## 改动与决定

触发条件：`Placement::overlay(inset, hit)` 原先把 3 px 可见线**居中**放进 `hit` 宽的命中条，再整体内缩 `inset`，因此实际可见间隙为 `inset + (hit - THICKNESS) / 2`。侧栏、树、Profile、Markdown/source、Home 等统一使用 `overlay(4, 8)`，可见间隙 6.5 px；Profile heatmap 用 `overlay(3, 8)`，为 5.5 px。

结果行为：把 `inset` 定义为**可见线到尾侧边缘的间隙**，命中条从尾侧边缘向容器**内部**展开（`Edge::End` 用 `right_0`/`bottom_0` 定位 track，可见线再用 `right(inset)`/`bottom(inset)` 定位）。因此：

- 所有内容区调用点改为 `overlay(1.5, 8.0)`：可见间隙 1.5 px，命中条仍为完整 8 px（`right-8..right` / `bottom-8..bottom`），可见线位于内侧 1.5–4.5 px。
- `Placement::BAND`（标签条）保持 `inset = 0`、`hit = THICKNESS`、`Edge::Start`，可见线与命中区都贴 band 起点，标签条专属布局不变。
- `Placement::clear_of(other, gap)` 按新的命中轨道重算角落避让，并只统计一次双方共享的尾侧间隙；双轴 corner 仍保留 2 px。
- `Edge::End` 的命中条宽度取 `max(hit, inset + thickness)`，保证更大的 `inset` 也不会让可见线落在命中条之外。

主要维护位置与完整调用点审计见 [统一滚动](../scrolling.md#尾侧间隙审计2026-10-29)。未改 Shell indicator、账户逻辑、Profile 布局、Reader 的 layout/image/HTML 实现、Ely/GPUI 版本或 Cargo.lock。

## 验证

- 代码/几何回归：`src/ui/scroll/tests.rs` 新增 `visual_gap_is_independent_of_the_inward_hit_area`，断言可见线到区域尾侧为 1.5 px、命中条为 8 px 且贴边，并按在命中条内侧（可见线之外）仍能真实拖动纵/横轴；`the_two_tracks_do_not_meet_in_the_corner` 更新为新的 corner 期望。
- 原生 Shell 等价预览：本地一次性开发者 example（不进入提交、不读真实账户/PAT）用真实 `Shell` + 合成 `/user` `/user/repos`（80 行）运行；144 DPI（150%）下逐列测量 `repo-scrollbar`，修复前 10 物理 px（约 6.67 逻辑 px），修复后 2 物理 px（约 1.33 逻辑 px，标称 1.5）；真实 thumb 拖动使滑块下移约 120 物理 px，wheel 继续移动，hover 淡入正常；两个显示器结果一致。截图与脚本在本地 `.agents/output/`、`.agents/task/`（均被忽略）。
- 100% 逻辑尺度由测试平台断言（1.5 px）；本机两个显示器均为 144 DPI，没有第二个原生 DPI 可比，已在文档中标注。
- 未覆盖：真实触控板惯性、真实 IME、macOS/Linux 原生窗口；`cargo test` 使用 GPUI 测试平台，不替代原生输入手感验收。

## Shell 组合复验（2026-10-29）

负责人要求把唯一集成目标改为 `feature/shell-pages`。协调者完成 rebase/推送/修改 PR base，本轮起点为 `3eca830b1c95c83f279c342d79803ae9698a7773`，实时 Shell 基线为 `96c32464786204ab1ca947f060ed68e0b6f484bc`；本轮结束前再次核对远端 ref，仍为该基线。此前 main 上的检查和截图仅为历史证据，不代表正式 Shell 接线已验收。

本轮应用 L002、L010、L011、L012：一次性本地 example 使用正式 `Shell`、正式页面 host 和真实服务/准备管线，只注入临时 SQLite、内存凭据及按资源身份应答的合成 transport。Repository 复用共享 `repository_fixture.rs` 的 400 文件树；包装层提供 Shell/Profile 读取，并同步长源码 descriptor 与 blob 字节数（240 行、长行双轴溢出）。未接触真实账户/PAT，不联网；该 example 已移回 `.agents/task/scroll-inset-combined-example.rs`，未提交。

原生运行通过 Win32 窗口定向消息与 DPI-aware `PrintWindow` 观察。桌面截屏一度只得到背景，已弃用这些帧；保存并实际查看的是窗口完整渲染帧，物理尺寸 1342×896、144 DPI。输入证据是 Windows 消息分发，不是人工鼠标/触控板手感验收。

| 区域 | 同一 Shell 基线的前后可见间隙（144 DPI） | 实际操作证据 |
| --- | --- | --- |
| 正式 Shell 仓库列表 | 10 → 2 物理 px（约 6.67 → 1.33 逻辑 px） | 在命中区内侧拖动，列表从首行移动到第 21 行附近，固定 footer 保持位置 |
| Profile 外层 | 10 → 2 物理 px | 在 8px 命中区内侧拖动，身份区向上滚动并露出 Contributions；不修改 Profile 内容 Flow |
| Repository tree | 10 → 2 物理 px | 在命中区内侧拖动，400 项列表从 `file-0000` 移到 `file-0173` 附近，Reader 内容保持原位 |
| Repository 内 Markdown Reader | 修复后的可见条靠近右缘 | wheel 实际把 README 从标题段移到下方列表/“Try it”段 |
| Repository 内 Source Reader | 纵横条均在真实内容 viewport 内显示 | 纵向拖动从第 1 行移动到第 87 行附近；横向条显示，但本轮窗口消息未证实横拖成功 |

完整帧在本地 `.agents/output/combined-*.png`；逐列数值在 `combined-gap-evidence.json`；三个 `combined-*-comparison.png` 仅把原生帧裁切放大并排，没有重绘 UI。修复前版本由本 worktree 临时恢复到上述 Shell 基线的 scrollbar 源码构建，采集后逐字节恢复修复源码，Git 工作区已核对。没有修改其他 worktree。

当前 Shell 基线的 Profile README 显示空内容区，Flow 接线由 Shell 所有者继续处理，本任务未修改。该现象限制 Profile README 的整页内容验收；外层 scrollbar 的几何与拖动已有独立原生证据。本轮也未新增原生 100% DPI、横向拖动成功或触控板证据；100% 几何、双轴隔离/corner/drag、隐藏无拦截、fade 与 wheel 的自动回归由下列实际测试覆盖。

组合检查均使用本 worktree 独立 `target`；重型 cargo 均经过 `.agents/task/cargo-serialized.ps1` 串行锁且 `CARGO_BUILD_JOBS=1`：

- `cargo fmt --all -- --check`、`git diff --check`：通过。
- `cargo check --locked --all-targets`、`cargo clippy --locked --all-targets -- -D warnings`：通过。
- `cargo test --locked`：7 个测试目标分别 726 / 726 / 56 / 159 / 136 / 494 / 18 次执行，合计 2315，全部通过（重复模块按各测试目标分别计数）。
- `cargo build --locked`：通过；原生组合 example 构建并运行成功。

本轮没有新的产品源码修改；最终文档提交同样重新触发 CI/Codex review。实时 head、对应 CI/review 收据及目标基线保存在 PR 和本地 delivery progress；旧 `b8a4f1b` 的通过记录不用于放行本轮。保持人工验收 `pending`，即使检查全绿也不合并到 `feature/shell-pages`。

## 后续

- 需负责人手动验收：已加 `pending`，不自动合并。
- 若审查要求进一步收紧其他调用点的数值，按本文件的可见间隙/hitbox 语义修改，不改变标签条 band。

## 可复用教训候选（交协调者汇入 LESSONS，本任务不直接写）

- **浮层“边距”参数必须区分可见线与命中区**：当命中区比可见线宽时，若把 `inset` 实现为“命中条整体内缩”，可见线会被再居中推远 `(hit-thickness)/2`，测量出的间隙与配置值不一致。把 `inset` 定义为可见线到尾侧边缘的间隙、命中条从边缘向内展开，能让配置值等于可见结果，同时保留命中区；审查 UI 间距时同时量可见元素与 hitbox，不能只看 CSS/布局参数。证据：本轮 `overlay(4,8)` 实际 6.5 逻辑 px，改成可见间隙语义后为 1.5 px；原生像素测量与单测一致。
