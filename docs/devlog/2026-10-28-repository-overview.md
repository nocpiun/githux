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


## 合并任务与统一页面集成（由 browser 执行）

负责人明确将两任务合并，overview worker 已停止。协调者以 `0d30185` 将 compact head `d70b8ab` 合并进 browser；本轮保留该提交及全部历史，并整合已合并 main（含 Profile #36），没有复制旧卡片实现。PR35 成为唯一交付/联合验收目标；PR37 在组合提交推送并核验祖先后关闭为 superseded，保留 branch/worktree/review 历史。

`RepositoryPage` 内嵌一个实际 `RepositoryOverview` entity，位置为描述后、页内 Tabs 前，自然高度 flex_none。使用 AccountService::page_session 的四个独立 extras 服务调用，捕获并比较与 RepositorySession 的完整授权 context。每端点取消/代次与 Code 分支代次分开；切同仓库分支不重读 extras，Retry 不重置文件/reader。旧授权、旧仓库、旧 endpoint 结果及 action 都拒绝；动作保留 typed target，仅预览报告不自动开网页。头像无 host image 时继续 initials fallback。

携带 PR37 `d70b8ab` 审查意见修复：独立 latest Ready 后忽略成组快照的旧 latest_error，新增回归。旧 notes/popover 审查对应路径已按 compact 规格删除，历史不抹除。browser 的保存 ref 大小写问题同时修复（Git 分支名区分大小写）。

统一预览 `examples/repository.rs` 与组合测试使用同一 request-routed fixture；新增 overview loading/partial/error/zero/long/latest404 场景，同一场景选择器。原生截图已实际检查 desktop 和 880×560 client：紧凑三组与真实树/源码同页，reader 保留剩余空间；原生 source 重复点三次及 main→feature→main 时 summary 保持，Languages 独立失败/Retry 的页面组合可操作。最终构建/全套验证与 PID/head 交接以 browser devlog 和 PR35 记录为准，不代表正式 Shell 或负责人验收通过。


组合最终验证（本机 Windows、独立 target、指定 run-additional-cargo.ps1 串行 helper）：fmt/check --locked --all-targets/clippy -- -D warnings 通过；全套测试 538 product / 321 account / 54 Home / 152 Profile / 131 reader / 469 combined repository / 14 overview 全部通过。产品与 repository example 构建、diff check 结果记录于 PR35 最终交接。只启动统一 repository.exe，不启动独立 overview 验收窗口。负责人联合视觉验收仍待进行；blocked/pending 不变，无 merge/auto-merge。


## Owner 四项布局修订（PR35，后续统一验收）

branch从标题右上角移到Tabs右侧；顶部删除stars/forks/REST issue+PR合计，原位置改Ely Star/Fork无数字按钮，仅typed binding宿主动作入口（无写API，未提供callback时禁用）。Issues与Pull requests分别紧邻badge，数据是独立的bounded GraphQL OPEN connections.totalCount，校验databaseId；复用既有query/账户scope，部分错误按字段失效，未知/loading/error隐藏badge，真实0保留。固定query排除closed/merged且open PR含draft，未用列表枚举/Search/合计相减。相邻只读模块为domain/github repository_counts及PageReadSession最小方法，未扩展Issue/PR列表或mutation。

顶部overview现在只画Releases/Contributors；同一languages数据改页面底边4px纯条，每语言独立色块+完整名/比例tooltip，取消Other/legend/标题/常驻重试。零/无数据/失败不画假分布，checked总量保持。focused example同步接口但联合验收仍只有repository.exe。

适用review同步修复：Retry branches运行中禁用以防重叠；revision失败的页面Retry复用已有details/overview，仅重试revision。回归保证同一reader和overview未被重建。

本轮实际查看DPI-aware desktop和880×560截图，布局符合新结构，底条横跨树+reader且不占文本行；原生hover时当前前台被外部窗口占用，未可靠送达，未据截图声称tooltip已真机通过。GPUI实际事件测试确认Rust tooltip在560px窗口内向上出现、移开消失；Star鼠标与Enter/Space各一次，旧binding拒绝，preview无mutation。最终完整checks与PID/head见PR35交接。

四项修订最终验证：指定串行 helper / own target / jobs1，fmt、locked all-target check、clippy -- -D warnings、完整 tests（541 product /324 account /54 Home /152 Profile /131 reader /478 repository /14 overview）、product/repository example build、git diff --check 全部通过。计数data门控验证响应暂停期间A→B→A使旧成功/403均StaleContext；Long场景含大badge和可选择的长分支。最终审查/CI/PID/head以PR35交接为准。

本轮a0b369c审查追加修复：README uncertain面板新增Retry README，retry_reader也将Uncertain路由到发现接口；只重试readme，不清空树/overview或重新解析commit。回归验证成功恢复及其他资源请求不增加。

README uncertain修复后完整复跑：541/324/54/152/131/479/14项全部通过，repository单独再跑479项通过。仍仅PR35待负责人验收，blocked不移除、不合并。

