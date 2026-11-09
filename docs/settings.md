# Settings 与 Accounts

2026-11-09 Native Shell 原位迁移：`src/ui/accounts/**`、`src/ui/settings/**` 替换旧 GPUI/Ely 实现，采用固定 NSH `e626a8dd8f8d8b277b9507cfd5ac74d0e67020f3` 的默认控件与语义 token。原位路径和唯一 Runtime 契约以 [执行契约](native-shell-inplace.md) 为准；本任务为本地页面交付，尚不代表默认应用已组合或最终视觉已验收。

此前 GPUI Settings 组件、其原生/测试证据保留在 Git 基线 `01decd2` 和 [旧开发日志](devlog/2026-11-01-settings-accounts.md)。旧组件不是最终 Settings 设计的批准。本次只迁移账户功能和既有分类布局，不增加偏好设置项目。

## 页面与宿主

公开产品分类只有 Accounts。宽态保留 168px 分类列、32px 间隔、最大 720px 内容列；分类行高 32px，选中项左边有 2×20px 短指示线。窄态（组件实际可用宽度低于 760px）为横向 NSH Tabs 与原生下划线、独立横向滚动；正文只纵向滚动。两种分类导航都使用 NSH Tabs/Tab 的键盘、选中与无障碍语义。字体由共享 foundation/root 提供，控件不复制 Ely 色表。

```rust
let accounts = crate::ui::accounts::AccountsView::new(navigation.clone());
let settings = crate::ui::settings::SettingsView::new(navigation);
// 在 NSH component scope 内构造；宿主保存实例，挂载一次：
let element = settings.element();
// UI 线程 tick；同一个 shell-owned RuntimeHandle：
settings.poll(&mut runtime);
// 丢弃页面前：
settings.retire();
```

`AccountsView` 同时用于登录和 Settings。表单在无保存账户时显示；已有账户时默认收起，Add account / Replace PAT 打开，成功提交后收起。替换目标是被点击账户的 `AccountKey`，可以不是活动账户。当前账户只有轻量 Current 标记，没有 Switch 按钮。

`ui::accounts::controller` 只管理本地表单、确认、消息和 UI effect 队列；不创建 AccountService、HTTP、凭据或数据库。事件只排队；`AccountsView::poll` 调用 `RuntimeHandle::submit/cancel/snapshot`，并完整替换目录、活动选择和仓库展示，包含零账户。后台永远不操作 NSH signal。重复点击在排队时即被本地门禁拦截，提交后由 Runtime 的 busy/settling 门禁接管。

Runtime 的完成结果可能与自动仓库读取同时出现在一个 snapshot。界面在该读取期间消费新的账户完成结果，并记住已消费结果；普通 poll 或仓库读取结束不重播旧成功来关闭后来打开的表单。取消账户变更不声称回滚；按钮保持 busy，直至服务完成并对账权威状态。关闭/退休页面不关闭 shell-owned Runtime。

## 账户操作与秘密输入

- Classic PAT 验证、保存、切换、重新验证、同身份替换均使用现有 Runtime Command，服务与持久化格式不变。
- Remove → Remove account / Keep account 在当前行确认；只有确认才提交 `Command::Remove`。移除后的空 snapshot 清除旧行与活动标记。该操作由本次迁移派发明确要求，覆盖旧 Settings 文档的“无移除”组件范围。
- 清理待完成或其状态读取失败保留紧凑 warning 与显式 Retry cleanup，提交 `Command::Recover`，不自动循环恢复。
- PAT 只在受控原生 field 与移交的 `Pat` 中存活，不进入 presentation、snapshot、错误文案或日志。提交、取消、切换表单与退休清空临时输入；提交/取消换新 keyed native input，释放前一输入节点的编辑/撤销历史。Show/Hide 只改变掩码，重置后回到隐藏。
- Enter 消费原生 SubmitEditing；组合态由 CompositionChange 跟踪，候选确认不提交；Esc 仅在非组合态取消。Add/Replace 发起点保留 ElementRef，取消和成功收起后返回该控件。headless 不能替代真实 Windows IME/焦点验收。
- 空输入只显示紧凑错误而不提交；身份不匹配、401、权限、SSO、网络/超时、限流、存储失败保持原错误含义。错误/警告用共享黄色标记和 tooltip；取消/成功不复制长条卡片。

## 头像、仓库与导航

`AccountsView::install_avatar_provider` / `SettingsView::install_avatar_provider` 接收集成作者的 `AccountAvatarProvider`。其 `avatar(AccountKey)` 在 UI poll 调用，仅取已准备且当前授权允许的值，不做 I/O；每次返回 None 会删除先前显示的图片。provider 必须覆盖 account/epoch/binding revision/cache scope 撤权，使用现有头像缓存/后台执行器。

`AccountAvatar::from_bytes(bytes, mime)` 只构造本地 data image source，接受 PNG/JPEG/WebP/GIF；integrator 在头像 worker 完整解码/尺寸/授权检查后缓存此值。页面没有 raw URL 图片加载回退；未安装 provider 时只显示相同尺寸圆形 initials。真实 provider 的实现、装配及跨账户/撤权组合验证仍由集成作者完成，不能把 initials 探针算作真实头像交付。

产品 Settings 的仓库浏览仍属于 Shell 侧栏。独立 Accounts example 可显式开启 `with_repository_preview()`，读取 Runtime 的完整/Partial/历史/失败状态：Partial 保留可用行并提供局部重试，失败不会显示为空集合；历史来源用与原 formatter 一致的相对时间文案。仓库点击提交 `PageNavigation::Open(Route::Repository {..})`；只 PAT 创建帮助使用 `External`。

## 开发示例和验证边界

`examples/account.rs` 使用最终 githux library 的生产 Runtime，仍按原 example 语义打开正常账户数据；初始化由 Runtime 后台完成。`examples/settings.rs` 为离线联合窗口：窗口创建前在临时目录组装真实 AccountService、SQLite、InMemoryCredentialStore 与可重复 transport；输入 `preview-alex` / `preview-sam` / `preview-jordan` 操作合成账户，其余输入返回 401，未建模端点为 599。不打开真实账户路径、不用真实凭据、不访问 GitHub。两个 example 均在事件循环结束后等待 ExitGate 再退出。

原位基线尚无最终 `app::runtime`/foundation/lib 装配，本地忽略目录 `.agents/task/probe` 仅 re-export 基线 Runtime/foundation 并提供文档要求的导航类型，编译实际拥有的页面源文件。examples 调用同一 library API，探针按 `githux` library 名检查它们。所需生产可见性与组合注册交给集成作者；不在页面分支改根 Cargo/lock/main/lib。

具体命令、run ID、旧测试映射、未覆盖项见 [本次开发日志](devlog/2026-11-09-nsh-accounts.md) 和任务本地 integration-notes。未执行真实 PAT、GitHub 私有内容、最终 Windows 原生操作、多 DPI 或其他平台验收；最终视觉由负责人确认，不使用 Computer Use 或截图。
