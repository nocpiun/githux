# Native Shell 迁移计划

> **负责人最新要求（2026-11-09，覆盖下方旧阶段安排）**：直接替换原有 Ely 应用，根 package/default entry 改为 NSH，迁移原 src/ui，清除 Ely/GPUI 和 src/nsh / native/nsh 双轨结构。具体路径、并行所有权与页面契约见 [原位替换执行契约](native-shell-inplace.md)。下方第一阶段内容仅为历史记录，不是当前可验收成果；不再以 githux-nsh 预览代替应用迁移。


更新：2026-11-09。状态：第一阶段三项成果已本地汇总；Runtime、Reader 与原生基础进入统一 `githux-nsh` package，完整页面迁移尚未开始。

## 已确认决定

- 完整迁移到 Native Shell 的原生 Windows 后端，使用纯 Rust；不采用 GPUI 嵌入、TSX 或 QuickJS 路线。
- 以 main `11f63e875a81feec3a3c2ccc30fec278454076c4` 的功能、布局结构、字体和已确认交互为基线。负责人后续明确：视觉样式使用 NSH 默认主题和控件，不要求复刻 Ely；此项覆盖早期“保持 Ely 画风/色值”的迁移要求。十个 UI 赛马分支独立保留，未选定、不纳入本次迁移。
- 最终成果汇总在 `migration/native-shell`；本轮不合入 main。临时任务分支只为隔离并行实现。
- 保持已实现功能等价；Windows 为本轮运行与原生验收目标。现有其他桌面平台编译能力须明确记录与维护，不把 NSH 支持某平台当作 Githux 已验收，不扩展到移动/Web/TUI 产品。

## 技术基线与依赖

2026-11-09 核对官方站点和仓库：Native Shell v0.4.0；tag 对应 commit `e626a8dd8f8d8b277b9507cfd5ac74d0e67020f3`。基础 agent 以该 release 的源码/API为准，先验证可获得的 crate、feature 和本项目 Rust 1.98.1 工具链兼容性，再锁定 Cargo.lock。禁止跟随浮动 main 或不经验证照抄文档版本号。

NSH 默认 feature 可能包含 JS；纯 Rust 路线明确关闭 JS/QuickJS 相关默认 feature，仅启用所需能力。原生 Rust 应用可直接 cargo run，NSH CLI 仅在确有工具/打包需求时使用，不先执行远程安装脚本。

一手参考：
- https://nsh.dev/docs/rust/
- https://nsh.dev/docs/windows/
- https://nsh.dev/elements/text-view/
- https://nsh.dev/docs/fonts/
- https://nsh.dev/docs/lists/
- https://github.com/longbridge/nsh/tree/v0.4.0

## 保留的业务与体验

认证、GitHub、network、domain、data、storage 与 content 的既有逻辑优先复用。账户 SQLite、Windows Credential Manager、CacheStore 路径/格式/权限隔离不因 UI 迁移改变。保留账户代次、资源身份、异步旧响应拦截、授权撤销、已显示内容清除、取消与退出提交门槛；UI 线程不得做阻塞存储/网络。

需要迁移的是 GPUI Entity/Context、窗口与事件、页面控制器、焦点/滚动/会话 UI 状态及 Ely 控件。workspace 目前混有 GPUI FocusHandle/ScrollHandle，需把纯路由/账户会话语义与新 UI 句柄分开。

保持 Noto Sans、Noto Sans SC、Google Sans Code；侧栏/island/两层 Tabs 的信息结构、手动尺寸偏好、内部导航、圆形头像、相对时间、图标加载/错误、Home open 条件与三类 Feed、Notifications 已有动作与禁用项均不变。视觉采用 NSH 自带默认主题（含浅/深色，跟随系统），不覆写 Ely 调色板或固定圆角；NSH 基础控件本身无强制外观，需要的布局与交互状态直接消费默认语义 token。已确认的黄色警告等状态语义仍保留。最终 Settings 新设计/新功能、Issue/PR 新详情内容不属于本轮。

Reader 是单独的兼容性关口：Markdown/GFM/已支持 HTML、跨块选择复制、链接按固定仓库 commit/path 解析、行号与只读源码、相对图片与缓存、安全来源策略、正文 padding、Profile Flow 完整高度、超长/二进制/空内容处理都须逐项覆盖。优先验证 NSH TextView/Editor 等原生能力；不能以有同名组件当成通过，也不能静默降级、自动外跳或绕过现有 ImageProvider 的授权路径。

## 阶段与所有权

初始基础交付（`423cae9`；下方联合预览为集成后的最新状态）：

- 独立 workspace 成员 package `githux-nsh`（`native/nsh/Cargo.toml`），lib `src/nsh/`、bin `githux-nsh`；旧 GPUI 应用保留在根 package 作对照。
- 固定 NSH v0.4.0 `e626a8dd8f8d8b277b9507cfd5ac74d0e67020f3`，关闭 `nsh-platform-windows` 默认 `js`；依赖树无 QuickJS/GPUI/Ely。
- 真实 Windows 原生窗口运行（标题 Githux，渲染验证），headless 探针 2 项通过。
- 公共接口在 `src/nsh/foundation/**`，Runtime 契约见 `src/nsh/runtime/README.md`，Reader 契约见 `docs/nsh-reader.md`。

第一阶段三个 agent 的原始分工（均已交付，本轮已正常退出并交接给协调者）：

