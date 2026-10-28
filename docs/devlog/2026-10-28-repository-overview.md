# 2026-10-28 repository-overview

任务：`repository-overview`。目标：实现可复用的 Releases / Contributors / Languages 仓库顶部资料组件，使用已实现的 `domain::repository_extras` 模型；独立于 repository-browser 的 Code 组件。

## 结果

- 新增 `src/ui/repository_overview/**`：`RepositoryOverview`、`OverviewBinding`、`ReleasesData`、`SectionState`、`OverviewAction`、`OverviewUpdate`、确定性 fixtures。
- 新增 `examples/repository_overview.rs` 与 `tests/repository_overview.rs`，通过 `src/bin_support/repository_overview_modules.rs` 的薄 `#[path]` 装配复用真实组件、`domain` 与公共 `ui::scroll`。
- 新增 `docs/repository-overview.md`，记录输入/动作 API、三段语义、布局/有界策略、预览命令、验证边界与后续正式接入步骤。
- 未修改 `src/ui/mod.rs`、`src/app/mod.rs`、`src/workspace/**`、`src/ui/page.rs`、repository-browser、Cargo 配置、CI 或 `docs/README.md`；正式产品模块树不导入该组件。

## 行为要点

- `OverviewBinding` 固定账户、仓库和宿主 generation；`apply_*` / `apply` 对不匹配绑定返回 `false` 且不改变当前状态。组件不读取凭据、不调用 HTTP、不下载 asset、不打开 URL、不写缓存。
- Releases 的 latest 使用 GitHub latest 语义；`None`/404 使用“无可见 latest，不证明仓库没有 Release”的歧义文案，列表仍显示 Release。draft/prerelease 标签保留；列表有界；Release body 只进入 bounded plain-text popover，不渲染 Markdown。
- Contributors 区分 204 `EmptyRepository`、成功空内容与错误；显示 `observed_entries()`，区分关联用户与匿名贡献者，不显示 email/raw JSON，不下载头像；大量贡献者只画有界行数。
- Languages 显示字节派生 share 与 bytes，`total_bytes == 0` 不推断 0%；名称省略号、颜色取主题 chart 色板、文字标签随 bar，注明舍入不保证总和。
- TOP 横条布局：宽窗口并排，窄窗口在顶部区域内换行/堆叠；行数按宽度收紧（宽 3 / 窄 2），组件高度有界并给下方阅读器留空间；不建立右侧纵栏。popover 使用公共 `ui::scroll` 的 `Motion` / `Bar` / `Bars`；关闭时取消 motion，避免与相邻阅读器竞争。

## 验证

执行命令与结果（本机 Windows，工作树 `D:\githux-repository-overview`）：

- `cargo fmt --all -- --check` 通过。
- `cargo check --locked --all-targets` 通过。
- `cargo clippy --locked --all-targets -- -D warnings` 通过。
- `cargo test --locked` 通过：主 binary 519 项、account 310 项、reader 123 项、repository_overview 44 项，0 失败。
- `cargo build --locked --example repository_overview` 通过。
- `cargo build --locked` 通过。
- `git diff --check` 通过。
- `cargo run --locked --example repository_overview` 实际启动到被外部 `timeout` 终止（退出码 143），说明 example 入口能进入事件循环；未做人工视觉/交互验收。

GPUI 测试覆盖：旧仓库/账户/generation 拒绝、单 section 失败/Partial 不影响其他段、latest 404 歧义、draft/prerelease 标签、匿名与大量贡献者及有界行、204/空内容/错误分离、零语言、长名称与舍入、popover 打开/关闭/焦点/Escape/外部点击、popover 滚动不泄漏到相邻模拟阅读器、回调使用当前绑定且每次点击一次、桌面与窄窗口 TOP 有界布局。

## 审查修复轮次（PR #37）

首轮 Codex 审查（commit `c90fd877c3`）提出两条 P2，均属可直接修复：

1. notes popover 只按组件底边定位，短/窄窗口下可能伸出窗口；修复为按 viewport 可用空间 clamp 面板宽度、高度和左上位置，并把面板实际尺寸传入 `notes_panel`。
2. popover 只 `track_focus`，Tab/Shift+Tab 可把焦点移出到后方 overview/阅读器；修复为在面板 `on_key_down` 中消费 `tab`，与既有 sidebar menu 的焦点包含方式一致。

修复后新增 `notes_panel_fits_a_short_window_and_traps_tab`，覆盖 480×700 窗口下面板完整落在 viewport 内，以及 Tab 后 popover 仍打开、Escape 正常关闭。重新执行 `cargo clippy --locked --all-targets -- -D warnings`、`cargo test --locked`（repository_overview 45 项）与 `cargo fmt --all -- --check`，均通过。

