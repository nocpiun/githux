# 仓库底部 language bar 溢出 island 圆角

- 日期：2026-10-29（Asia/Shanghai）；任务 ID：repository-language-clip；分支 `fix/repository-language-clip`。
- 状态：实现与验证完成，待 PR 审查与负责人原生验收。

## 问题与触发条件

仓库页把 Languages 色条作为页面根的最后一个 `flex_none` 子元素贴在 island 底边（`src/ui/repository/view.rs` 的 `render`，色条本身由 `src/ui/repository_overview/summary.rs` 的 `language_bar` 渲染）。island 自身是 `rounded(6px)` + `border_1` + `overflow_hidden()`。色条一旦有真实比例数据并贴住底边，就在左右两个下圆角处以方形越过圆弧，正是所报缺陷。

真实窗口复现条件：页面内容足够短（本次预览的 Normal 场景）、色条有数据、窗口在常规与窄宽度下都成立；与比例、颜色、hover 和 tooltip 无关。

## 根因

GPUI 0.3.10 的 `ContentMask` 只有矩形边界（`gpui-pre` 的 `window.rs` 注释亦写明“Currently, only rectangular content masks are supported”）。`div.rs` 的 `overflow_hidden` 只把子元素裁到一个矩形；`Style::paint` 的 `rounded` 仅把圆角画在元素**自身背景**的 quad 上，边框也是同一个圆角 quad。也就是说：圆角只是被绘制出来，**没有裁任何子内容**。色条是子元素，因此它自己的矩形背景原封不动画到了圆角之外。

由此还确认：给色条或页面加 `rounded` 无济于事（那只影响它们自己的背景轮廓，仍不裁切相邻兄弟或父级描边），用更大 padding 或隐藏整条色条则会掩盖问题并破坏已确认的底部贴边要求。

## 修复位置与做法

新增 `src/ui/island.rs`：`RoundedClip` 是只包裹一个子元素的 `Element`，`request_layout` 直接返回子元素的 layout id（不引入 padding、margin、尺寸或新的布局盒），`prepaint` 委托子元素，`paint` 顺序为：

1. 画子元素（island 的 surface、被矩形裁切的内容、它自己的圆角背景与 1px 边框）；
2. 在 island 的四个角各擦除一个 `radius × radius` 方块减去四分之一圆——正是圆角外、矩形内那块唯一会露出子元素方角的地方——用 island 外围的 `colors.bg` 填色；
3. 用同一半径重画 island 的 1px 描边，因为擦除方块覆盖了落在角内的那段边框。

`Shell::island`（`src/ui/mod.rs`）改为构造原来的 frame 后用 `island::rounded_island(frame, outside, border)` 包裹；`module` 声明 `mod island;`。语言条本身、页面结构、文件树、README reader、滚动区域与 tab strip 一行未改。

选择该方案而不是 `with_content_mask` 变通的原因：内容 mask 仍是矩形，无法表达弧；给色条自身加圆角不能裁兄弟/父级；缩小色条宽度会破坏“页面底边比例色条”的已确认几何；隐藏色条直接违反验收。角擦除 + 描边重画只在最外 6px 的四角处覆盖像素，语义上等于真正的圆角裁切，且不改变任何布局、命中区域、滚动或 tooltip 拟合。

## 验证

自动检查（本 worktree，`CARGO_BUILD_JOBS=1`，经 `.agents/task/` 的互斥包装）：

- `cargo fmt --all -- --check`、`git diff --check`：通过。
- `cargo check --locked --all-targets`：通过。
- `cargo clippy --locked --all-targets -- -D warnings`：通过。
- `cargo build --locked`：通过。
- `cargo test --locked`：590 / 590 / 54 / 156 / 135 / 490 / 16 全部通过。

新增/相关用例：

- `ui::island::tests::erasers_cover_the_corner_square_outside_the_island_arc`：四个擦除块的 bounds 恰为对应角方块、曲线中点在 island 半径上（不是直斜边）、子元素自己的方角确实落在弧外；即它验证的是“该擦的区域被擦、不该擦的区域保留”。
- `ui::island::tests::narrow_and_degenerate_islands_clamp_instead_of_panicking`：窄窗、零高、零面积时半径被钳到半宽且不产生反向几何。
- `the_rounded_corner_mask_does_not_move_the_island_or_its_page`（`src/ui/tests.rs`）：真实 Shell 在 1280×820 与 880×560 下，island 到窗口右/下的间距仍是 `ISLAND_INSET`、左边界仍是侧栏宽 + `ISLAND_GAP`，页面滚动区域仍正好落在 1px 边框之内——证明包装层没有改变布局。

原生窗口验证（真实 GPUI 窗口，非测试平台）：`cargo run --example repository` 的预览新增一个仅供预览的 `Corner clip: on/off` 开关，用与产品相同的 6px 圆角、1px 边框和同一份 `RepositoryPage`（含真实色条）装配。截图为 DPI-aware `PrintWindow` 抓取的真实帧：

