# Native Shell 第一阶段集成

- 日期：2026-11-09；执行者：主 Agent。本地汇总至 migration/native-shell，不 push/PR、不合 main。
- 三个原执行器均正常 /quit，dispatch 确认 exited、无子进程及写入锁后接管。基础 423cae9 保留，Runtime 065187f 摘取为 3e107ac，Reader 7f1f669 摘取为 a2ff620。

## 改动

- 将 Runtime、Reader 与既有无 UI 业务模块装配到 githux-nsh；沿用单一 Cargo.lock，NSH 固定 v0.4.0/原 rev，关闭 JS。原 GPUI 应用保留作后续等价对照。
- Reader 的独立测试 crate 改为同一 package 的 test/bin 目标，fixture 移到 src/nsh/preview/reader.rs。联合预览可在 Foundation 与 Reader 之间切换；尚无真实账户与业务页面消费。
- 检查发现原 Foundation toolbar 只构造但没有挂载，按钮/input 均不可见。实际挂载并用响应式计数测试点击、warning 直接重试、输入与切换回归。warning 修正为黄色图标/tooltip，不加 Retry 文案。
- 统一 Root 注册 UI 四种字重和代码字体、Reader 基线色值；字体测试明确只证明 Root 请求字体族，不冒充逐字形回退验收。相关教训已覆盖于 LESSONS L002/L012/L017，无重复新增。

## 验证

均通过 scripts/verify.mjs，环境 dev/test debug=0、incremental=0，独立 target、共享编译 mutex。

- 严格 Clippy：cargo clippy -p githux-nsh --all-targets --locked --offline -- -D warnings，通过（290fc29f）。
- 联合回归：cargo test -p githux-nsh --locked --offline，570 库测试 + 5 Reader 兼容测试通过（7d53ff20）。覆盖现有业务层、账户隔离/撤权/退出、Foundation 控件与预览切换、Reader HTML/图片/Flow。
- 构建：cargo build -p githux-nsh --bin githux-nsh --locked --offline，通过（4d2ffa67）。
- 格式：cargo fmt -p githux-nsh -- --check，通过（c3d71c77）。
- 依赖树：cargo tree -p githux-nsh --edges normal --prefix none --locked --offline，通过；无 GPUI/Ely/QuickJS（b362e712）。
- Windows：启动 target/debug/githux-nsh.exe，系统返回标题 Githux - Native Shell migration preview 的窗口。负责人随后明确效果由本人验收，停止 Computer Use；没有截图或交互验收结论。

最初联合 check 期间追加主题接线，触发旧 lib 元数据与更新中 bin 的编译不一致；停止修改后完整 Clippy/测试/构建通过。初次 Clippy 因将原私有核心模块全量公开触发原有内部 helper 的 public API lint，恢复包内可见性后通过；未更改核心业务行为或压制 lint。

## 剩余工作

见 docs/native-shell-migration.md。完整 Shell/Home/Feed/Repository/Profile/Notifications/Issues/PR/Accounts 页面尚未迁移。Runtime 现在编译及行为测试已接线，生产 UI/真实账户尚未接线。Reader 大文档虚拟化、真实图片缓存消费、IME/多 DPI/跨块复制等仍保留为后续验证项。负责人先验收此阶段预览。