| 任务 | 分支 | 唯一写入范围 | 阶段产物 |
| --- | --- | --- | --- |
| native-shell | migration/native-shell | Cargo/入口/构建配置、src/nsh/mod.rs、src/nsh/foundation/**、src/nsh/shell/**、公共主题控件、总计划 | 能运行的纯 Rust 原生窗口、公共控件/字体/输入/虚拟列表探针，依赖锁定与接线契约 |
| nsh-runtime | migration/nsh-runtime | src/nsh/runtime/**，独立测试与自身说明 | 复用现有服务的框架无关运行时/账户/会话桥接，明确 snapshot/command/异步及撤权接口 |
| nsh-reader | migration/nsh-reader | src/nsh/reader/**，独立探针/测试与自身说明 | NSH Reader 实现与真实能力矩阵，图片/链接适配契约，缺口与验证证据 |

每个 agent 单独 worktree、自己的 target，不触碰 main 或其他实验；Cargo 共享 mutex 仍执行。基础作者拥有 Cargo.toml/Cargo.lock/顶层模块，其他作者需要临时验证时在各自 .agents/task/ 下建隔离 probe manifest，不改共享入口或锁文件。所有权依赖写成 integration-notes，不能复制另一位的实现来解除等待。

第二阶段在公共契约与最小原生运行证据就绪后拆分 Home/Feed、Repository、Profile、Notifications/全局事项、Accounts/Settings 等任务；每个页面连同控制器与行为测试作为一个交付单元。shared work_item_row、header/gutter、theme/fonts/tooltip 仍有单一作者。各页只调用 runtime 与 reader 的已验证接口，不各自新建账户服务或 CacheStore。

集成作者只集成执行者明确 ready 的本地提交；不得搬入仍在修改的工作区、自动合入 main 或把未集成页的占位算作完成。迁移中可暂时并存旧入口作为对比，最终正常启动须完全走 NSH 并清除正式构建对 Ely/GPUI 的依赖。

## 第一阶段联合预览

三个任务的本地成果已汇总至 `migration/native-shell`：基础 `423cae9`、Runtime 原提交 `065187f`、Reader 原提交 `7f1f669`。根 package 仍保留旧 GPUI 应用；NSH 的 `native/nsh` 使用同一套 auth/content/data/domain/github/network/storage 源码，不复制或改写账户及缓存格式。

- 启动与编译命令见 [README](../README.md#native-shell-迁移预览本分支)。`githux-nsh` 窗口可切换 Foundation / Reader，字体与主题由统一 Root 提供。
- Runtime 已纳入正式 package 编译及离线行为测试；预览不启动生产 Runtime、不访问用户账户。账户 UI 和各业务页面消费 Runtime 属于下一阶段，不能将编译通过当成正式接线。
- Reader 的兼容测试、独立预览和联合窗口使用同一 package / Cargo.lock / target；原独立测试 manifest 已撤除，避免探针与真实装配分叉。
- 集成检查修复了基础 toolbar 被构造后丢弃、无法真正点击的问题，添加按钮回调、输入与页面切换回归。真实失败控件恢复为黄色 warning 图标 + tooltip + 点击重试，复用已确认设计。
- 联合验证：`cargo test -p githux-nsh --locked --offline` 570 项库测试 + 5 项 Reader 兼容测试通过；严格 Clippy、fmt、预览二进制构建通过。依赖图无 GPUI/Ely/QuickJS。仅确认 Windows 窗口成功启动，负责人要求最终视觉效果由本人验收，协调者未做 GUI 交互或截图验收。
- 后续样式调整已移除 Ely 色值和固定圆角，Root 使用 NSH 默认主题加原字体族，控件消费默认 token，按钮使用 NSH Button；窗口使用 NSH 默认系统外观。最终 shell/island/导航布局、真实 IME、多 DPI、Reader 大文档虚拟化及图片持久缓存接线仍待完成。当前不能作为完整应用替换 main。

## 本地提交与交付

初期采用独立分支本地协作，允许本地提交并线性摘取到 migration/native-shell。负责人已于 2026-11-09 进一步授权：推送该汇总分支并创建指向 main 的草稿 PR，供 GitHub 查看 diff；关联 First Release 并保留 pending 验收标签。该授权覆盖初期“不 push/PR”的限制，仍不授权合入 main。后续迁移继续在此分支/PR 累积，完整实现与负责人验收完成前保持 Draft。

各任务交付 .agents/task/implementation-result.md、integration-notes.md、实际 run 记录和自己主题的 devlog，delivery-state.json 记录阶段、commit、依赖与未覆盖项。阶段完成不等于整项迁移完成。无需不停轮询另一个 agent 或重复全量编译。

## 验收门槛

1. 可启动的 Windows 原生 NSH 窗口；依赖树确认无 QuickJS，最终正式构建无 Ely/GPUI。
2. 所有现有正式页面和动作接到实际数据服务；账户切换、旧请求、缓存离线/撤权与退出路径保持正确。
3. 字体中英混排、IME、Tab/Enter/Space/快捷键、窗口标题栏/resize、列表滚动与动态高度、图片裁切及 Reader 选择/链接/Flow 以原生证据核验。
4. 现有业务回归继续保留；GPUI UI 场景映射到 NSH headless/原生测试，不通过删除测试/关掉模块/隐藏失败宣称完成。
5. Cargo 全部通过 node scripts/verify.mjs -- cargo ...；采用定向验证和共享编译队列。本轮可统一命令环境 CARGO_PROFILE_DEV_DEBUG=0、CARGO_PROFILE_TEST_DEBUG=0、CARGO_INCREMENTAL=0，不能修改全局配置。
6. 精确记录 Windows 实测、headless 与其他平台构建的边界；不能以截图、编译或框架官方宣称替代真实功能验收。

完成标准是完整等价迁移分支及可运行交付，不是 NSH hello world 或仅把原界面嵌入 GPUI。
