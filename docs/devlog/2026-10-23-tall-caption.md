# Tall 原生标题栏

2026-10-23。负责人要求在保留系统原生窗口按钮的前提下，将按钮底边贴齐 island；需先看真实预览，确认通过后才允许提交 PR。

## 实现

- 新 Windows 专属 AppWindow 适配关联 GPUI 已有 HWND，使用 Windows App SDK 1.8 的 ExtendsContentIntoTitleBar + PreferredHeightOption::Tall。
- SDK 返回标题栏 Height/RightInset，按当前窗口 DPI 转为 GPUI 逻辑坐标。内容行从 28px 改为 32px，上下仍各 8px；典型顶栏48px，island从该行下缘开始。
- 现有 DWM 适配保留为回退，不与 AppWindow 路径叠加。无需改写 Ely/GPUI 或修改依赖缓存源码。
- 固定元数据生成 Rust 绑定；正常构建不下载 SDK 或运行生成器。runtime 准备脚本按 SHA256 校验官方 NuGet 包，只写应用本地输出目录。Windows CI artifact 附带运行时、PRI、清单和许可证。

## 接入中实际遇到的问题

仅复制运行时 DLL 并加载 WindowsAppRuntime.dll 不能激活 DispatcherQueueController，报 REGDB_E_CLASSNOTREG。补全源自包内 package.appxfragment 的 WinRT 激活清单后解决。GPUI 默认已嵌入资源1清单，额外 /MANIFEST:EMBED 导致链接资源重复；改用应用本地独立激活上下文，保留 GPUI 既有清单。

成功启动日志确认在本机 DPI144：Height72、RightInset216，即48逻辑像素高、144逻辑像素的按钮保留区。此日志只证明 SDK 启用与度量，不替代真实绘制/点击验证。

## 验证状态

- 首轮真实预览，负责人反馈：高度、对齐、交互正常，背景不对。修正为通过 WinRT Color 显式传入 Ely 顶栏背景/前景，激活及失焦的按钮底色一致，保留系统 hover/pressed 效果；第二轮负责人确认“一切正常”，同时希望整个标题栏再稍微缩小。
- Computer Use native pipe重试后仍不可用，未声称自动化验证过真实绘制。
- 背景修正后，本地 Clippy -D warnings、32 项测试及 debug 构建通过；含 WinRT Color 装箱/接口转换验证。runtime 准备脚本在独立输出目录通过，清单重生成核对一致，actionlint 与 git diff --check 通过。已重建包含运行时清理的新预览，关闭退出仍待验证。负责人已批准最终预览，准备提交 PR。
- 跨屏/其他DPI与其他Windows版本尚未验证。


## 高度微调的边界

背景修正版已获负责人真实窗口验收，但负责人随后要求进一步降低整条顶栏。AppWindowTitleBar 的 Height 是只读值，PreferredHeightOption 只有 Standard/Tall（另有隐藏用的 Collapsed），没有任意像素高度设置；通常 Standard 为32、Tall为48逻辑像素。保留系统按钮并让其底边贴齐 island 时，不能仅把 GPUI 行改为40或44而留下系统48px按钮。负责人最终选择保持原生按钮高度不变，仅调大 Githux 标题字体、调小搜索框高度。当前保持48px顶栏与系统 Tall 按钮，品牌从 Ely Sm（默认12px）改为 Md（默认14px）；搜索框从32px改为28px，在既有32px内容行内居中，标题与搜索框中心线不变。该微调的 Clippy、32项测试、debug构建和 diff --check 均通过，负责人确认比例合适并授权提交 PR，进入 review 循环。

来源：[高度选项](https://learn.microsoft.com/en-us/windows/windows-app-sdk/api/winrt/microsoft.ui.windowing.titlebarheightoption?view=windows-app-sdk-1.8)、[只读高度](https://learn.microsoft.com/en-us/windows/windows-app-sdk/api/winrt/microsoft.ui.windowing.appwindowtitlebar.height?view=windows-app-sdk-1.8)。

最终验收：负责人确认保留系统按钮高度，采用 Githux 默认14px字号、搜索框28px高度；背景、悬停及点击、窗口化/最大化对齐通过。本机32项测试不代替跨平台真实窗口验证。

## PR 审查修复

首轮审查指出：完成 Tall 配置后，诊断日志中的 Height/RightInset 读取仍使用 `?`，读取失败会在未回滚 Tall 的情况下触发旧 DWM 适配。移除这段启动尺寸日志，配置成功后直接保存 Tall 状态并返回成功；必需的配置失败仍走已有 ResetToDefault/样式恢复路径。正常外观和交互不变，本地 fmt、Clippy 与32项测试通过。初次预览日志中的尺寸证据仍是历史实测记录。