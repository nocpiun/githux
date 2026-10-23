# Windows 运行时（开发启动）

更新：2026-10-23。本文说明 `cargo run` 如何自动准备 app-local Windows App SDK 运行时，以及哪些场景仍需手动处理。UI 与按钮外观不属于本文，见 [Tall 原生标题栏](devlog/2026-10-23-tall-caption.md)。

## 结论

在 Windows 上从仓库运行 `cargo run --locked` 即可。运行前，Cargo runner 会把固定版本、经 SHA256 校验的 Windows App SDK 运行时放实际被启动的 exe 旁边；首次缺缓存时需要联网下载，之后复用本地缓存并离线运行。

这是**开发启动准备**，不是系统安装：不注册系统包、不需要管理员权限、不修改系统或用户的 ExecutionPolicy、不创建常驻服务。运行时仍是随程序分发的普通 DLL/PRI 文件、激活清单与许可证，与既有打包方式一致。因此“无需系统安装”不等于“没有运行时依赖”：直接双击未经准备的裸 exe 仍会缺少这些文件。

## 自动做了什么

`.cargo/config.toml` 为 Windows 目标配置 runner `scripts/run-githux.cmd`。Cargo 只对 `cargo run` 使用 runner；`cargo build`、`cargo check`、`cargo clippy` 仍是普通构建，不会下载或运行任何东西。

runner 的行为：

1. 只在被启动的二进制是 githux 应用（`target/<profile>/githux.exe` 或 `target/<triple>/<profile>/githux.exe`）时准备运行时。`cargo test`、`cargo bench`、example 与其他 binary 位于 `target/<profile>/deps/` 或另有名字，按原样启动，不联网、不启动 GUI。
2. 从 exe 的 PE 头读取目标架构，据此选择 `win-x64`/`win-arm64`/`win-x86` 载荷，不使用 `PROFILE` 字符串猜路径。
3. 调用 `scripts/windows-runtime-lib.ps1` 的共用准备逻辑（`scripts/prepare-windows-runtime.ps1` 是同一逻辑的 CLI 入口）。
4. 以继承 stdio 的子进程启动 exe，并原样返回其退出码。

缓存位于同一 target 目录下：默认构建为 `target/windows-appsdk/`，`--target <triple>` 为 `target/<triple>/windows-appsdk/`，自定义 `--target-dir` 则在其下。每个 worktree、每个 target 目录各自独立，不共享可写 Cargo target。

## 缓存与幂等

- NuGet 包按固定版本与 SHA256 校验；下载先写入临时文件，校验通过后才原子改名成 `package.zip`，半包不会成为有效缓存。
- 每个文件在拷贝前比对预期 SHA256；只有内容不同才通过临时文件替换，所以重复运行不重写、不改 mtime，也不重复解压 100 MB 级的包。
- 缺失或损坏的文件会从已验证的包恢复；缓存包本身损坏会被丢弃并重新下载。
- 存在性来自包内 `runtimes-framework/win-<arch>/native/` 的实际文件清单与哈希，不依赖任何 marker 文件。
- 目标文件正被其他进程（例如正在运行的 Githux）占用时，不会原地覆写，而是报错并说明关闭后重试。

## 失败与诊断

准备失败时 runner 在 Cargo 终端给出简短原因与非零退出码，并且**不启动应用**，因此不会静默落回 Standard 按钮。常见原因：首次下载需要网络、缓存包损坏、目标目录被占用。下载与子进程错误保留原始信息；不打印环境变量，也不涉及凭据。

已发布的 binary 在缺少运行时时仍沿用既有的外观回退（Tall 不可用则退回旧 DWM 路径），那属于运行时行为，与本准备流程无关。

## 边界

- **仓库外 `--manifest-path`**：Cargo 只从当前目录向上查找 `.cargo/config.toml`，在仓库外调用 `cargo run --manifest-path <repo>/Cargo.toml` 不会加载本仓库的 runner。请先 `cd` 到仓库（或其子目录）再运行，或先执行一次 `scripts/prepare-windows-runtime.ps1 -Destination <该 target 输出目录>`。
- **手动双击裸 exe**：`cargo build` 不准备运行时。需要直接运行产物或分发时，先执行 `scripts/prepare-windows-runtime.ps1 -Destination <exe 所在目录>`。CI 的 Windows artifact 已在打包步骤中带上运行时、清单与许可证。
- **非 Windows 平台**：runner 只在 `[target.'cfg(windows)']` 下配置，Linux/macOS 构建与 CI 矩阵不受影响。
- **仓库内子目录**：runner 的第一个元素由 Cargo 相对 `.cargo/config.toml` 解析，因此在仓库根或任意子目录调用都正确，子进程保持调用者的工作目录。

## 手动用法

```powershell
# 准备默认 debug 输出目录
./scripts/prepare-windows-runtime.ps1

# 为自定义输出目录或架构准备
./scripts/prepare-windows-runtime.ps1 -Destination "dist/githux-win-x64" -Architecture x64

# 修复损坏的缓存或输出目录
./scripts/prepare-windows-runtime.ps1 -Destination target/debug
```

`-CacheRoot`、`-SourceBase`、`-PackageManifest` 供离线测试或内网镜像使用，默认值始终是固定的官方来源与固定哈希。

## 验证边界

`scripts/tests/windows-runtime.test.ps1` 用临时目录与本地包源离线验证准备逻辑（半包、损坏、占用、重复运行、缓存损坏），并用一个独立的小 Cargo fixture 在真实 Windows 进程链上验证 runner 的参数往返、工作目录、退出码、`--release`、显式 `--target`、自定义 `--target-dir`、测试 harness 透传与失败不启动。fixture 不能替代真实 githux 窗口外观验收。
