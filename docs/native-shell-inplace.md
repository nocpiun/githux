# Native Shell 原位替换：执行契约

2026-11-09 负责人纠正：必须替换现有 Ely 应用；不接受另建 src/nsh / githux-nsh 作为迁移交付。本文件覆盖旧阶段文档和任务提示中的双入口安排。旧原型代码只作为可搬迁实现来源，不能作为继续保留第二套应用的理由。

## 最终结构与交付

- 根 package 仍叫 githux；src/main.rs -> src/app 启动真正的 NSH 应用。默认 cargo run/build 运行新应用，不再通过 -p githux-nsh 选择另一个程序。
- 现有 src/ui 下原位替换窗口、组件、页面和 Reader；移走可用的新实现后删除 src/nsh 和 native/nsh。Cargo 的正常、开发和各平台依赖以及锁文件不再包含 Ely/GPUI；不引 JS/QuickJS。
- NSH 固定 e626a8dd8f8d8b277b9507cfd5ac74d0e67020f3，使用默认样式和系统外观。保留已确认字体、功能、信息结构和交互，最终视觉由负责人验收，不运行 Computer Use。
- auth/content/data/domain/github/network/storage 为原业务层，优先保持不变；不得复刻另一套服务、账户库、凭据或缓存，也不得以隐藏页面、删回归或 feature 默认禁用来消除编译错误。
- migration/native-shell / Draft PR #75 汇总；不得合入 main。所有工作分支仅本地提交，集成作者摘取 ready 提交。

## 已有代码的目标路径

