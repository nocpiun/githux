# Githux

基于 Rust、GPUI 与 Ely 的 GitHub 桌面客户端，Windows 优先，后续覆盖 macOS 和 Linux。当前正式工程只实现 **Hello, world!**：Ely 深色主题、真实 Button 和 FocusScope、可见点击计数及随 binary 注册的字体。登录、网络、数据库与业务页面尚未实现。

首版功能取舍见 [产品约定](docs/product.md#首版范围)，后续开发顺序与验收讨论见 [开发路线图](docs/roadmap.md)。首版不做 macOS/Linux 产品适配或安装更新系统；已有跨平台 Debug CI 保留。

## Windows 安装与运行

1. 安装 [Rust/rustup](https://rustup.rs/) 与 Visual Studio 2022 Build Tools 的 **Desktop development with C++**（MSVC x64、Windows SDK）。
2. 在本仓库运行 `rustup toolchain install 1.98.1 --profile minimal --component rustfmt --component clippy`。`rust-toolchain.toml` 固定版本，不需改变全局默认。
3. 运行：

```sh
cargo run --locked
```

首次构建需访问 crates.io 与 Ely Git 上游，且编译体积较大。默认输出在本 worktree 的 `target/`；不要与其他项目/原型共享可写 target。Windows debug binary 为 `target/debug/githux.exe`。

Windows 正常应用的 debug / release 构建均使用 GUI 子系统，直接启动 exe 不分配控制台窗口；`cargo run` 等 Cargo 命令本身仍在调用者终端运行。测试 harness 保留控制台输出。正常 Windows 应用不再依赖 stdout/stderr 作为可见日志，持久诊断系统尚未实现。

窗口标题为 Githux，正文为 Hello, world!。点击 Press 后计数递增；用 Tab / Shift+Tab 切换焦点，再按 Enter / Space，检查焦点环与计数重绘。关闭最后窗口退出应用。本轮使用临时系统标题栏；自定义标题栏、island 与应用级多标签留给下一轮外壳实现。

## 构建检查

```sh
cargo fmt --all -- --check
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked
cargo test --locked
git diff --check
```

[CI 配置](.github/workflows/ci.yml) 将 Windows 代码检查（fmt、check、clippy、test）与四个平台的 Debug 构建并行执行。Build 在 Windows x64、Ubuntu x64、macOS Apple Silicon / Intel 上原生构建，并上传 `githux-<Rust target>-debug` artifact，包含可执行文件和内置字体许可证；Linux/macOS 使用 `.tar.gz` 保留执行权限。

末尾的 `CI` job 汇总所有上游结果，可在分支规则中将名称为 `CI` 的检查设为 required check。只有代码检查和整个构建矩阵均成功时才通过；维护约定见 [代码约定](docs/code-standards.md#验证)。各提交的实际结果见 GitHub Actions，验证边界见 [CI 开发日志](docs/devlog/2026-10-21-ci-matrix.md)。

`cargo test` 使用 GPUI 测试平台渲染实际 Hello 视图，覆盖 Tab / Shift+Tab、焦点保留与 Enter / Space 激活。测试平台不经过 Windows 原生输入分发，原实现也能通过该测试，不能据此认定用户反馈的键盘问题已经修复。真实窗口的渲染、字体与键盘操作仍需人工验收；本机键盘复测已由负责人确认通过，具体场景与限制见 [开发日志](docs/devlog/2026-10-21-bootstrap-gpui-ely.md)。macOS/Linux 原生运行与真实交互尚未进行人工验收。

## 依赖与字体

- Rust edition 2024，工具链 1.98.1；单 Cargo package 与 binary，`Cargo.lock` 纳入版本控制。
- `gpui-pre = 0.3.8`、`gpui-pre-platform = 0.3.8`，启用 `font-kit`；Linux 额外启用平台的 `x11` 与 `wayland` 窗口后端。
- [Ely 上游](https://git.zacharyzhang.com/ZacharyZhang-NY/Ely-GPUI-Components.git) 固定 revision `da3fdcaf39f2fa85ce1dd9cad07c3c81cf1dd721`，未应用原型的 IME/readonly 补丁。未来引入输入/只读编辑器时先重现相关问题，再评估上游修复或有针对性的补丁；Hello 不需要这些补丁。
- UI 默认 Noto Sans，代码默认 Google Sans Code。固定官方字体文件与许可证见 [字体来源](assets/fonts/README.md)，编译进 binary。中文回退、复杂排版和 IME 尚未验收。

## 源码目录

| 路径 | 边界 |
| --- | --- |
| `src/main.rs` | binary 入口 |
| `src/app/` | 应用启动、依赖组合和框架初始化 |
| `src/ui/` | Ely 组件与视图局部状态；当前只有 Hello |
| `src/platform/` | 字体注册；未来系统凭据、IME 与 OS 集成 |
| `src/workspace/` | 预留窗口、路由、标签、页面会话与焦点归属 |
| `src/domain/` | 预留不依赖 UI 框架的业务类型与规则 |
| `src/auth/` | 预留账户与授权上下文 |
| `src/data/` | 预留业务入口、缓存策略、请求合并与通知 |
| `src/github/` | 预留 GitHub API 适配与字段转换 |
| `src/network/` | 预留传输、超时、代理、限流与调度 |
| `src/storage/` | 预留设置、账户引用、缓存与迁移 |
| `src/content/` | 预留 Markdown、源码与图片预处理 |
| `src/diagnostics/` | 预留脱敏诊断与性能证据 |

预留模块仅有职责说明，没有伪服务、模型或 trait 框架。架构约束与尚未确定的技术选择见 [架构文档](docs/architecture.md)，协作入口见 [AGENTS.md](AGENTS.md)。
