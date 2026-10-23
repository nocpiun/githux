# 开发启动自动准备 Windows 运行时

2026-10-23，任务 `auto-windows-runtime`，分支 `fix/auto-windows-runtime`。状态：实现完成，待审查。

## 背景

负责人反馈：在最新分支直接 `cargo run` 后，Windows 原生标题栏按钮退回 Standard 高度。原因是 `D:/githux` 的 `target/debug` 缺少 app-local Windows App SDK，上一份独立 worktree 有运行时。手动补齐可以恢复 Tall，但每次新 worktree、`cargo clean`、换 `--target-dir` 都要重做。要求把这一步自动化，而不是让用户在初始化项目时另装 SDK。

## 实现

- 新增 `.cargo/config.toml`，只对 `cfg(windows)` 配置 Cargo target runner `./scripts/run-githux.cmd`；`cargo build`/`check`/`clippy` 保持普通构建。
- 新增 `scripts/run-githux.ps1`：识别被启动的是不是 githux 应用、从 PE 头读取架构、调用共用准备逻辑、以继承 stdio 的子进程启动并按原样返回退出码。
- 新增 `scripts/windows-runtime-lib.ps1`：固定包清单、SHA256 校验、原子落盘、逐文件哈希比对、被占用时报错而非覆写的共用准备逻辑。
- `scripts/prepare-windows-runtime.ps1` 改为该逻辑的 CLI 入口，保留 CI 打包使用的 `-Destination`/`-Architecture` 语义。
- 新增 `scripts/tests/windows-runtime.test.ps1` 离线回归测试。
- 新增 `docs/windows-runtime.md`，README 的 Windows 启动说明改为只推荐 `cargo run --locked`。

## 关键取舍

**为什么用 runner 而不是 build.rs**：build.rs 里下载并写 `OUT_DIR` 父目录要靠 `PROFILE` 字符串猜实际 exe 位置，且对 `--target`、自定义 `--target-dir`、`cargo test` 都会误伤。runner 拿到 Cargo 实际要执行的 exe 路径，天然正确；代价是只覆盖 `cargo run`，`cargo build` 后手动双击裸 exe 仍需打包步骤。

**参数往返**：`.cmd` 桥用 `%*` 把参数交给 PowerShell，但 PowerShell 的 `$args` 会被重新解析，`"`、`%`、`^`、`&`、`!` 会变形。实测确认 `[Environment]::GetCommandLineArgs()` 能拿到未受损的原始 argv，因此 runner 用它在子进程中重建命令行。`ProcessStartInfo.FileName`+`Arguments` 已经接受原始命令行文本，不需要 `CreateProcessW` P/Invoke。真实往返覆盖空格、引号、`&`、`%PATH%`、`!x!`、`^`、反斜杠、Unicode 与空参数。

**缓存与幂等**：旧脚本每次解压并覆盖全部 DLL。现在按包内实际文件清单 + SHA256 判断是否需要写入，只替换内容不同的文件；下载先落临时文件、校验后原子改名。重复运行不联网、不改 mtime，缓存损坏可自我修复。

**来源与哈希不变**：固定版本、官方 NuGet 源与 SHA256 保持原值；`-SourceBase`/`-PackageManifest` 只供离线测试与镜像，不能在默认路径上放宽校验。

## 验证

- `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/tests/windows-runtime.test.ps1`：20 项通过。含固定版本/哈希回归、PE 架构解析、半包/损坏/被占用/重复运行/缓存损坏、以及独立 Cargo fixture 上的真实 Windows 进程链（root 与子目录调用、参数往返、退出码、`--release`、显式 `--target`、带空格与 Unicode 的自定义 `--target-dir`、`cargo test` 透传且不准备 GUI 运行时、准备失败不启动应用）。
- 本任务 worktree 真实 SDK：删除 `target/windows-appsdk` 与 `target/debug` 运行时后 `cargo run --locked` 自动下载并写入 27 个文件；再次运行 0 写入、运行时文件 mtime 不变；删除个别文件后可恢复；被占用时报错退出码 1 且不覆写。manifest 引用的 11 个激活目标文件全部就位，两份许可证随附。
- 真实预览：`cargo run --locked` 启动 Githux 窗口。UI Automation 读取 `AppWindow Custom Title Bar` 216×72 物理像素、Minimize/Maximize/Close 各 72×72（DPI 144 → 48 逻辑像素，即 Tall），日志无 `Tall caption unavailable` 回退。同一份 exe 移走运行时后手动启动会打印回退信息，作为对照。
- 未覆盖：桌面截图在本机会话为黑屏，未取得像素级外观证据；因此这次只报告原生按钮度量与激活路径，不改 UI，不改写前次 Tall 视觉验收。未在别的 Windows 版本、其他 DPI、arm64/x86 实机验证。

## 后续

- 待 PR 审查与最新 head CI。
- 若未来要求 `cargo build` 后也能直接双击运行，需要在打包/分发步骤而非 runner 里解决。
- Cargo 在仓库外通过 `--manifest-path` 调用不会加载本仓库 `.cargo/config.toml`，已在 `docs/windows-runtime.md` 说明。
