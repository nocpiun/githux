# 账户服务与每账户 workspace 接入正式 Shell

- 日期：2026-10-28；任务 ID：account-shell；执行者：基层 Agent。
- PR：[#38](https://github.com/nocpiun/githux/pull/38)。状态：首版接线已合入本轮改动；三项负责人反馈（真实头像、自动加载去 Complete/Refresh、Settings 重做）已实现并本地验证，新 head 待推送/CI/Codex 审查与 `pending` 负责人重新验收。不合并。

## 改动与决定

把已接受的账户服务与 UI 桥接接入正式 Githux Shell，替换侧边栏的假账户/仓库数据，并按负责人 2026-10-28 的决定实现每账户独立 workspace。

- 每账户独立标签已确认并落地：`workspace::Sessions` 持有 logged-out workspace 加每个 `AccountKey`（host + 稳定数字 ID）各一个 `Workspace`，`Deref` 到当前可见 workspace。切回某账户恢复该账户自己的标签、顺序与活动标签；全局式标签（如 Settings）也归活动账户 workspace。`TabId` 由窗口内所有 workspace 共享的单调分配器分配，两个账户的相同本地标签编号不冲突。这是进程内内存状态，重启仍从 Home 开始，无 workspace 持久化。详见 [架构方案](../architecture.md) 与 [产品约定](../product.md#外壳与导航)。
- 复用既有 `AccountService`/`RepoService`/`AccountRuntime` 及 generation/`RepositoryReadContext` 复核/取消/对账行为，未复制第二套登录、网络或存储。`app::run` 创建 app 级 `app::accounts::Accounts` 控制器（装在 `AccountsHandle` 全局），后台线程恢复账户目录；`ui::Shell` 只持弱引用读它的快照。`data`/`auth` 等模块的模块级 `dead_code` 允许已随真实接入移除。
- 登录/管理/切换通过 Settings 里最小 Accounts 分区（仅 PR28 已接受的 add/select/replace/revalidate）；无账户删除、登出或缓存政策。logged-out 侧栏是诚实的登录入口、无假仓库；启动恢复区分 `PendingVerification`，不把持久化身份当已核验授权。
- 侧栏账户行、仓库列表与 palette 使用真实账户身份与 `/user/repos` 数据；fixture 只剩 Home 演示列。仓库列表区分 Loading/Complete/Partial/空/错误状态；Partial 行可用且不完整可见，失败不冒充空列表；账户切换先作废旧列表再发布新内容；显式 refresh 只针对该列表（标签条固定工具栏 Refresh 不借用）。
- 仓库会话只保存导航（branch/path），不保存 `RepositoryReadContext`/`RepositorySession`/已读内容/未完成 generation；每 workspace 的 `incarnation` 在可见时递增，返回账户须用新授权上下文重建读取。
- 暴露窄生产访问点 `app::AccountsHandle::services`（账户服务 + repo 服务 + executor + 当前账户与 selection 计数），供下一集成任务挂仓库页；本任务不实现仓库浏览器，仓库标签保留 stub，不伪造浏览器。
- 开发者独立 example 保留（`cargo run --example account`），经 `examples/account.rs` 的薄 `#[path]` 装配直接复用产品 `app::accounts`、`ui::accounts`、`ui::scroll` 与开发者 preview 窗口（`src/bin_support/account_example.rs`）；preview 窗口、场景选择与退出逻辑只存在于该 example 目标，产品 binary 的模块树不含它们（`app::run` 无 preview 分支/flags）。

## 验证

已执行（本机 Windows，base 913eb5e）：

```sh
cargo fmt --all -- --check
cargo check --locked --all-targets -j 1
cargo clippy --locked --all-targets -j 1 -- -D warnings
cargo test --locked -j 1          # bin 538、account 装配 538、reader 123
cargo build --locked --example account -j 1
cargo build --locked --release -j 1
git diff --check
```

测试覆盖：登录/切换/替换/重验/重启恢复、取消对账、Partial 分页、错误不冒充空列表、A/B 账户同一仓库的隔离与 A→B→A 恢复、返回账户不重新发布旧授权内容、视图释放后账户 runtime 关闭。测试用临时 SQLite、合成凭据与注入传输；未使用真实 PAT。

**未执行**：真实 PAT 登录与真实 GitHub 联调，须由负责人在原生窗口输入 token。真实窗口的视觉/输入法/多 DPI 验收仍未做；代码检查与 GPUI 测试平台不替代。

## 后续

- 补 PR 描述、关联 First Release milestone #1、触发 Codex review；通过后加 `pending` 等待负责人原生窗口验收，本 wave 不合并、不 auto-merge。
- 仓库页与 Shell 的最终组装是后续集成任务，依赖本 PR 与 repository-browser PR 先合入 main。

## 负责人反馈轮（2026-10-28，第二轮）

负责人验收后提出三项调整。原任务"不做头像下载"和"显式 Refresh 才读取"被这两项新要求覆盖，Settings 改版也由负责人追加授权给本任务。仍未合并、未验收。

1. **真实头像**：新增 `src/app/avatar.rs`（`AvatarCache` / `AvatarFetcher` / `AvatarBytes` 字节源）与 `src/ui/avatar.rs`（统一头像渲染 + initials fallback）。仓库 namespace 用 `RepoOwner.avatar_url`，账户用 `AccountIdentity.avatar_url`，侧栏、Settings 账户行、footer 共用同一实现。身份为 `host#id + url`，重复 owner/重绘去重，失败终态不重试，请求不带 PAT，只接受 https（本地测试例外）。二进制图片不能走 `HttpResponse.body: String` 的 REST 传输，因此用专用 reqwest 字节客户端，未改共享传输契约。
2. **自动加载 + 去 Complete/Refresh**：`Accounts::autoload_if_needed` 以 `AutoLoadKey{account, epoch, selection}` 去重，在启动恢复、登录、切换、替换、revalidate settle 后各读一次；重绘/resize/collapse/Settings 不触发；失败 key 也记录，避免风暴。侧栏删除 `Complete set of N repositories.` 与常驻 Refresh 按钮，改为加载轻量指示点、失败时一次性 Retry、成功零仓库简短空态。删除了指向"Refresh to read…"的过时提示。
3. **Settings › Accounts 重做**：`Cargo.toml` 增加 `ely-settings`（同一 git URL/rev），`Cargo.lock` 仅新增 `ely-settings` 与传递 `ely-canvas`（`tempfile` 的 windows-sys 在两个允许版本间被 cargo 归一，不是升级）。`src/ui/accounts/form.rs` 改用 `SettingsSection`/`SettingsRow`，每账户一行、去掉重复当前账户摘要与禁用 Current 按钮、添加/替换独立表单组、删除工程说明式长正文。

### 验证（本轮）

`cargo fmt --all -- --check`、`cargo check --locked --all-targets -j 1`、`cargo clippy --locked --all-targets -j 1 -- -D warnings`、`cargo test --locked -j 1`（bin 567、account 装配 567、reader 131）、`cargo build --locked --example account -j 1`、`cargo build --locked --release -j 1`、`git diff --check` 全部通过。

新增测试：`the_repository_list_loads_once_by_itself_and_never_storms`（登录后自动读一次；重绘/resize/collapse 不再读；失败不重试风暴）、`avatars_load_by_identity_and_do_not_cross_accounts`（注入字节源，Ready/Failed 隔离、去重、不串账户）、`the_settings_accounts_page_is_one_compact_list_plus_a_form`（最小 880×560 下账户列表与表单在岛内、无重复当前账户块、有 Current 标识）。`app::avatar` 模块测试覆盖 URL-only https 限制、按内容嗅探格式、404/坏图失败、按身份+URL 去重。

原生窗口：本机启动 `target/release/githux.exe`（PID 38144，Responding，标题 `Githux`），`PrintWindow` 取到真实渲染帧：侧栏显示 `Repositories (110)`、**无 Complete 文案、无 Refresh 按钮**，且仓库行显示**真实 GitHub 头像**（nociun/githux、NriotHrreion 等）。截图 `.agents/output/account-shell-preview.png`。**未验证**：受本机桌面会话限制，模拟点击 Settings 未被接收，Settings 页面与头像加载完成态的视觉仍需负责人在原生窗口确认；真实 PAT 登录仍未执行。

### 后续

- 推送新 head，触发 `@codex review`，按 skill 核验当前 head 的 CI 与审查；`pending` 等待负责人重新验收；不合并、不 auto-merge；`merge_owner` 已纠正为 coordinator。
- 三项之外的设置分类（General/Appearance/Network 等）未实现，未引入 `ely-settings` 之外的设置功能。

## 负责人反馈轮二（头像形状修正）

负责人纠正：头像必须保持**圆形**，从未授权改为正方形；上一轮真实图片接管 initials 后，方形来源图片露出了四角。仅修形状，不动真实头像加载、自动加载或 Settings 结构。

- 根因：`src/ui/avatar.rs::avatar` 的图片分支只给 `ely_core::primitives::Image` 设了 `fit(Cover).size_full()`，注释误称外层 `rounded_full().overflow_hidden()` 已足够。锁定 Ely `primitives/image.rs` 的 `RenderOnce` 会把自身 `base.style().corner_radii` 拷给内部 `gpui::img`，GPUI `Img` 再把 radii 传给 `Window::paint_image`，所以圆角必须落在图片节点本身。
- 修复：图片分支补 `.rounded_full()`。外层 frame 仍是 `rounded_full().overflow_hidden()`，尺寸/`ObjectFit::Cover` 居中裁切/固定槽位不变，因此 initials fallback 与真实图片轮廓一致，侧栏收起/展开的 `Rems` 插值尺寸也继续平滑。改动只在这一处共用渲染器，未改调用方。
- 修复了那条误导注释。

### 验证（本轮）

- `cargo fmt --all -- --check`、`cargo check --locked --all-targets -j 1`、`cargo test --locked --bin githux -j 1`（577 项）通过；release 构建通过。
- 真实窗口：`target/release/githux.exe`（PID 46196，Responding，标题 `Githux`），`PrintWindow` 抓取真实帧后用最近邻放大核对侧栏头像：nociun/githux 蛋糕头像与 opanel 头像四角已被圆裁切、露出侧栏背景，不再有白色/不透明方角。截图 `.agents/output/avatar-circle.png`、放大 `.agents/output/avatar-zoom.png` / `avatar-zoom3.png`。核对后关闭该自启进程。
- 补充独立离线原生样例 `cargo run --example avatar`：左侧显示红色不透明方形源图（绿色中心），右侧调用生产共用渲染器显示 repository、收起/展开过渡、账户及 Settings 对应尺寸，末列为 initials fallback。此样例不打开账户数据库、不联网。原生窗口 PID 72760 的 DPI-aware `PrintWindow` 截图为 `.agents/output/opaque-avatar-circle.png`；四组图片共 16 个圆外角像素均为背景 RGB(24,33,42)，中心均保留源图绿色 RGB(24,224,112)，未裁切源图的角为红色 RGB(255,24,48)。像素记录为 `.agents/output/avatar-pixel-evidence.json`，验证图片圆角实际绘制，而非只核对样式或宽高。
- **未验证**：真实 Shell 的 Settings 页与账户 footer 同屏交互、侧栏收起/展开动画及真实 PAT 输入仍需负责人验收。离线样例验证共用渲染器在对应尺寸的裁切，不能代替这些原生产品交互。样例窗口按已记录 PID 请求正常关闭，未操作其他任务窗口。

### 后续

- 推送修复 head，触发 `@codex review`，核验当前 head CI/审查后置 `pending` 等负责人验收；不合并、不 auto-merge，`blocked` 由负责人管理。

## 审查追加（圆形修复后）

Codex 对本轮 head 的新增意见：分页 Partial 结果只有行可点却没有 Retry，且已记录的 load key 会让之后同授权的 settlement 不再读取，导致缺失页拿不到。`repository_list_status` 现在对 `Partial` 也提供一次性 Retry（完整列表仍只有 `Repositories (N)`，失败仍保留各自一次性 Retry）。Partial 行仍可点击，不掩盖缺失原因。`refresh()` 走既有 `start()`，本次授权会重新发起读取。新增 `partial_results_keep_rows_usable_and_offer_a_scoped_retry` 回归测试。