当前head c7e9177审查clean但Linux CI揭示旧端点测试时序假设：真实新Languages请求可能已完成，不能断言仍Loading。将测试隔离到受控发布队列，取消旧工作/推进代次，只注入过期错误并核对当前Loading不变；不改生产逻辑或网络层。随后重新验证/审查。


## Owner五项UI调整（公共Pill/Latest/从左分组/Tab图标/树留白）

新增共享`src/ui/badge.rs`，窄改编锁定Ely普通Badge渲染，文件头保留30c66fe来源及完整MIT版权/许可；仅Filled/Outline Pill+公开Tone语义映射，无上游私有API、无vendor/依赖改动。静态标签不拦焦点/点击，不隐藏0/不自行99+。仅挂接正式UI与repository/overview装配并迁移本轮Badge。

版本前Ely Tag，可信稳定latest后Success Outline Latest；draft/prerelease不丢失且不冒充稳定Latest，独立latest优先，404/error/Loading/空/旧仓库不显示。完整Release观察数量用Neutral Pill，Partial不显示伪总数。Releases/Contributors取消190px basis+grow，按内容从左约32px紧随，右侧自然留空。五tab加对应Ely图标，icon+label+count，父tab点击/underline保持。

树滚动区外四边7px留白，内实际viewport含uniform_list+wheel+共享Bar同一target；行仍填满内宽，行高/缩进/scroll核心不改。新增实际Home/End回归验证首末行完整且有外间距；初次测试失败定位为新增外viewport缺debug selector而非滚动缺陷，补定位标记后通过。

真实DPI-aware桌面暗色和880×560明色统一页截图已实际查看：Tag/绿色Latest完整圆弧、数量/Private胶囊居中，Contributors紧随左组，5tab图标齐、branch右置，选中行四周有留白。原生鼠标点击Light/Dark已送达并观察实际明色帧；Long场景尝试后前台转移，未把未可靠送达的进一步输入写为已验。原有底条tooltip/计数/StarFork/账户隔离契约保持，正式Shell仍未挂载。

本轮最终验证：指定helper串行fmt/check/clippy -D warnings、完整tests 541/324/54/152/131/483/16、product与repository example build、diff check全通过。实际Win32鼠标/键盘又核验880×560明色Long场景：长tag省略+完整Latest、大计数123456/98765胶囊与tabs不挤坏；Long tree中真实End滚到file-0399，末行圆角/底边留白完整，README/source状态未被树滚动改变。已切回Normal暗色。最终head/PID/CI/review见PR35交接。


## README容器/正文inset与Star/Fork内部计数（含现场底padding回归）

保留全部未提交修改继续同一PR35。发现原Markdown根px_2/py_3使list视口固定缩小；初次把py_3移到list仍不正确：GPUI原生wheel/set_offset考虑padding，但max_offset_for_scrollbar不包括，Motion/thumb因此clamp掉底部inset。按负责人现场回归改为首尾真实AST块测量padding，完整计入item总高，不假节点/不恢复外层空带。scroll_to_end的item_count临时末锚点也在repository模式等待惰性尾测量，避免按旧reach过早clamp。左右原间距+4px。默认宿主保持原值，source行号/排版无修改。

ReaderView新增MarkdownLayout Default/Repository及一次性setter，show保留配置；相同setter无动作、不重建文档/选区。Star/Fork单一复合div按Ely token绘制icon+label+公共Pill数量，有效details128/9直接读，无新请求；无callback禁用/旧binding保护、Enter/Space/数量点击一次typed action，无mutation/假成功。

GPUI回归：full viewport与首尾内容bounds、无假AST块、重复配置保留选区、短文上下空白清选、段落/列表/代码/表格末块wheel与thumb target到底、source实体/gutter不变；badge count在按钮边框内/点击各一次/0及大值/失效清理，保留旧动作套件。

真实最新构建PID77524（阶段性）已DPI-aware查看880×560 Star128/Fork9按钮内胶囊、README首屏。长段落真实wheel到底，最后句下方清晰留白；长列表wheel到底、表格wheel到底也实际查看。code场景后窗口被系统最小化，恢复后部分输入又遇SendWait Access denied/foreground0；最终code与thumb到底未可靠原生验完，不将中段拖条或标题栏截图当通过。共享目标模拟验证不替代负责人原生手感验收。证据保存在ignored .agents/task/tail-*。

最终构建补验：在PID51648的实际HWND定向投递native wheel及按下/移动/释放消息（不需抢占其他窗口），code末块的wheel/drag到底截图均已实际查看，代码容器下方有明确内容inset再到language条。此为Windows消息驱动证据，和真实设备输入/负责人手感验收分开；此前Access denied缺口未伪造。最终全部checks为545 product/324 account/54 Home/156 Profile/135 reader/488 repository/16 overview，product+example构建通过；末次焦点小修后重跑结果以PR35交接为准。
