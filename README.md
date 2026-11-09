# Githux

纯 Rust 的 GitHub 桌面客户端。本迁移分支使用 [Native Shell](https://nsh.dev/) 的 Windows 原生后端和默认主题，UI 字体为 Noto Sans / Noto Sans SC，代码字体为 Google Sans Code。

## 当前迁移状态

`migration/native-shell` 原位替换默认应用：`src/main.rs` → `src/app` / `src/ui`。根 Cargo 不再依赖 Ely、GPUI 或 QuickJS，也没有第二套 `src/nsh` 产品入口。当前包含真实账号与仓库侧栏、Home/Feed、Profile、Repository/Reader、Notifications、全局 Issues/PRs 和 Accounts/Settings。

这是供验收的迁移分支，尚未合入 main。页面使用现有业务、凭据和缓存服务；原型/探针通过不能替代真实根应用的组合验证。检查与未覆盖交互见 [原位迁移](docs/native-shell-inplace.md) 和 [集成记录](docs/devlog/2026-11-09-native-shell-inplace.md)。首版范围仍以 [产品约定](docs/product.md) 为准。

## Windows 构建与运行

需要 Rust/rustup、Visual Studio 2022 Build Tools 的 Desktop development with C++（MSVC x64、Windows SDK）。`rust-toolchain.toml` 固定 Rust 1.98.1；首次构建需要访问 crates.io 和 Native Shell Git 上游。

在此 worktree 中执行：

```powershell
$env:CARGO_PROFILE_DEV_DEBUG = '0'
$env:CARGO_PROFILE_TEST_DEBUG = '0'
$env:CARGO_INCREMENTAL = '0'
node scripts/verify.mjs -- cargo build --locked
if ($LASTEXITCODE -eq 0) { .\target\debug\githux.exe }
```

默认目标就是 `githux.exe`；无需启用迁移 feature 或另一个 package。输出在当前 worktree 的 `target/`，不要与其他任务共享可写 target。也可以用 `node scripts/verify.mjs -- cargo run --locked` 启动；该命令会持有共享构建锁直到应用退出，验收建议使用 build + 直接运行。

本分支不再通过旧 Windows App SDK runner 下载或准备 Ely 标题栏运行时。Windows 构建使用原生 NSH；macOS/Linux 当前只验证可移植库和测试，不发布未实现的平台应用。

## 页面与导航

Classic PAT 凭据保存在系统凭据库。未登录时显示账户入口；切换账号会退役旧页面并恢复该账号的进程内标签。服务授权、异步旧响应和缓存读写的边界继续由既有服务控制。

搜索按钮或 Ctrl+K 打开本地页面/仓库搜索；侧栏提供 Home、Settings 和仓库入口，底部头像进入 Profile。标签支持点击切换、关闭、Ctrl+W 和 Ctrl+Tab / Shift+Ctrl+Tab。刷新按钮调用当前页面刷新。搜索中可以进入 Notifications、Issues 和 Pull requests。

Home 只显示 open Issues/PRs；Feed 只显示关注账号合并 PR、星标仓库、创建仓库，继续加载 API 可用历史，卡片显示仓库简介与 PR 摘要。Read more 打开 Githux 内部 PR 标签，详情内容仍按既有范围留空。Profile 的可选 README 缺失时不占位。Repository 提供分支、文件树、正文/源码和图片。Notifications 的操作与缓存行为沿用已确认服务约定。

NSH headless 测试不等于 Windows 视觉验收。原生滚动/选区、IME、多 DPI、长文性能以及旧标签拖拽和焦点行为的完整对等性仍需验收；本分支不把这些写成已完成。

## 检查

所有 Cargo 操作使用共享 runner，记录真实 exit code 和完整日志：

```sh
node scripts/verify.mjs -- cargo fmt --all -- --check
node scripts/verify.mjs -- cargo check --locked --all-targets
node scripts/verify.mjs -- cargo clippy --locked --all-targets -- -D warnings
node scripts/verify.mjs -- cargo test --locked -- --test-threads=4
node scripts/verify.mjs -- cargo build --locked
git diff --check
```

记录在被忽略的 `.agents/task/runs/`。页面测试使用合成凭据和 mock transport。独立组件示例只用于开发，不进入正式启动路径或发行包。

贡献与文档分工见 [AGENTS.md](AGENTS.md)、[文档索引](docs/README.md)、[代码约定](docs/code-standards.md)、[工作流](docs/agent-workflow.md) 与 [LESSONS](LESSONS.md)。
