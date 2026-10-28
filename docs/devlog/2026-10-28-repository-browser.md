# 仓库浏览页面：真实数据阅读切片（返工）

更新：2026-10-28。任务 `repository-browser`，worktree `D:\githux-repository-browser`，分支 `feature/repository-browser`，PR #35。基线 `913eb5e`（main，PR28）；本轮先整合 origin/main `342e72d`（reader-motion PR32、page-data PR33）再返工。

## 本轮返工背景

负责人看过首版预览后指出：示例无法理解、左侧文件列表体验差。主 Agent 只读核查确认了三类实际缺陷：预览只支持首屏、后续操作假 404；文件树是文字胶囊式行、缺少清晰导航；页面缺少页内 Tabs 层级。原任务已明确本波次由 coordinator 合并，`delivery-state.json` 的 `merge_owner` 已从 `worker` 纠正为 `coordinator`，`arbitration_ever_required` 保持 `false`（不虚构裁决记录）。

## 改动与决定

### 1. 示例可完整、反复浏览

`examples/repository.rs` 原用一次性队列 `pop_front()`，耗尽后统一 404。已改为共用 `src/bin_support/repository_fixture.rs`：

- 按 method / path / query / 分支 / commit / tree / blob 身份应答，同一资源可重复读取，与点击顺序和并发完成次序无关。fixture 自身测试断言“同一 blob 连读三次都成功”“脚本失败只触发一次”。
- Normal 含两条真正不同的分支（commit、根 tree、README blob 与 `src` tree 均不同）、根 `README.md`、`docs/guide.md`、两层嵌套 `docs/guides/nested/deep-note.md`、`src/main.rs`、`assets/`（二进制 `logo.bin`、空 `empty.txt`、3 MiB `huge.log`）、symlink 与 submodule 行。所有展示的普通目录/文件都可操作。
- 未建模请求返回 `599` 并打印开发诊断，明确写出“请补 fixture，不要当成 GitHub 权限结果”，**不再伪装成产品错误**。
- README 正文改为小型正常仓库的说明，技术说明移到预览工具栏，不再冒充仓库简介。

### 2. 预览场景与验收共用数据

- 工具栏（页面之外）提供 Scene 下拉框 + Reset + action 输出，默认 Normal；场景含 EmptyRepository、ReadmeUncertain、NoReadme、DirectoryFailure、FileFailure、NoBranches、TruncatedTree、LongTree。
- 场景切换重置 fixture 与请求代次，不把前一个异常的失败计数带进下一个场景。`DirectoryFailure`/`FileFailure` 的脚本失败只消费一次，单次 Retry 即可恢复。
- **测试改为直接使用同一 fixture**：原先测试与预览各用一套 transport 的问题已消除，`tests/repository.rs` 与 `examples/repository.rs` 现在共享 `repository_fixture.rs`。
- 工具栏、场景数据、临时账户、合成凭据、外部链接只报告等逻辑只存在于 example 与 bin_support，产品 binary 不声明这些模块。

### 3. 文件树重做为行式导航

- 每行 `w_full` 填满树视口，统一行高 26px、统一左右边距；hover/选中背景覆盖整行可点击区，不再是按名称长度收缩的胶囊。
- 选中由 `current_path()`（稳定完整路径）决定，不用父目录内 index，跨目录/跨分支都不会错选。
- 目录有独立 16px chevron 槽位 + 目录图标 + 名称；文件在该槽位留空对齐。缩进 `INDENT * depth`，目录优先、同级稳定排序保持。
- 单击目录行或 chevron 都只切换一次（chevron 用 `stop_propagation`），且不清空右侧已打开正文；单击文件行打开文件。
- 长名称用 `EllipsisTooltip` 单行截断并给完整路径；键盘焦点用单独中性描边，与 hover/选中三态可区分。
- 目录自身状态：`loading…` / `empty` / 失败原因 + 该行内 `Retry`，其他目录与正文不受影响。
- 键盘：树聚焦后上下移动、左右展开折叠或回父节点、Home/End、Enter 打开；只消费无修饰导航键，带修饰键留给平台/IME/阅读器。行移动通过列表自身 `scroll_to_item` 滚入视口。
- 保持惰性加载与 `uniform_list` 虚拟化，树和正文各用共享 `Bars`/`Motion`。

### 4. README 去重

移除混在文件列表中的伪“README”行。默认 README 成功后对应 descriptor 的真实 `path`（根 / `docs/` / `.github/` 均可）；选中视觉与手动打开同一文件一致。阅读器头部显示仓库相对路径；“Back to README”只在“有可用 README 且当前显示其他文件”时出现（`can_return_to_readme()` 读分支的 discovery 状态，文件选择不再清掉它的判断依据）。

