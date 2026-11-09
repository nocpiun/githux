# Native Shell 迁移计划

更新：2026-11-09。状态：路线已由负责人确认，迁移实现尚未完成。

## 已确认决定

- 完整迁移到 Native Shell 的原生 Windows 后端，使用纯 Rust；不采用 GPUI 嵌入、TSX 或 QuickJS 路线。
- 以 main `11f63e875a81feec3a3c2ccc30fec278454076c4` 的布局、字体和已确认交互为基线。十个 UI 赛马分支独立保留，未选定、不纳入本次迁移。
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

保持 Noto Sans、Noto Sans SC、Google Sans Code；深色现有外观、侧栏/island/两层 Tabs、手动尺寸偏好、内部导航、圆形头像、相对时间、图标加载/错误、Home open 条件与三类 Feed、Notifications 已有动作与禁用项均不变。最终 Settings 新设计/新功能、Issue/PR 新详情内容不属于本轮。

Reader 是单独的兼容性关口：Markdown/GFM/已支持 HTML、跨块选择复制、链接按固定仓库 commit/path 解析、行号与只读源码、相对图片与缓存、安全来源策略、正文 padding、Profile Flow 完整高度、超长/二进制/空内容处理都须逐项覆盖。优先验证 NSH TextView/Editor 等原生能力；不能以有同名组件当成通过，也不能静默降级、自动外跳或绕过现有 ImageProvider 的授权路径。

## 阶段与所有权

第一阶段立即并行三个 agent：

| 任务 | 分支 | 唯一写入范围 | 阶段产物 |
| --- | --- | --- | --- |
| native-shell | migration/native-shell | Cargo/入口/构建配置、src/nsh/mod.rs、src/nsh/foundation/**、src/nsh/shell/**、公共主题控件、总计划 | 能运行的纯 Rust 原生窗口、公共控件/字体/输入/虚拟列表探针，依赖锁定与接线契约 |
| nsh-runtime | migration/nsh-runtime | src/nsh/runtime/**，独立测试与自身说明 | 复用现有服务的框架无关运行时/账户/会话桥接，明确 snapshot/command/异步及撤权接口 |
| nsh-reader | migration/nsh-reader | src/nsh/reader/**，独立探针/测试与自身说明 | NSH Reader 实现与真实能力矩阵，图片/链接适配契约，缺口与验证证据 |

每个 agent 单独 worktree、自己的 target，不触碰 main 或其他实验；Cargo 共享 mutex 仍执行。基础作者拥有 Cargo.toml/Cargo.lock/顶层模块，其他作者需要临时验证时在各自 .agents/task/ 下建隔离 probe manifest，不改共享入口或锁文件。所有权依赖写成 integration-notes，不能复制另一位的实现来解除等待。

第二阶段在公共契约与最小原生运行证据就绪后拆分 Home/Feed、Repository、Profile、Notifications/全局事项、Accounts/Settings 等任务；每个页面连同控制器与行为测试作为一个交付单元。shared work_item_row、header/gutter、theme/fonts/tooltip 仍有单一作者。各页只调用 runtime 与 reader 的已验证接口，不各自新建账户服务或 CacheStore。

集成作者只集成执行者明确 ready 的本地提交；不得搬入仍在修改的工作区、自动合入 main 或把未集成页的占位算作完成。迁移中可暂时并存旧入口作为对比，最终正常启动须完全走 NSH 并清除正式构建对 Ely/GPUI 的依赖。

## 本地提交与交付

这是独立分支的本地协作迁移，覆盖 dispatch local-only 模板的“不要 commit”缺省：允许实现者在自己的任务分支提交已验证成果以便集成；禁止 push、创建 PR、远端留言或合入 main。临时分支通过本地线性摘取/整合进入 migration/native-shell；发布/PR另按负责人后续指令。

各任务交付 .agents/task/implementation-result.md、integration-notes.md、实际 run 记录和自己主题的 devlog，delivery-state.json 记录阶段、commit、依赖与未覆盖项。阶段完成不等于整项迁移完成。无需不停轮询另一个 agent 或重复全量编译。

## 验收门槛

1. 可启动的 Windows 原生 NSH 窗口；依赖树确认无 QuickJS，最终正式构建无 Ely/GPUI。
2. 所有现有正式页面和动作接到实际数据服务；账户切换、旧请求、缓存离线/撤权与退出路径保持正确。
3. 字体中英混排、IME、Tab/Enter/Space/快捷键、窗口标题栏/resize、列表滚动与动态高度、图片裁切及 Reader 选择/链接/Flow 以原生证据核验。
4. 现有业务回归继续保留；GPUI UI 场景映射到 NSH headless/原生测试，不通过删除测试/关掉模块/隐藏失败宣称完成。
5. Cargo 全部通过 node scripts/verify.mjs -- cargo ...；采用定向验证和共享编译队列。本轮可统一命令环境 CARGO_PROFILE_DEV_DEBUG=0、CARGO_PROFILE_TEST_DEBUG=0、CARGO_INCREMENTAL=0，不能修改全局配置。
6. 精确记录 Windows 实测、headless 与其他平台构建的边界；不能以截图、编译或框架官方宣称替代真实功能验收。

完成标准是完整等价迁移分支及可运行交付，不是 NSH hello world 或仅把原界面嵌入 GPUI。
