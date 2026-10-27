# Ely 官方新仓库迁移

日期：2026-10-27。负责人要求主 Agent 亲自实施，保持未合并，等待负责人手动验收。

## 基线与交付范围

- Githux 基线：`84b029e63b9ca28dfc9d1abf7d8aed7795684c0e`，包含已合并的 workspace-tabs。
- 独立分支：`refactor/ely-migration`。没有拉入尚未合并的 sidebar-layout、content-reader、account-ui 或 shared-scroll。
- Ely 旧提交：`da3fdcaf39f2fa85ce1dd9cad07c3c81cf1dd721`，旧 Git 地址。
- Ely 新提交：[`30c66fe1d5d23db769a6c834b771d054d7f288cf`](https://github.com/ELY-Open-Source/Ely-GPUI-Components/commit/30c66fe1d5d23db769a6c834b771d054d7f288cf)，查询时官方 main 最新提交，时间 `2026-10-26T22:32:09Z`；比旧 pin 前进 149 个提交。固定 SHA，不跟随漂移的 main。
- 使用正常 Git 依赖，没有 vendor、Cargo patch、本地缓存修改或新 fork。其他未合并 worktree 没有被此迁移修改。

## 应用实际迁移

| 原实现 | 迁移结果 |
| --- | --- |
| 单体 `ely-gpui-component` 依赖和 `ely_gpui_component::*` | 直接使用官方 `ely-core 0.2.0` 和 `ely_core::*`，含启动、主题、外壳组件与测试初始化 |
| GPUI / platform `0.3.8` | 统一固定 `0.3.10`；Linux x11/wayland、font-kit 和 macOS runtime_shaders 保留；Rust 仍为 1.98.1 |
| 默认携带 Ely 的 Inter / JetBrains Mono | regular/dev 依赖均关闭默认 fonts；项目 Noto Sans / Google Sans Code 注册与主题设置保留；Ely 必需的 IBM Plex Sans SVG 字体仍由上游 Assets 提供 |
| GPUI `ScrollWheelEvent` | 改为 `ScrollEvent`，现有滚轮模拟显式指定 `ScrollSource::Pointer`，不混同触屏滚动 |
| `KeyBinding::new("secondary-k/w", …)` | 新静态解析器不再解析 secondary 别名，原调用会在启动时 panic；改为 `KeyBinding::load` 通过 app 的 OS 解析，保留 Windows/Linux Ctrl、macOS Cmd |
| 接受任意借用字符串的测试按键 helper | 跟随 `Keystroke::parse` 的静态字符串签名；现有调用全部是固定按键 |
| 总入口初始化所有垂直组件 | `ely_core::init` / `init_for_tests` 只初始化实际依赖的核心组件；保留 Githux 自有快捷键和焦点作用域 |

`Cargo.lock` 包记录从 **885 减少到 738**。这表示本次解析图的包数量，不能直接等同于二进制大小或启动速度；尚未做性能基准。上游图标改为按使用链接，项目继续使用 `IconName`，无需复制图标资源。

## 上游增加、增强和整理了什么

对照固定的新旧源代码和 [上游 changelog](https://github.com/ELY-Open-Source/Ely-GPUI-Components/blob/30c66fe1d5d23db769a6c834b771d054d7f288cf/CHANGELOG.md)，以下区分模块搬迁与功能增加：

| 范围 | 变化及对 Githux 的影响 |
| --- | --- |
| crate 拆分 | 31 个库 crate：核心 `ely-core`、各垂直领域 crate、`ely-desktop` 和兼容汇总入口 `ely-gpui-component`。聊天、图表、编辑器等大量组件是原有功能拆包，不能称为新增的 31 套组件。本应用现阶段只引入 core。 |
| 滚动 | 原有 `ScrollArea` / `Scrollbar` 增加 owner 持有的 handle 接入、显式 `presence`、活动显示；新增 `ScrollSource`、`ScrollModel`、`ScrollPlace`，可接普通 handle、虚拟列表和自定义模型；修复列表末端及拖动/hover 保持。 |
| Markdown | `MarkdownDocument` 内置滚动条及 source 行号；`MarkdownRenderer` 支持 owner 提供解析 blocks 和 pictures。解析实现改为 pulldown-cmark / 自有 mdast，inline math 和代码保护有修复。都在 `ely-documents`，不是本次主线已接入的页面。 |
| Tabs / 树 / 编辑器标签 | 现有 Tabs 支持宽条滚动、选中项显露、长标题截断；Tree、FileTree、EditorTabs 增强 `on_context`。这些是原组件的行为增强。 |
| 核心 API | 增加 `Sizable`、`Disableable`、`Selectable`、`Changeable` 能力 trait；容器统一 `ParentElement`；整理公开 API / rustdoc。既有组件链式方法仍可用，不需要为使用 trait 而增加无关封装。 |
| 桌面 | tray、Dock badge、Omarchy 迁至 `ely-desktop`；新增 Omarchy 主题读取/跟随能力；tray 扩展 Windows/Linux。Githux 当前没有使用这些功能。 |
| 输入/无障碍/本地化 | 表单、弹层和焦点节点补名称与角色；无显式初始焦点的 Dialog 会聚焦首控件；Tooltip 随语言切换撤下；PathInput 提供原始路径选择回调。六种语言 catalog 更完整，不能据此声称 Githux 已完成本地化。 |
| 构建与资源 | GPUI 升至 0.3.10；新提交声明最低 Rust 1.98；平台接入改为 feature；图标按使用链接，字体可选，移除 Ely 对 rust-embed / isolang 的依赖，macOS 平台代码转向 objc2。 |

没有发现本应用正在使用的 `Avatar`、`NavItem`、`TextInput`、`IconButton`、`Tooltip`、`FocusScope`、Theme 等组件被删除。上游把部分实现细节收回内部，例如 `editor::welcome::StartAction`、`finance::stage::Visible` 改成 `pub(crate)`，并非删除相应 WelcomePage 或金融组件。Markdown 解析树的类型来源、垂直模块的依赖路径，以及 desktop 的路径是后续任务需要重新核对的边界。

## 自定义实现替换判断

本轮替换的是依赖入口、初始化和框架事件 API，**没有用相似但语义不同的上游控件替换已经验收的应用交互**。

- **标签滚动条保留**：新版上游能控制 opacity，但厚度固定为主题 6 logical px；活动显示立即出现、等待 idle 后淡出。没有 Githux 所需的每实例 3 px、保留窄条带布局、140 ms 可反向淡入淡出和拖动回调接口。现有细条和隐藏后无透明命中区域保持不变。本次迁移不等于 PR #29 的统一 scrollbar 已集成。
- **应用级标签保留**：上游 Tabs 是内容页选择器。Githux 的资源去重、会话状态、关闭规则、中键关闭、拖拽排序和固定刷新工具栏仍由 Workspace/Shell 管理。
- **搜索卡片保留**：新版 Palette 的 Enter capture 仍直接运行当前项，没有检查 TextInput 的 marked range。Githux 对组合态 Enter 的先行拦截仍有必要；升级依赖不删除这一保障。
- **Windows 标题栏适配保留**：项目已验收的 Windows App SDK Tall 系统按钮、DPI / DWM 回退仍由平台层负责，不因上游也有 TitleBar 就替换。
- **字体注册保留**：上游默认字体与产品约定不同，继续使用固定的 Noto Sans / Google Sans Code。

未来 content-reader 需要按新版本评估 `ely-documents` / `ely-editor`，避免重新加入整个 facade 或 vendor。未合并分支后续整合时应统一 Cargo pin / GPUI 版本、核心 import 与测试事件类型；本记录不授权合并或改变那些 PR 的范围。

## 验证

Windows 本机使用 Rust 1.98.1 验证，以下原始退出码均为 0：

- `cargo fmt --all -- --check`
- `cargo check --locked --all-targets -j 2`
- `cargo clippy --locked --all-targets -j 2 -- -D warnings`
- `cargo test --locked -j 2`：**345 passed，0 failed**，包括窗口初始化、标签打开/关闭/切换/重排、水平滚轮与滚动条拖动/淡入淡出、toolbar 固定间距、搜索与 IME 组合态、服务层回归。
- `cargo build --locked -j 2`
- `git diff --check`

首次测试发现旧 `secondary-k/w` 静态注册导致启动 panic，修复后完整测试通过；不是只改版本号再以编译成功作为迁移完成证据。关闭默认字体后，使用实际 Assets 和字体注册路径的 Windows binary 已启动，进入有标题的窗口并达到 input-idle，没有启动错误输出。Windows App SDK app-local runtime 准备成功。

真实 GUI 自动操作未完成：Computer Use 报 native pipe 不存在，按工具流程重试及重置后仍失败。因此窗口启动只能证明启动路径可运行，不代表字体/图标显示、IME 候选、滚轮手感、DPI 或原生窗口按钮已经通过视觉验收。已启动本次独立预览供负责人检查；macOS/Linux 本机未运行，跨平台证据以后续 PR CI 为准。

建议人工检查：启动 Home 的字体/图标与边距；Ctrl+K 查询和中文输入法候选确认；打开多个仓库后的横向滚动/细条渐变/拖动排序/中键关闭；刷新 toolbar 位置；Windows 系统标题栏与缩放。其他未合并任务的新页面或新侧栏不在这份主线预览中。

保持 Draft、未合并。负责人手动验收之后才讨论合并，仍需当前提交的 CI 与审查通过。