### 5. 页内 Tabs 层级

新增 `page_tabs.rs`：仓库摘要 → 横向页内 Tabs（Code / Issues / Pull requests / Releases / Settings，Code 默认选中）→ Code 区域。只有 Code 有内容，其余内容区留空（`repository-tab-empty`），不加占位卡片或假设置。切出 Code 停止绘制树与正文并结束阅读器手势，切回时复核授权并恢复导航意图。commit 短号降为树列头部的小字次要信息。

### 6. 错误按资源分层

`PageError` 不再在描述里断言作用域；404 文案改为“该资源不存在或当前账户无法访问”，不再写“仓库对账户不可见”。仓库详情/分支/revision 失败才是页面级；目录失败定位到该行；blob/文件失败只替换阅读区；README 发现失败独立。`retry_directory` / `retry_reader` / `retry_readme` 各自只重试一个资源，不重载整页。

### 7. Codex 首轮审查修复（同一 head 的 4 项）

`1a55a52` 的 Codex 审查提出 4 项，均为本轮范围内的真实缺陷，已逐项修复并加回归测试：

| 级别 | 问题 | 修法 |
| --- | --- | --- |
| P1 | `activate()` 在 `session()` 失败时提前返回，保留了上一个账户的摘要/分支/树/正文（`active=false` 后 `check_authorization` 不再清理，渲染又因 `selected_branch()` 存在而抑制页面错误） | 在获取 session **之前**清空所有授权绑定状态，失败路径也保持已清空；新增测试：移除账户后 `deactivate`+`activate` 必须无残留 |
| P1 | A→B→A 时两次 A 解析共用一个页面代次与授权上下文，`begin_branch()` 不推进代次也不取消旧工作，branch-name 检查会接受旧的 A 结果 | 新增单调 `branch_request` 身份，`Job::Revision`/`Body::Revision` 携带它，`apply_revision` 先比对该身份再比对分支名；新增 A→B→A 测试 |
| P2 | 打开文件后展开任意未加载目录会经 `apply_directory → resolve_selection` 无条件重取同一 blob，正文回到 Loading 并重建 `ReaderView`（丢滚动与选区），目录失败也触发 | `resolve_selection` 在“当前就是 Showing 且 reader 的 path 等于当前选中路径”时直接保留；新增测试验证同一 reader 实体存活且 blob 未被再次下载 |
| P2 | 相对链接指向不存在的祖先目录（或恢复的路径在新分支上父目录已不存在）会永久停在 Loading：`child_directory()` 对“父已加载但没有该目录条目”也返回错误，却被当成“父未加载” | 区分“待加载的父目录”与“不可用的祖先”：后者立即发布终态；目录加载失败时，若当前选中路径正等待该目录，也发布终态。新增测试验证无下载且不留 Loading |

## 验证

本机 Windows，独立 `target/`，`CARGO_BUILD_JOBS=1` / `-j 1`：

- `cargo fmt --all -- --check` 通过
- `cargo check --locked --all-targets` 通过
- `cargo clippy --locked --all-targets -- -D warnings` 通过
- `cargo test --locked`：main binary **538**、account 装配 **321**、reader **131**、Home **54**、repository **453**（其中本任务页面行为测试 36 项、导航模型测试 4 项 + fixture 自测 6 项）全部通过
- `cargo build --locked --example repository` 通过
- `git diff --check` 通过

页面测试现在与预览共用同一 fixture，覆盖 normal 首屏、嵌套目录展开/收起/再展开只请求一次、同文件重复打开 3 次、README→源码→README、分支真实差异与切回、目录/chevron 单次切换且不清空正文、键盘导航不吞修改键、README 相对链接跟随、外部链接只报告、页内 Tab 切换清空与恢复、目录失败与文件失败的作用域与单次重试、无 README / 不确定 / 409 / 空分支 / 截断树 / 长树、场景重置恢复正常、Back to README 显示条件、慢旧请求不覆盖新选择、授权 A→B→A、失活、rebind、两页独立。

**真实窗口**：`target/debug/examples/repository.exe` 启动，标题 `Githux · Repository page preview`、进程 Responding，`PrintWindow` 取到真实渲染帧：预览工具栏（场景按钮 + 说明 + action 行）、`octo/ledger` 摘要与描述、页内 Tabs（Code 选中并带下划线）、Files 树（assets/docs/src 目录、link-to-readme 符号链接、README.md 选中行、vendor submodule）、commit 短号 `c0ffee0`、README.md 头部与真实 Markdown 渲染。首帧曾发现 fixture 的 README blob `size` 与真实正文不一致（384 vs 实际字节），真实管线正确报 `blob size differs from descriptor`；修正后正常渲染——这证明预览走的是真实校验链路而不是镜像常量。

