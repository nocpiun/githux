# 正式 GPUI + Ely 工程启动

- 日期：2026-10-21；任务 ID：bootstrap-gpui-ely；执行者：基层 Agent。
- 状态：原实现已提交并创建 [PR #1](https://github.com/nocpiun/githux/pull/1)；负责人已确认键盘修复的真实窗口复测全部正常，Windows GUI 子系统修复及本地检查也已完成。负责人现已允许提交并继续交付，修复将统一进入原 PR；新提交的云端 CI 与审查仍待验证。
- 当前验证对象：基于 `b1f995683c1be5a1aed55db35c2fef81c312e0e0` 的未提交键盘修复与 Windows GUI 子系统修复工作区。下方“初始构建验证”记录保留首轮基于 `d2ea765` 的本地结果。

## 改动与决定

建立 edition 2024 的单 Rust Cargo package 与真实 binary，固定 Rust 1.98.1（rustfmt/clippy），不更改全局默认。GPUI / platform 精确 0.3.8，platform 启用 font-kit；Ely 使用官方 Git 上游固定 revision `da3fdcaf39f2fa85ce1dd9cad07c3c81cf1dd721`，锁文件纳入交付文件，不使用原型 path、vendor 副本或共享可写 target。

启动模块初始化 Ely 深色主题、打开 Githux 窗口；Hello 视图组合 Ely Button 与根 FocusScope，点击时增加可见计数并通知重绘。最后窗口关闭时退出应用。本轮临时使用系统标题栏，正式自定义标题栏、island 与标签留给下一轮。

固定官方 Noto Sans 2.015 Regular 与 Google Sans Code 7.001 upright variable 字体，编译嵌入并注册；标题/按钮默认 Noto Sans、计数用 Google Sans Code。许可证、下载地址及 SHA-256 见 [字体来源](../../assets/fonts/README.md)。没有下载或打包全语言字体集；字体家族名称/版本已从字体 name table 核对。中文回退、复杂排版与 IME 尚未验收。

源码边界与上游补丁策略见 [架构文档](../architecture.md)。预留模块只含职责说明，没有登录/网络/数据库实现，也没有提前引入服务 trait 框架。只读参考指定原型文件，未修改原型或主目录。Windows CI 配置已添加；本记录中的本地结果不替代 GitHub CI 结果。

## 初始构建验证

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

## 键盘反馈与本地候选修复

负责人实际看到深色界面，鼠标点击 Press 能增加计数，但 Tab、Enter 和 Space 无反应。原提交的云端审查无发现不代表真实 UI 验收通过；本轮没有操作 GitHub 或重触发审查。

核对固定 revision 的 Ely `Button::focus_handle`、`FocusScope`、按钮测试与 GPUI 0.3.8 的实际实现：Ely 负责在焦点作用域内绑定 Tab / Shift+Tab；Button 无显式句柄时走内部 `tab_index(0)` 路径，显式句柄则走 `track_focus`；后者必须由句柄启用 `tab_stop(true)`。GPUI 只为已聚焦元素处理 Enter / Space 的按下和释放，不能用全局快捷键替代焦点。

候选修复在 `Hello` 中持有独立的 `press_focus`，创建为 `cx.focus_handle().tab_stop(true)`，通过 Button 的公开 `focus_handle` API 传入。根 FocusScope 与初始根焦点保留，鼠标处理、Ely 画风及计数回调不变，没有新增全局按键绑定，也没有升级或修改依赖副本。

**根因尚未完全确认**：原实现缺少应用持有的显式按钮焦点，但上游内部自动焦点路径按 API 本应可用；不能仅凭这处差异认定它是 Windows 实际故障的唯一根因。新增的测试直接渲染实际 Hello 视图，修复前的集成方式也通过了同一测试，因此测试平台未复现原生窗口缺陷。显式焦点方案目前是供人工复测的最小候选修复；若仍失败，需针对真实窗口的输入接收与焦点分发继续定位。

`src/ui/tests.rs` 使用 Ely 的测试初始化与 GPUI 测试平台，检查根焦点下 Enter / Space 不增加计数、Tab 离开根并进入按钮、Enter / Space 各增加计数、重绘后保持焦点、从根 Shift+Tab 进入按钮，以及单按钮正反导航循环。该测试不扩展公共 API，不经过 Windows 原生输入管道，不验证焦点环像素或鼠标输入。测试所需 `test-support` 仅通过 dev-dependency 启用，Cargo.lock 新增 9 个测试传递依赖，没有升级现有包或改变 GPUI/Ely 固定版本。

Windows 本地执行并通过（均退出 0）：`cargo fmt --all -- --check`、`cargo check --locked --all-targets`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo build --locked`、`cargo test --locked`（1 test）及 `git diff --check`。最终日志位于本地 `.agents/task/keyboard-*.log`，原集成方式对照日志为 `keyboard-test-before.log`。

开始与结束均核对进程，没有本 worktree 的 `githux.exe` 占用；本轮未关闭其他程序，也未启动窗口。所有改动未 stage / commit / push；本地交接为 `.agents/task/keyboard-fix-result.md`，替代上一份交接中的过期提交状态。

## 负责人真实窗口复测

2026-10-21，主 Agent 启动本 worktree 的新 binary（SHA-256：`be2a58329c758cbf83f98e71ce7ceadcf72d2acddef1f476661ea851b90735e9`）。负责人在实际 Windows 窗口完成复测并回复“全部正常”：激活窗口后 Tab 可使 Press 出现焦点边框，随后 Enter / Space 各增加一次计数，Shift+Tab 后仍可激活。该结果是本机真实交互验收证据，与上节测试平台的结果分开记录。

本机复测表明这份修复已满足当前键盘验收；由于原实现同样通过测试平台用例，仍不将内部默认焦点路径失效的具体原因写成已证实结论。未额外验证跨平台、多 DPI、IME 或所有窗口激活路径。当时负责人“先不提交”的指令仍有效，该轮改动继续保留在本地；后续已明确允许提交并继续交付。

## Windows 启动控制台修复

负责人反馈直接启动应用会先弹出终端窗口，再显示主窗口。主 Agent 读取旧 debug executable 的 PE Optional Header，确认 Subsystem=3（Windows CUI）；入口没有 `windows_subsystem` 属性，这是本轮修复的直接原因。

在 `src/main.rs` 添加 `#![cfg_attr(all(target_os = "windows", not(test)), windows_subsystem = "windows")]`。该属性对 Windows 正常应用的 debug / release 均生效，不受 `debug_assertions` 限制；测试 harness 不应用此属性，其他平台不受影响。不增加依赖，不使用启动后隐藏控制台或外部脚本。

直接启动 Windows exe 不再分配控制台；`cargo run` 等命令仍在调用者终端中运行。正常 Windows 应用不再依赖 stdout/stderr 作为可见日志，持久诊断系统尚未实现，本轮不扩展日志功能。

本地 fmt、`cargo check --locked --all-targets`、Clippy（`-D warnings`）、`cargo build --locked`、`cargo test --locked`（1 test）与 `git diff --check` 全部退出 0。直接读取 PE32+ Optional Header：新 debug 应用 Subsystem=2（Windows GUI），测试 executable Subsystem=3（Windows CUI），测试输出正常。新应用 SHA-256 为 `a801d7647ebd7339222e9cdc81abf431f5f5904162fb9db93e09cce003b01447`；完整 PE 证据与日志见本地交接 `.agents/task/console-fix-result.md`。本轮未做 release 冷构建，release 适用性来自属性未限定构建 profile；实现者未启动应用，未新增单属性镜像测试。主 Agent 随后直接启动新版 exe，确认标题为 Githux 的主窗口已创建且保持响应，未使用隐藏窗口参数；此为启动证据，不是新增人工交互验收。上节负责人键盘实测记录保留，不能代替本轮启动体验复测。全部修复继续未 stage / commit / push，未操作 GitHub。

## 后续

负责人已明确允许提交并继续。将键盘与 Windows GUI 子系统修复统一提交至原 PR，推送后显式请求新的 Codex review，并核验新 head 的 CI 与审查；原提交的通过结果不能覆盖新修复。提交前已核对本地 fmt/check/clippy/build/test（1 passed）日志和 PE 证据，全部通过，交付阶段只更新文档，不重建正在使用的 binary。最终 SHA、触发评论、云端检查和合并结果保存在本地交接及 PR，避免为补录结果另造提交。

初始交付时主 Agent 已从非工程根目录启动 debug binary，确认进程建立标题为 Githux 的窗口、保持响应且无启动错误；该进程/句柄证据不等同于交互通过，键盘反馈与本轮限制见上节。macOS/Linux、多 DPI、跨屏、中文/Emoji 回退、复杂排版、IME、正式外壳与业务能力仍未覆盖。最终交付仍需最新提交 CI/review 与真实验收门槛满足，PR 政策没有改变。
