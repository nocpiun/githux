# 正式 GPUI + Ely 工程启动

- 日期：2026-10-21；任务 ID：bootstrap-gpui-ely；执行者：基层 Agent。
- 状态：本地实现、Windows 构建检查和进程/窗口创建验证完成；最终 GUI 交互、CI 与 review 结果以 PR 及交接记录为准。
- 验证对象：`feature/bootstrap-gpui-ely` 的未提交工作区，基线 `d2ea765`。

## 改动与决定

建立 edition 2024 的单 Rust Cargo package 与真实 binary，固定 Rust 1.98.1（rustfmt/clippy），不更改全局默认。GPUI / platform 精确 0.3.8，platform 启用 font-kit；Ely 使用官方 Git 上游固定 revision `da3fdcaf39f2fa85ce1dd9cad07c3c81cf1dd721`，锁文件纳入交付文件，不使用原型 path、vendor 副本或共享可写 target。

启动模块初始化 Ely 深色主题、打开 Githux 窗口；Hello 视图组合 Ely Button 与根 FocusScope，点击时增加可见计数并通知重绘。最后窗口关闭时退出应用。本轮临时使用系统标题栏，正式自定义标题栏、island 与标签留给下一轮。

固定官方 Noto Sans 2.015 Regular 与 Google Sans Code 7.001 upright variable 字体，编译嵌入并注册；标题/按钮默认 Noto Sans、计数用 Google Sans Code。许可证、下载地址及 SHA-256 见 [字体来源](../../assets/fonts/README.md)。没有下载或打包全语言字体集；字体家族名称/版本已从字体 name table 核对。中文回退、复杂排版与 IME 尚未验收。

源码边界与上游补丁策略见 [架构文档](../architecture.md)。预留模块只含职责说明，没有登录/网络/数据库实现，也没有提前引入服务 trait 框架。只读参考指定原型文件，未修改原型或主目录。Windows CI 配置已添加；本记录中的本地结果不替代 GitHub CI 结果。

## 验证

Windows x86_64-pc-windows-msvc，实际 `rustc 1.98.1 (48a229cea 2026-09-01)`。以下命令全部退出 0：

```sh
cargo fmt --all -- --check
cargo check --locked --all-targets
cargo build --locked
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
git diff --check
```

`cargo test` 运行 0 项测试，表示测试配置构建成功，不是 GUI 交互覆盖。完整 debug binary 已生成于 `target/debug/githux.exe`；编译与工具链日志保存在本地忽略目录 `.agents/task/`。

精确工具链首次安装因 TLS 连接中断失败，重试成功。首次 check 发现窗口关闭回调需两个参数，已按 GPUI 实际 API 修正并重跑通过。没有更换依赖或关闭安全功能。

## 后续

主 Agent已从非工程根目录启动 debug binary，确认进程建立标题为 Githux 的窗口、保持响应，stdout/stderr无启动错误。桌面自动化接口重试及重新初始化后仍因本机连接管道不可用而无法截图或操作；已请负责人验证实际画面、点击计数及Tab/Enter/Space操作。该交互验收结果写入PR/交接，不把窗口句柄与进程响应等同于视觉或交互通过。

本轮无待裁决实现项。macOS/Linux、多 DPI、跨屏、中文/Emoji 回退、复杂排版、IME、正式外壳与业务能力均未覆盖。交付通过任务分支与PR推进，最终合并仍需最新提交CI/review与验收门槛满足。PR政策没有改变。