**原生交互补验（本轮最终修复工作区）**：采用 DPI-aware `SetThreadDpiAwarenessContext(-4)` 后取得完整 1882×1286 物理像素帧，分支下拉与 Markdown 均完整可见；此前非 DPI-aware 取帧的右侧裁切不能作为实际布局缺陷，也不能仅靠测试布局推断真机已通过。新取帧确认了真实窗口布局。只操作本任务 exe/PID，未关闭其他任务窗口。

实际 Win32 鼠标/键盘输入已执行并观察帧变化：

1. Normal 首屏：Code 选中、真实 README.md 单一选中、摘要正常；场景下拉与 Reset 均可见。
2. docs chevron 连按三次（展开→收起→展开），每次只切换一次，README 保留；src 展开后 main.rs 连续点击三次均显示源码，头部路径为 src/main.rs。
3. main→feature/docs-polish→main，feature 分支的 src/feature_note.rs 确实出现，切回消失；Markdown/文件依固定 commit 重读。
4. README 相对链接到 docs/guide.md、再跟随 nested note 到 docs/guides/nested/deep-note.md，祖先自动展开且选中真实节点；外部 Rust 链接只在工具栏记录 `external link (not opened)`。
5. Directory failure 场景打开 docs，行内 Retry 一次恢复子目录，README 始终保留；File failure 场景打开 main.rs 阅读区报错，Retry 一次恢复源码，摘要与树保留。
6. 切到 Issues 内容区空白，切回 Code 恢复当前文件。

截图、临时 native probe 脚本保存在被忽略的 `.agents/task/`，不进入产品或 Git；它们证明上列实际操作，不替代负责人视觉验收。

### 后续审查与验收补齐

- `selection_request` 为每次文件选择/重试提供独立单调身份，旧正文错误也不能覆盖后来同文件成功；分支身份覆盖所有后台消息，切分支取消前轮工作。
- 分支列表失败优先解析已保存分支；新增 Retry branches 只刷新列表，不重建已显示快照和 reader。正文失败保持终态，无关目录完成不触发隐式重试。
- 根目录失败新增 `Retry files`，使用当前 snapshot 的根 handle；不重置资料或已显示 README。
- 子目录截断/Partial 显示 `Incomplete`，只有完整空子目录才显示 empty。
- 默认 README 若在 docs/ 下，按 descriptor.path 展开祖先并选中真实节点；新增该场景和事件测试。
- 实际鼠标/键盘事件测试使用与 example 同一 Normal fixture，验证 chevron、整行空白点击、重复文件、键盘 dispatch；新增 binary/Empty/TooLarge 与空子目录状态验证。
- Markdown 链接 action 在 reader update 结束后执行，并再次核对页面/分支/文件代次，避免取消 reader 手势的回调重入。
- 工具栏改有界场景下拉框；补 Delayed、README in docs、Root failure 场景、长中文文件名与空目录。feature 的 guide 使用独立 blob/tree，正文与分支说明一致。
- 本轮追加整合已合并 main `8de1959`（Home #34），没有从未合并分支取实现。

## 未覆盖

- 未做原生长树 thumb 拖到末尾、触控板、多 DPI/跨屏与 IME 联合验收；这些继续交负责人。上列已执行的真实鼠标/键盘交互不等于负责人体验验收。
- 正式 Shell 接线（`src/ui/mod.rs` / `src/app`）与账户切换时标签处理仍属后续集成任务。
- 真实私有仓库 / SSO / 限流联调需负责人提供 PAT；本轮仍只用合成凭据与本地 fixture。
- macOS/Linux 仅保编译，未做原生验收。

## 交接

新版预览会在最终检查及提交后按最后构建启动并保持打开，PID/head 记录在 delivery-state.json；负责人也可从 `cargo run --example repository` 或 `target/debug/examples/repository.exe` 打开，用工具栏逐个场景复验。PR #35 继续沿用，`pending` 用于本轮重新验收、`blocked` 保持由负责人管理，本任务不合并、不启用 auto-merge。


## Owner 合并 browser + compact overview（统一页面）

本轮接收协调者本地合并 `0d30185`，其中保留 overview compact head `d70b8ab`。browser 为两组件及组合唯一写入者；不再启动独立 overview 验收页面。整合 main `8ab0d5c`（含 Profile #36），保留所有已接受 Home/Profile 代码。

