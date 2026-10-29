# Shell 页面接入（Home / Profile / Repository）

- 2026-10-29，任务 `shell-pages`，执行者：基层实现 Agent（派发会话）。
- 状态：实现、自动检查与本机真实窗口自测完成；等待负责人验收。PR 链接与最终 head 在交付记录中补充。

## 目标与结果

把已合入的 Repository、Home/Feed、Profile Overview 从独立预览接入正式 Githux Shell，形成真实账户下的在线阅读闭环。结果是产品 `target/debug/githux.exe` 默认启动进入真实 Home，可从侧栏仓库打开真实 RepositoryPage，并从命令面板/链接进入真实 Profile；没有另建 fixture 产品页面。

## 改动与决定

- 新增 `src/app/pages/` host 层：Home、Profile 与 Repository 挂载、请求代次、结果队列和窄导航 sink。服务仍由 `AccountServices` 提供；没有新 PAT/HTTP/SQLite。
- Shell 按 `TabId` 保存 `MountedPage`。普通激活/重排保留同一实体；关闭只删除该页；账户/授权上下文变化先丢弃 host，再按导航意图重建并重读。
- `RepositoryPage::suspend` 只停止背景交互，供普通标签切换；授权变化继续走 fresh session 的 `activate`/`rebind` 语义。
- Home 固定 `involves:` + 无时间窗口；Feed 使用服务端顺序、无自动轮询，并遵守返回的 Poll-Interval 刷新窗口。两栏独立发布。
- Profile 先读基本资料，再用服务器 id/canonical login 建立 `ProfileTarget`；pins、滚动一年贡献和 README 随后并行读取。README 的缺失/不确定/不适用使用新增中性呈现状态，不伪装为错误。
- `Route::UserProfile { host, login }` 支持他人 Profile，key 做 host + 大小写归一；只有服务器响应可以更新 canonical 显示名。
- 头像沿用 `app::avatar` 唯一 loader/cache 与 `ui::avatar` 圆形呈现。Home Feed 和 Profile header 只接收已解码图片。
- 正式页面拥有自己的局部滚动；Shell 的外层页面 wheel/scroller 现在只用于 Settings 与留空页面。
- 审查发现并修复两处问题：Home/Profile 的同步 action 转发会在组件更新栈内重入 host，改为 frame 后派发；Feed 的 Poll-Interval 到期后原逻辑不会恢复刷新，现用独立 deadline 只恢复按钮可用性，不发请求。两者都有回归测试。

## 验证

本 worktree 独立 target、`-j 1`：

- `cargo fmt --all -- --check`：通过。
- `cargo check --locked --all-targets`：通过。
- `cargo clippy --locked --all-targets -- -D warnings`：通过。
- `cargo test --locked`：主 binary 715 项、其余 target 分别 715/55/157/135/488/16 项，全部通过。
- `cargo build --locked`：通过。
- `git diff --check`：通过。

新增正式集成测试使用临时 SQLite、内存凭据、可控 transport、真实 `Accounts`/`Shell`/页面组件，覆盖真实 Home/Feed façade、侧栏仓库页、Profile canonical 身份、同资源激活/重排、关闭页隔离和重新验证后旧 host 丢弃。

本机 Windows 正式 `target/debug/githux.exe`：

- 启动 PID 42100，标题 `Githux`，窗口句柄有效，Responding；随后主动关闭。
- 初始 Home 在真实 `involves:` 读取完成后显示 Issues/PRs；Feed 有真实事件与真实头像。
- 侧栏仓库行打开真实 README、分支/提交、文件树、Releases/Contributors、Issues/PR 计数和底部 language bar，Home 标签保留。
- 命令面板进入 Profile，显示真实头像、公开资料、README、Pinned repositories；页内滚动到达 pins。截图只写入被忽略的 `.agents/output/`，未提交。
- 产品 exe 中未发现 `Account wiring preview` 或 `Overview loading` 这类开发场景字符串。

## 未覆盖与后续

- 负责人真实 PAT 输入、完整登录/切账户验收尚未执行；本任务没有读取或导出任何真实凭据。
- 贡献热力图底部在合成滚轮下未到达；Profile 现有组件测试覆盖贡献 section，但真实原生滚轮手感仍需负责人确认。
- IME、多 DPI/跨屏、触控板、真实窗口聚焦与拖拽手感未由本轮自动化证明。
- Settings 仍为过渡实现，必须独立设计；本轮合并不代表 Settings 最终验收。
- Issue/PR 详情、通知、搜索与写操作未实现；Star/Fork 没有成功回调，不宣称写能力。
