# 仓库浏览页面：真实数据阅读切片

更新：2026-10-28。任务 `repository-browser`，worktree `D:\githux-repository-browser`，分支 `feature/repository-browser`，基线 `913eb5e`（main，PR28，含 sidebar PR20、reader PR27）。

## 交付内容

- `src/ui/repository/**`：`navigation.rs`（纯导航状态、`TreeModel`、错误分类、分支选择与完整性说明）、`mod.rs`（页面视图、代次/授权/发布、后台任务队列）、`view.rs`（摘要、树列、阅读器列）、`tests.rs`（18 项页面测试）。
- `examples/repository.rs`：确定性脚本传输 + 临时账户目录 + 合成凭据的独立预览，跑真实服务管线。
- `src/bin_support/repository_modules.rs`：供 example/test 复用的薄 `#[path]` 模块装配。
- `tests/repository.rs`：集成入口，让 `cargo test` 真正执行页面测试。
- `docs/repository-browser.md` 与本文档。

未改动 `src/ui/mod.rs`、`src/app/mod.rs`、`app/accounts`、workspace、既有 reader 或共享 scroll 文件；无 Cargo.toml/Cargo.lock 变更。

## 关键实现取舍

- 一次浏览代次 = 一个已验证 `RepositorySession` + 单调页面代次 + 分支/commit/资源身份。`RepositoryPageServices` 只注入 `Arc<AccountService>`、`Arc<BackgroundExecutor>` 与 `RepositoryLocator`；页面自身不打开数据库、不读凭据、不取 PAT。
- 慢操作经既有 executor，结果入锁队列、GPUI 侧按 ~30ms 节拍排出（`start_receiver`/`drain`），无 `block_on`、render 内无 I/O。每条结果发布前核对页面代次、捕获的 `RepositoryReadContext`、分支、commit 与阅读目标身份；`check_authorization` 在无消息时也复核 session 的 `verify()`，所以 A→B→A 后旧数据不会残留。
- 分支切换重新解析 `RevisionSpec::Ref` 到新固定 commit，旧树/README/正文立即作废；`TreeModel` 只保存已加载目录，展开才发一次 `git/trees/{sha}`，绝不带 `recursive`，无全树下载、无文件系统 checkout。
- 空态诚实：完整空根树才给「No README」克制空阅读器；404 是 `ReadmeUncertain`；409 是「Git 数据为空或不可用」；截断/Partial 树与分支带独立说明；auth/SSO/限流/网络分别归类。重试只有用户点按的单次有界操作。
- 阅读器链接由页面接管：同仓库同 commit 相对链接页内跟随；外部 URL 转交主机；跨库/跨 commit/危险 scheme 拒绝或报告。
- 树视图用真实 `uniform_list`（只建可见行），滚动与条用共享 `ui::scroll`（`Motion`+`Bars`+`Bar`），阅读器滚动仍归 reader 自身。

## 验证

本机 Windows（`CARGO_BUILD_JOBS=1` / `-j 1`，独立 `target/`）：

- `cargo fmt --all -- --check` 通过
- `cargo check --locked --all-targets` 通过
- `cargo clippy --locked --all-targets -- -D warnings` 通过
- `cargo test --locked`：main binary **519**、account 装配 **310**、reader **123**、repository **409**（含本任务 18 项页面测试 + 复用 reader/scroll 套件）全部通过
- `cargo build --locked --example repository` 通过；`git diff --check` 通过

页面测试覆盖：真实管线渲染 README Markdown 再渲染选中源码、惰性树一次只发一个 tree 请求且不重复取根树、切换分支丢弃更慢的旧快照、放弃的文件晚完成不覆盖当前、授权变化清空且 A→B→A 不恢复、视图 drop / deactivate 后迟到成功与错误不发布、Partial 分支与截断树不冒充完整、无 README 的克制空态、README 不确定与失败分离、空/不可用仓库 409、Unsupported/TooLarge 诚实态、单次有界重试、两页独立分支/路径/树/滚动、rebind 保留导航意图并在新授权下重读、长树有界且只滚自己、行数上限、链接同 commit 跟随与外部 URL 转交。

真实窗口（PrintWindow 取帧）：`examples/repository.exe` 独立启动，窗口标题 `Githux · Repository page preview`、进程 Responding；取到真实渲染帧：摘要（`octo/demo`、Public、stars 128 · forks 9 · open issues & PRs 4、描述）、Files 树列（README/docs/src/README.md，README 选中）、固定 commit 短号 `c0ffee0`、README.md 头部与真实 Markdown 阅读（标题、段落、内联代码、链接、rust 代码块）。首帧曾暴露 fixture 里 README blob 的 `size` 与真实正文不一致（384 vs 实际字节），使真实 pipeline 正确报 `blob size differs from descriptor`；修正 fixture 尺寸后 README 正常渲染。这证明预览运行的是真实校验链路，不是镜像常量。

## 未覆盖 / 待验收

- 真实 Windows 输入：树行点击展开/选中、分支下拉、滚轮平滑与 thumb 拖动手感、README 链接点击转交。测试平台不经过 Windows 原生输入分发，不能替代负责人真机验收。
- 正式 Shell 接线（`src/ui/mod.rs`/`src/app`）与账户切换时标签处理仍属后续集成任务；本任务只交付可复用页面与精确的 mounting/teardown 契约（见 `docs/repository-browser.md`）。
- macOS/Linux 原生验收未做（只保编译）；真实私有仓库/SSO/限流联调仍须负责人提供 PAT。

## 交接

后续 Shell 集成只需：打开账户服务与 executor 后构造 `RepositoryPageServices`，`cx.new` 页面并 `activate`；切账户/换授权时 `rebind`（或重建页面并传入保存的 `navigation()`）；关闭会话时 `deactivate` 后释放。公共 API 与生命周期见主题文档。