第二轮审查指出 480×480 时 row budget 不能约束堆叠后的总高度。现将 TOP 区域限制在 min(360px, 窗口高度一半)，真实 ScrollHandle 配合公共 Motion/Bar/Bars 承载溢出。测试同时覆盖 1200×800、600×480、480×480，要求阅读区至少占一半且末段可滚到。修正模拟 reader 的真实满高 viewport，避免内容高度冒充可用阅读高度。

同轮核对补齐：latest endpoint 独立 typed state/update；payload 仓库身份复核；领域 Partial 与统计延迟提示；匿名名字 fallback；fixtures 仅 example/test 装配；最大 u64 语言计数和溢出错误的 UI 边界；窗口失活取消本地滚动，弱引用按键拦截包含 Tab/Shift+Tab/Escape。

第三轮审查发现列表更新会移除 notes 渲染，但遗留 open_release 和按键捕获。render 在输入树发布前核对选中 release/body，缺失时统一 close_notes，取消 motion 并恢复焦点。新增回归覆盖 Loading、Error、缺少 body、缺少 release 四种更新。最终本地全套测试为 519 / 310 / 123 / 48 项。首次重跑遇到已有 localhost bounded transport 测试一次连接失败（Network 而非 ResponseTooLarge）；未改动该范围，随后完整重跑通过，记录此瞬时环境失败。

## 未执行 / 后续

- 未做真实 Windows 预览窗口的人工视觉、鼠标手感和多 DPI 验收；`cargo run` 启动事实不等于视觉验收。
- 未接入正式 Shell 或 repository-browser；未调用真实 `RepoService` / `repository_extras` 读取；未读取真实 PAT 或访问真实 GitHub。
- 正式接入仍须宿主构造绑定、发起独立三段读取、发布前复核 `is_current`、处理 typed 动作，并把组件放在仓库页 TOP 区域。边界与建议步骤见 `docs/repository-overview.md`。


## 负责人要求的紧凑视觉重做（同一 PR #37）

负责人 2026-10-28 拒绝此前卡片/滚动/notes 密度，明确以任务提供的 02-repository-header.png 为层级参考。本轮先读取实际图片，再重写为自然高度横向摘要条。原方案与此前“交付”记录仅作历史，不代表验收通过。

移除三卡片、固定半窗高度、内部 scroll、release history、notes/popover、Open/Notes 按钮、常驻 refresh 和 byte/API 段落。Releases 只显示 supplied latest 版本 link、可选宿主日期和必要 flags；Contributors 为最多 5 个头像，匿名不可导航；Languages 为一个 segmented bar、最多三项 legend，长尾合并 Other，checked byte 运算。保留账户/仓库/generation 及 payload identity 门槛、独立失败/Partial、404/204/空值区别。Ely 支持 host-decoded RenderImage，新增绑定范围内的图像输入；没有新增网络/图像获取系统或伪造照片。组件自然高度嵌入契约和宿主 image/date 依赖见 topic doc。

example 默认短值，压力场景显式切换，下面少量有意义文档说明代替 80 行占位。未复制 browser/header/tree 产品实现。薄装配不再导入 shared scroll，因为该组件已无滚动 owner。

同步 origin/main（最终验证基线 8de19593e6a4b9342aafa79074759d9a93231a29），保留其 Home/page-data/reader 工作，没有竞争写入。fmt、locked all-target check、clippy -D warnings、完整 cargo test、product build、example build、diff check 均通过；全套 538 / 321 / 54 Home / 131 reader / 13 overview-target（其中 8 个组件用例）通过。构建过程中原生 preview exe 仍打开曾导致 Windows LNK1104；关闭本任务 PID 后构建通过，无代码/依赖绕行。

真实本机 Windows 144 DPI，以 DPI-aware PrintWindow 捕获并实际查看 Ready、窄窗、长名称/300 人/20 语言、latest 404、Partial、单段 Error、204、Zero 场景。鼠标真实切换场景。将 Ready 与原始参考并排生成 comparison.png 并检查：三个小组位于 identity/description 与 Code 标记之间，常态约 80 logical px，无卡框/固定半窗/内部滚动，正文空间主导。窄窗两行分组；长名称省略但 share 与 Other 可读。初版非 DPI-aware capture 裁掉右侧，修正 capture DPI 后重新核实，未将裁剪图当交付证据。最终截图保存在 .agents/task/{ready,narrow,long,latest,partial,error,empty,zero}-native.png；comparison.png 为原图与实际 native 的并排检查材料。没有上传原始日志、凭据或私人记忆。

仍待负责人视觉验收；没有声称 IME、多显示器 DPI 切换或原生辅助技术已验收。正式 Shell/repository-browser 挂载、账户服务和图片获取仍为后续整合。保持 owner-managed blocked，添加 pending 仅表示新预览已准备好，不合并、不 auto-merge。新 head 重新请求 Codex 审查；blocked 状态与 CI/review 分开报告。
