# Githux

基于 Rust、GPUI 与 Ely 的 GitHub 桌面客户端，Windows 优先，后续覆盖 macOS 和 Linux。当前正式工程实现 R1 的外壳布局：Windows 自定义标题栏、固定侧边栏与有界仓库列表、Ely island、Home 双列假数据、本地导航搜索浮层、应用级基础标签（打开/复用/切换/关闭），以及随 binary 注册的字体。PAT 登录、凭据保存与可取消的 GitHub 读取等服务层已实现（PR #14），但尚未接入 UI；仓库阅读器、Profile Overview、设置选项等业务页面仍未实现。

首版功能取舍见 [产品约定](docs/product.md#首版范围)，后续开发顺序与验收讨论见 [开发路线图](docs/roadmap.md)。首版不做 macOS/Linux 产品适配或安装更新系统；已有跨平台 Debug CI 保留。

## Windows 安装与运行

1. 安装 [Rust/rustup](https://rustup.rs/) 与 Visual Studio 2022 Build Tools 的 **Desktop development with C++**（MSVC x64、Windows SDK）。
2. 在本仓库运行 `rustup toolchain install 1.98.1 --profile minimal --component rustfmt --component clippy`。`rust-toolchain.toml` 固定版本，不需改变全局默认。
3. 运行：

```powershell
cargo run --locked
```

首次构建需访问 crates.io 与 Ely Git 上游，且编译体积较大。默认输出在本 worktree 的 `target/`；不要与其他项目/原型共享可写 target。Windows debug binary 为 `target/debug/githux.exe`。

`cargo run` 会自动把固定版本、经 SHA256 校验的 app-local Windows App SDK 运行时放到实际被启动的 exe 旁边：首次缺缓存时下载，之后复用本地缓存并离线运行；`--release`、显式 `--target`、自定义 `--target-dir` 以及新 worktree、`cargo clean` 后都无需手动准备。`cargo test`/`cargo bench` 等其他 Cargo 运行不经此准备。这是开发启动准备，不是系统安装，但也**不等于没有运行时依赖**：`cargo build` 不准备运行时，手动双击裸 exe 或分发给他人时仍须执行 `./scripts/prepare-windows-runtime.ps1 -Destination <exe 所在目录>`，并保留同目录 DLL、PRI、`githux-wasdk.manifest` 与 SDK 许可证，不能只复制 exe。CI 的 Windows artifact 已在打包步骤中包含这些文件。未准备运行时会回退到标准高度的 DWM 按钮。细节、缓存位置与 `--manifest-path` 等边界见 [Windows 运行时](docs/windows-runtime.md)。

Windows 正常应用的 debug / release 构建均使用 GUI 子系统，直接启动 exe 不分配控制台窗口；`cargo run` 等 Cargo 命令本身仍在调用者终端运行。测试 harness 保留控制台输出。正常 Windows 应用不再依赖 stdout/stderr 作为可见日志，持久诊断系统尚未实现。

启动进入 Home：左上角 Githux 位于自定义标题栏，从空白处可拖动窗口、双击最大化，右侧按钮依次为最小化、最大化/还原、关闭。右上角搜索入口或 `Ctrl+K` 打开 command palette，结果只有本地导航命令（各页面与当前假数据仓库）：上下移动、Enter 打开、Esc 关闭。侧边栏顶部 Home、Settings，中间仓库列表只有自身滚动，底部头像进入 Profile，其下四个等宽入口为 Notifications、Issues、PRs、New；New 尚未确定创建范围，因此保持禁用。

island 顶部是应用级标签条：点击侧栏仓库/Profile、底部入口或 palette 结果会打开或激活对应标签（同一资源复用、不重复），`Ctrl+W` 关闭当前标签，`Ctrl+Tab` / `Shift+Ctrl+Tab` 切换；关闭活动标签切到左邻，关闭最后一个标签后新建 Home。Home、Settings 与全局占位页按路由单例，仓库按 `owner/repo` 单例（大小写不敏感）。每个标签保留自己的页面滚动位置、焦点与仓库会话选择。标签数量不设上限：放不下时标签条自行横向滚动（滚轮 / `Shift`+滚轮 / 原生水平滚动 / 鼠标悬停时淡入的细滚动条），只影响标签条本身；新建、切换或 Tab 聚焦会把目标标签滚入视野。标签可以横向拖拽排序（按标签身份移动，不重建会话），鼠标中键在标签任意位置关闭该标签（不先激活），关闭按钮只在 hover 或活动标签上出现。标签条最右侧有一个固定的工具栏，本轮只放一个未接逻辑的禁用刷新入口。标签文本可省略，悬停显示完整名称。

页面内容：Home 用少量固定假数据展示左栏 Home（最近参与）与右栏 Feed（关注动态）的密度；Repository 与 Profile 只显示最小资源标识以核验路由；Settings 与 Issue/PR 等未设计页面内容区按约定留空，不添加“未实现”卡片。关闭最后窗口退出应用。仓库阅读器与页面的真实数据接入、账户切换时的标签处理仍属于后续任务（账户/PAT 服务层已实现，UI 未接入）。字体、中文回退、输入法、多 DPI 与窗口按钮的真实窗口验收状态见 [外壳布局开发日志](docs/devlog/2026-10-22-shell-layout.md)，应用级标签的实现与边界见 [标签开发日志](docs/devlog/2026-10-23-workspace-tabs.md)；macOS/Linux 仍保留系统标题栏，未做产品适配。

## 构建检查

```sh
cargo fmt --all -- --check
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked
cargo test --locked
git diff --check

# Windows 运行时准备与 Cargo runner 的离线回归测试（Windows）
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/tests/windows-runtime.test.ps1
```

[CI 配置](.github/workflows/ci.yml) 按改动范围运行：应用或构建配置改动触发 Ubuntu Rust 检查（fmt、check、clippy、test）及四平台 Debug 构建；纯文档跳过两者，Agent 脚本变更单独运行 Node 测试。Build 在 Windows x64、Ubuntu x64、macOS Apple Silicon / Intel 上原生构建，并上传 `githux-<Rust target>-debug` artifact，包含可执行文件和内置字体许可证；Linux/macOS 使用 `.tar.gz` 保留执行权限。

末尾的 `CI` job 汇总所有上游结果，可在分支规则中将名称为 `CI` 的检查设为 required check。范围判断成功、所需任务均成功、其余任务按规则跳过时通过；失败或意外跳过不能放行。范围及门槛见 [代码约定](docs/code-standards.md#ci-改动范围)。各提交的实际结果见 GitHub Actions，验证边界见 [CI 开发日志](docs/devlog/2026-10-21-ci-matrix.md)。

`cargo test` 有两部分证据。外壳部分使用 GPUI 测试平台渲染实际视图，覆盖启动 Home、缩放时只有仓库组让出高度、侧栏独立滚动、各入口路由与选中态、应用级标签的打开/复用/切换/关闭/上限与每标签滚动、焦点和仓库会话状态隔离、`Ctrl+W`/`Ctrl+Tab` 快捷键、搜索浮层的打开/过滤/高亮移动/Enter/Esc/组合态 Enter、焦点返回、中文与超长名称保持在列内，以及未设计页面的留空行为；测试平台不经过 Windows 原生输入分发，也不驱动真实的非客户区命中测试，因此不能替代真实窗口验收，本机窗口验收见 [外壳日志](docs/devlog/2026-10-22-shell-layout.md)，应用级标签的边界见 [标签开发日志](docs/devlog/2026-10-23-workspace-tabs.md)。

服务层部分覆盖 PAT 身份验证、凭据存取、授权代次、`/user` 与 `/user/repos` 分页/部分结果、限流与超时/取消分类、后台执行器收敛。其中若干用例通过仓库内的最小 localhost HTTP/1.1 mock 服务驱动真实 `reqwest` 客户端并检查实际请求头；不访问 GitHub、不使用真实凭据，唯一真实系统集成是自清理的 Windows Credential Manager 测试项。范围、命令与未覆盖部分（真实 GitHub 联调、HTTPS、重启恢复、UI 接入）见 [认证与网络](docs/auth-and-network.md#9-验证证据)。

## 依赖与字体

- Rust edition 2024，工具链 1.98.1；单 Cargo package 与 binary，`Cargo.lock` 纳入版本控制。
- `gpui-pre = 0.3.10`、`gpui-pre-platform = 0.3.10`，启用 `font-kit`；Linux 额外启用平台的 `x11` 与 `wayland` 窗口后端。
- [Ely 上游](https://github.com/ELY-Open-Source/Ely-GPUI-Components) 固定 revision `30c66fe1d5d23db769a6c834b771d054d7f288cf`；当前外壳直接依赖 `ely-core 0.2.0`，使用 `ely_core::*`，不经过汇总所有组件的 facade，也不复制上游源码或应用本地补丁。关闭默认 `fonts`（保留上游必需的 IBM Plex Sans SVG 字体），启用 `runtime_shaders` 保持 macOS 构建方式。迁移范围、上游接口变化与保留的适配见 [迁移记录](docs/devlog/2026-10-27-ely-migration.md)。搜索浮层没有复用 Ely 的 `CommandPalette`（它在任何 Enter 上运行高亮行），改用自绘卡片并在按键绑定前拦截 Enter、检查查询框的 marked 区间；未来引入只读编辑器时仍先重现相关问题再决定补丁。
- UI 默认 Noto Sans，代码默认 Google Sans Code。固定官方字体文件与许可证见 [字体来源](assets/fonts/README.md)，编译进 binary。中文回退、复杂排版和 IME 尚未验收。
- 网络与服务层：`reqwest = 0.13.5`（`default-features = false`，启用 `native-tls`、`http2`、`charset`、`system-proxy`）与 `tokio`（`rt`/`sync`/`time`/`macros`/`net`）；另有 `serde`/`serde_json`/`thiserror`。选型理由、取消与关闭模型见 [认证与网络](docs/auth-and-network.md#4-网络与执行模型)。

## 源码目录

| 路径 | 边界 |
| --- | --- |
| `src/main.rs` | binary 入口 |
| `src/app/` | 应用启动、依赖组合和框架初始化 |
| `src/ui/` | Ely 组件与视图局部状态：外壳、标题栏、侧边栏、页面、应用级标签条、搜索浮层、几何参数与布局 fixture |
| `src/platform/` | 字体注册；未来系统凭据、IME 与 OS 集成 |
| `src/workspace/` | 路由、会话集合与标签身份；每个会话的滚动、焦点与仓库会话状态 |
| `src/domain/` | 预留不依赖 UI 框架的业务类型与规则 |
| `src/auth/` | 预留账户与授权上下文 |
| `src/data/` | 预留业务入口、缓存策略、请求合并与通知 |
| `src/github/` | 预留 GitHub API 适配与字段转换 |
| `src/network/` | 预留传输、超时、代理、限流与调度 |
| `src/storage/` | 预留设置、账户引用、缓存与迁移 |
| `src/content/` | 预留 Markdown、源码与图片预处理 |
| `src/diagnostics/` | 预留脱敏诊断与性能证据 |

预留模块仅有职责说明，没有伪服务、模型或 trait 框架。架构约束与尚未确定的技术选择见 [架构文档](docs/architecture.md)，协作入口见 [AGENTS.md](AGENTS.md)。
