# Native Shell 迁移：原生基础（foundation-ready）

- 日期：2026-11-09。任务 ID：native-shell（`migration/native-shell`），本地协作迁移。
- 状态：第一阶段完成——纯 Rust Windows 原生窗口、公共控件、字体/输入/滚动探针可运行；业务页面未迁移。

## 改动与决定

- 新增 workspace 成员 package `githux-nsh`（`native/nsh/Cargo.toml`），lib 在 `src/nsh/`，bin `githux-nsh`。根 package 的旧 GPUI 应用保留作对照，双栈并存不是最终状态。
- 固定 NSH v0.4.0（`e626a8dd8f8d8b277b9507cfd5ac74d0e67020f3`），关闭 Windows 平台默认 `js` feature；依赖树 632 个包无 QuickJS/GPUI/Ely。
- 移植字体资产（`foundation/fonts.rs`）、Ely 暗色 token（`foundation/theme.rs`）、公共控件（`foundation/controls.rs`）与虚拟列表/输入/拖动探针（`shell/mod.rs`）。
- 接口契约见 `.agents/task/integration-notes.md`；主题文档由 [native-shell-migration.md](../native-shell-migration.md) 统一维护。

## 验证

- `cargo build -p githux-nsh` → 成功。
- `cargo clippy -p githux-nsh --all-targets -- -D warnings` → 通过。
- `cargo fmt -p githux-nsh -- --check` → 通过。
- `cargo test -p githux-nsh --lib` → 2 passed（滚动后行渲染、无安装字体的 family 解析）。
- 根 package `cargo check` → 通过，基线未破坏。
- 真实窗口：`githux-nsh.exe` 进程 `MainWindowTitle=Githux`、`Responding=True`；PrintWindow 截图验证页面标题、行号、中英混排与交替背景。未做真实 IME/多 DPI/触控板。

## 后续

- 精确滚动条外观（3px / 0.20 / 0.38 / 140ms 双向可逆 fade）待视觉验收。
- runtime/reader 任务 ready 后再整合账户/会话与 Reader；当前未集成。
- 业务页面全部未迁移，等待主 Agent 按稳定契约派发页面任务。