| 可复用来源（集成基线） | 最终路径 | 唯一写入者 |
| --- | --- | --- |
| src/nsh/runtime/** | src/app/runtime/**；sessions 移到 src/workspace/mod.rs，runtime 可 re-export | native-shell 集成作者 |
| src/nsh/foundation/** | src/ui/foundation/**；字体可放 src/platform/fonts.rs | native-shell 集成作者 |
| src/nsh/reader/** | src/ui/reader/**，替换旧 GPUI Reader | nsh-reader-inplace |
| 原 app/pages/home、profile 与 ui/home、profile | 原路径，NSH 实现与真实服务接线 | nsh-home-profile |
| 原 ui/repository、repository_overview | 原路径，NSH 实现与真实服务接线 | nsh-repository |
| 原 ui/notifications、global_work_items 和 app/pages 对应控制器 | 原路径 | nsh-global-pages |
| 原 ui/accounts、settings | 原路径，调用既有 Runtime 账户命令 | nsh-accounts |

Cargo、顶层 mod/lib/main、app/pages/mod.rs、workspace、platform、通用 UI 和 CI/全局测试注册仅集成作者修改。页面作者拥有对应 examples/tests 和其内部测试；不得修改其他页或公共文件解除编译依赖。通用 work item row 由集成作者统一移植供 Home 和全局列表复用。根 README、AGENTS 和总计划由集成作者收尾，主题文档/本任务 devlog 由各执行者维护。

## 最小页面接线契约

集成作者在 src/app/pages/mod.rs 提供以下契约；页面作者不得各自复制该生产模块。若 NSH 源码证据要求调整，先在 integration-notes 明确兼容适配，不改业务范围。

```rust
use crate::app::runtime::{PageContext, RuntimeHandle};
use crate::workspace::{Route, TabId, RepositoryNavigation};
use nsh_core::prelude::AnyElement;
use std::rc::Rc;

pub enum PageNavigation {
    Open(Route),
    Repository { route: Route, intent: RepositoryNavigation },
    External(String),
    CanonicalProfile { tab: TabId, login: String },
}
pub type NavigationSink = Rc<dyn Fn(PageNavigation)>;

pub trait PageView {
    fn element(&self) -> AnyElement;
    fn poll(&mut self, runtime: &RuntimeHandle);
    fn refresh(&mut self, runtime: &RuntimeHandle);
    fn set_active(&mut self, active: bool);
    fn retire(&mut self);
}
```

页面工厂：
- app::pages::home::HomePage::new(context: PageContext, navigation: NavigationSink)
- app::pages::profile::ProfilePage::new(context: PageContext, login: String, navigation: NavigationSink)
- ui::repository::RepositoryPage::new(context: PageContext, owner: String, name: String, intent: RepositoryNavigation, navigation: NavigationSink)
- app::pages::notifications::NotificationsPage::new(context: PageContext, navigation: NavigationSink)
- app::pages::work_items::WorkItemsPage::new(context: PageContext, route: Route, navigation: NavigationSink)（仅 Issues / PullRequests）

以上都实现 PageView。工厂返回 Self，不再携带 GPUI Window/App/Entity。将旧控制器中的服务逻辑搬到原路径的 NSH 实现，不把页面退化成纯展示 fixture。组件用 NSH signal / dynamic；用户事件写入页面自己的 UI 线程队列/状态，poll 分发异步工作并接收结果。后台只访问既有 Services::spawn/捕获的 page_session/repository_session 与取消令牌，不能调用 UI 信号。

Shell 在前台定时 tick/wake 中 drain Runtime 并 poll 已挂载页面（包括非活动页的撤权检查）；切换/替换/删除账户及 binding_revision 变化时 retire 旧页，按新的 PageContext 重建；关闭 tab retire。页面自己的刷新代次使用 PageSlot/PageTicket，publish/value/clear_invalid 以 runtime.sessions() 校验。已经显示的数据同样检查授权，不得只拦截新请求。异步数据不能在 element/render 内发起 I/O。

Settings/登录特殊工厂：ui::accounts::AccountsView::new(navigation: NavigationSink)，提供 element(&self)->AnyElement、poll(&mut self, runtime: &mut RuntimeHandle)、retire(&mut self)。组件通过队列向 Runtime 提交 Command，PAT 不进日志/snapshot；Shell 的登录状态和 Settings 都复用此对象。ui::settings 可封装导航布局，不新建账户服务。

Reader 最终保持现有 NSH ReaderHandle/ReaderDocument/ResourceContext/ImageBridge 的公开签名，唯一前缀变更为 crate::ui::reader；宿主消费真实 ImageProvider、固定资源身份和被捕获的授权。页面作者可读原 src/nsh/reader 作为 API 参考，不能复制 Reader 或改为自动浏览器跳转。

## 并行验证与集成边界

共同基线仍含旧入口，因此独立任务本地可在被忽略的 .agents/task/probe 内用临时 manifest/模块装配编译自己实际的 src/ui/src/app 文件。探针不得提交为另一个生产 package，也不能把它通过当作完整应用通过。缺少共享模块时可用最小契约适配作定向验证；最终必须与集成作者的真实契约组合验证。

每任务提交 implementation-result.md 和 integration-notes.md，写准确 commit、API、实际测试、迁移前后回归映射与剩余事项。只交付自己拥有的文件；对旧共享测试/模块的改动建议交集成作者。所有 Cargo 使用 scripts/verify.mjs，共享 mutex，dev/test debug=0，incremental=0。工作已完成后不重复跑全套、不忙等其他 agent。

最终应用检查包括默认入口构建、全目标检查、实际依赖树、既有业务回归和新页面组合验证；无法保留的场景必须明确报告，不能静默删除。NSH 默认样式不等于可以舍弃真实页面功能。Windows 原生效果最终由负责人看运行结果。
## 2026-11-09 组合集成

六个任务已汇入 migration/native-shell，原路径上的真实页面替换契约桩。协调者补齐根模块和测试入口、Settings 宿主、头像/README 图片绑定、启动恢复状态与退出清理；取消旧 Windows App SDK runner 和打包步骤。Profile 与 UserProfile 使用同一资源工厂。共享服务与缓存业务测试在根库中执行，重复的 repository 测试装配仅保留一份。

此状态仍是待验收的迁移分支，不是 main 发布。原生鼠标/IME/跨屏、完整键盘导航和标签拖拽、长文 Reader 选区等不能由编译或 headless 检查代替，详见本轮开发日志。

宿主现在仅在账户快照、导航、头像完成、挂载结构或页面占位状态实际变化时重建，而非每 200ms 重建所有控件。`PageView::host_presentation` 默认空，仅 Profile 用它通知首次资料/错误占位切换；常规正文仍由各页面自己的信号更新。`set_active` 只在活动 tab 改变时调用，Home 的相对时间时钟也不再每个 tick 被重置。