新增 `src/ui/repository/extras.rs`：仓库详情成功后构造一个 RepositoryOverview entity，并创建现有 `PageReadSession`；完整 context 必须等于该页 RepositorySession context，且当前仍有效。四个独立 extras 调用在现有 BackgroundExecutor 执行，Code 继续读取；单端点取消/请求代次/发布队列与分支 Code 代次独立。切分支不重读或丢弃 extras；账户/授权/页面失活清理；旧成功/失败与 stale action 均不能回写。各组 Retry 只更新对应组；typed PageAction::Overview 保留 canonical release/contributor 目标，预览只报告。

统一层级已实际查看 DPI-aware 原生帧：identity/description → compact Releases（一个版本）/Contributors（最多五头像）/Languages（一个分段条+短 legend）→ 页内 Tabs → 真实树和 ReaderView；没有卡框、第二个 header、模拟 reader 或固定半窗区域。1100×800、880×560 client 核验仍有可用文件阅读区域。原生在 880×560 打开 src/main.rs 并重复点三次，再 main→feature→main，extras 保持；错误场景 Languages 不影响成功 siblings 或 README。

新增组合测试重用同一 fixture：四端点首次各一次、分支变化保持 entity/不倍增请求、Retry 保留 reader、partial/latest404/zero、旧端点错误在 retry 后拒绝、deactivate 清理、授权 A→B→A/rebind 旧动作拒绝、两种目标尺寸阅读区约束；overview 自身行为套件继续运行。一次既有 localhost bounded test 碰到瞬时 Network 错误，未修改网络层，完整 repository 重跑通过。最终全量结果见后续验证记录。

历史合并权限：两任务完整已知历史 arbitration_ever_required 均 false，逻辑 OR 仍 false；merge_owner 为 coordinator，blocked/pending 保留。PR35/37 的相关反馈已读取，修复 case-sensitive ref 与 superseded latest_error；旧 popover 反馈沿历史保留，不恢复已删除设计。


组合最终验证（本机 Windows、独立 target、指定 run-additional-cargo.ps1 串行 helper）：fmt/check --locked --all-targets/clippy -- -D warnings 通过；全套测试 538 product / 321 account / 54 Home / 152 Profile / 131 reader / 469 combined repository / 14 overview 全部通过。产品与 repository example 构建、diff check 结果记录于 PR35 最终交接。只启动统一 repository.exe，不启动独立 overview 验收窗口。负责人联合视觉验收仍待进行；blocked/pending 不变，无 merge/auto-merge。


## Owner 四项布局修订（PR35，后续统一验收）

branch从标题右上角移到Tabs右侧；顶部删除stars/forks/REST issue+PR合计，原位置改Ely Star/Fork无数字按钮，仅typed binding宿主动作入口（无写API，未提供callback时禁用）。Issues与Pull requests分别紧邻badge，数据是独立的bounded GraphQL OPEN connections.totalCount，校验databaseId；复用既有query/账户scope，部分错误按字段失效，未知/loading/error隐藏badge，真实0保留。固定query排除closed/merged且open PR含draft，未用列表枚举/Search/合计相减。相邻只读模块为domain/github repository_counts及PageReadSession最小方法，未扩展Issue/PR列表或mutation。

顶部overview现在只画Releases/Contributors；同一languages数据改页面底边4px纯条，每语言独立色块+完整名/比例tooltip，取消Other/legend/标题/常驻重试。零/无数据/失败不画假分布，checked总量保持。focused example同步接口但联合验收仍只有repository.exe。

适用review同步修复：Retry branches运行中禁用以防重叠；revision失败的页面Retry复用已有details/overview，仅重试revision。回归保证同一reader和overview未被重建。

本轮实际查看DPI-aware desktop和880×560截图，布局符合新结构，底条横跨树+reader且不占文本行；原生hover时当前前台被外部窗口占用，未可靠送达，未据截图声称tooltip已真机通过。GPUI实际事件测试确认Rust tooltip在560px窗口内向上出现、移开消失；Star鼠标与Enter/Space各一次，旧binding拒绝，preview无mutation。最终完整checks与PID/head见PR35交接。

四项修订最终验证：指定串行 helper / own target / jobs1，fmt、locked all-target check、clippy -- -D warnings、完整 tests（541 product /324 account /54 Home /152 Profile /131 reader /478 repository /14 overview）、product/repository example build、git diff --check 全部通过。计数data门控验证响应暂停期间A→B→A使旧成功/403均StaleContext；Long场景含大badge和可选择的长分支。最终审查/CI/PID/head以PR35交接为准。

本轮a0b369c审查追加修复：README uncertain面板新增Retry README，retry_reader也将Uncertain路由到发现接口；只重试readme，不清空树/overview或重新解析commit。回归验证成功恢复及其他资源请求不增加。

README uncertain修复后完整复跑：541/324/54/152/131/479/14项全部通过，repository单独再跑479项通过。仍仅PR35待负责人验收，blocked不移除、不合并。