- 关闭裁切（修复前行为）：`.agents/output/08-clip-off.png`、窄窗 `.agents/output/12-narrow-clip-off.png`。底部色条在左角自 x=11、右角至 x=1848（窄窗同样）仍是完整紫色方块，方角越过圆弧。
- 开启裁切（修复后行为）：`.agents/output/07-clip-on.png`、窄窗 `.agents/output/11-narrow-clip-on.png`。同一位置变为背景色与 1px 边框弧；色条左右两端分别收在 x≈12/1847（窄窗 ≈12/1315），底部位置、比例、hover tooltip 均未改变。
- 放大的角部对照：`07-corner-on-left/right.png` 与 `08-corner-off-left/right.png`、`11-narrow-on-*` 与 `12-narrow-off-*`。

预览开关只是开发脚手架（和已有的场景选择、Reset、Light/Dark 一样），不进入产品启动路径；产品始终使用带裁切的 frame。**未覆盖**：文件树/Reader 交互与拖拽滚动条的手动复测只在本预览中做了滚轮和窗口缩放的直观检查，未逐项重跑；100% 之外的其他 DPI 档位、真实 PAT 账户内容未在本任务验证；原生验收仍待负责人。

## 后续

- 推送后对当前 head 触发 `@codex review`，走 [PR 审查流程](../pr-review-flow.md)。
- 加 `pending` 等负责人原生验收；不合并、不 auto-merge，`blocked` 由负责人管理。
- 若后续升级到支持非矩形 mask 的 GPUI，可把 `RoundedClip` 换成真正的 mask，删除角擦除。


## 迁移到 Shell 页面集成分支后的组合验证

负责人指定唯一集成目标为 `feature/shell-pages`（PR #42），本 PR #46 不再面向 main。协调者完成 rebase 后，验证 head 为 `e0e211c311425a411adaa21f15f1d4f195aadce1`，实时目标基线为 `96c32464786204ab1ca947f060ed68e0b6f484bc`。原 main 上的组件检查与截图只是历史证据，不等于正式 Shell 已验收。本轮保留 `pub mod home` 与 `mod island`、真实页面接线及 `outer_scroll` 条件；未修改 Profile README Flow 或简介换行逻辑。

在该组合上，经 `.agents/task/cargo-serialized.ps1`（相同 named mutex、jobs=1、worktree 独立 target）运行 fmt/check all-targets/clippy all-targets -D warnings/test/build，全部通过。测试计数为 728 / 728 / 55 / 158 / 135 / 495 / 18。PowerShell 5.1 将 cargo 正常 stderr 当作错误的问题，仅在本地忽略的包装中修正，仍传回 cargo 退出码。

原生验证改用本 worktree 的正式 `target/debug/githux.exe`，通过应用已有保存账户读取真实公开仓库，不读取、打印或导出 PAT。使用 DPI-aware PrintWindow 和按 PID 限定的 Windows 消息进行输入、截图；这是原生程序接收合成鼠标/滚轮消息的验证，不代表手动输入手感或完整账户认证验收。当前显示器 scale 为 150%。

- `shell-repo-return.png`：正式 Shell 的 Repository 页面、文件树、真实 README 与 language bar 同屏，底条两端沿岛圆角收边。
- `shell-language-tooltip.png`、`shell-narrow-tooltip.png`：底边 hover 显示 `Rust 93%`，向上拟合；`shell-reader-scroll-own.png` 验证 README 滚轮改变正文而底条保持原位置。
- `shell-profile-own-ready.png`、`shell-home-return.png`、`shell-repo-return.png`：Repository → Profile → Home → Repository 往返；Profile 资料/pins/贡献仍可见，仓库保留文件与正文。本基线的 Profile README 呈现缺口由 Shell 任务负责，本任务不把它说成已经修复。
- `shell-repository-narrow.png`、`shell-sidebar-collapsed.png`：resize 到 1328×964 设备像素客户区，以及侧栏收起后，文件树/Reader 可见、底条仍贴底且两端不越界。
- 正式 Shell 缺陷反向核验：临时仅绕过 `rounded_island` 构建并运行同一组合，得到 `shell-before-clip.png`，之后恢复源码并重新构建修复版。对照 `shell-repo-return.png`，左下角 `(361,1218)/(362,1218)` 从完整紫色 RGB(169,152,221) 变为背景 RGB(37,37,37)，右下角 `(1908,1218)` 从完整粉色 RGB(220,138,144) 变为相同背景；保留的内侧条段颜色不变。放大图为 `shell-before-clip-left/right.png`、`shell-repo-return-left/right.png`；像素记录 `shell-corner-pixels.json`。这些真实账户截图只保留在忽略的 `.agents/output`，不上传。

按 L002/L011/L012，本轮新增的证据是正式 Shell 页面接线与圆角的组合运行，而非将先前 example 当成正式路由。曾尝试单独组合探针，但原仓库 fixture 不覆盖侧栏/Home/Profile 接口，会给出明确 599；该临时探针已移入本地任务目录，不提交、不作为成功证据。没有新增需要单列的稳定教训；可复用部分已由上述三条覆盖。并行原生窗口同名时需按本任务 PID 限定输入和截图，避免串入其他窗口。

仍未覆盖其他原生 DPI、手动 thumb 拖拽和动画手感；保留 pending，不合并或 auto-merge。最终 CI/review 收据和新 head 留在 PR 与 delivery.progress，不以旧 main 结果放行。
