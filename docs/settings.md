# Settings 与 Accounts 原生组件

本任务实现独立 GPUI/Ely Settings 组件和离线联合预览。正式 Shell 仍使用原账户页面；本 PR 不增加路由、不改现有账户控制器或存储实现。最终原生视觉与交互需负责人验收，PR 保持 `pending`。

## 已确认范围

公开分类只有 Accounts：添加、选择当前账户、重新验证、替换 PAT。无移除、登出、外观/网络/缓存偏好、Keyboard/About 或仓库设置入口。

宽态分类行沿用正式 Shell 侧栏密度：Ely `ControlSize::Md` 行高（而非固定像素）、8px 图文 gap、`Radius::Md` 圆角、`fg_muted`/`hover`/`fg` 主题层级；当前分类左侧画与主侧栏同参数的 2px 短 indicator（宽 2、高度按 2/3 行高 clamp 20–24、右侧圆角 4、左直角，`fg_muted`，在 8px lane 内、不占点击目标、只有当前项有）。窄态继续用横向 Tabs 下划线，不复制该 indicator。宽态是 168px 分类列、32px 间隔和最大 720px 内容列；组件实际可用宽度低于 760px 时改为单行 icon + label Tabs。分类条独立横向滚动，页面内容仅纵向滚动。预览额外分类明确标为 Fixture，不属于产品配置。

窄态分类 Tabs 与仓库页内容 Tabs 使用同一组主题参数，而不是另造视觉：`TextSize::Sm`、`IconSize::Xs`、icon-label `gap_1p5`、单项 `px_2`/`py_2`、项间 `gap_2`；未选 `FontWeight::NORMAL` + `fg_muted`，选中 `MEDIUM` + `fg`，hover 文字 `fg`；选中底部 2px `accent` 下划线与整条 1px `border` 基线。键盘 focus 用背景色表达，不增加边框尺寸；横向滚动容器就是 tab 行本身，使 `scroll_to_item`、选中末项 reveal 与滚轮/滚动条继续有效，滚动条画在下方预留带而不是压住下划线。

PAT 表单默认收起。Add account 和账户行 Replace PAT 在列表下方打开表单，替换绑定被点击身份，允许目标为非当前账户。当前账户用 Current 标记，不生成 Switch 控件。账户状态来自服务权威目录，Verified、PendingVerification、MissingCredential、CredentialUnavailable、过期/拒绝与连接失败保持区别。

## 宿主接口与状态归属

源码在 [`src/ui/settings/`](../src/ui/settings/mod.rs)，当前由 [`examples/settings.rs`](../examples/settings.rs) 直接装配，尚未注册进 `ui/mod.rs`。

```rust
let binding = cx.new(|cx| SettingsBinding::new(existing_accounts_controller, window, cx));
let settings = cx.new(|cx| SettingsView::new(binding, window, cx));
```

- `SettingsBinding` 接收宿主既有的 `Entity<app::accounts::Accounts>`；不创建服务、网络、系统凭据或数据库。稳定身份继续使用 `AccountKey { host, id }`。
- `SettingsView` 持有页面与分类条的真实 ScrollHandle、公共 Motion/Bars、分类焦点；按实际布局宽度响应，不从窗口宽度猜侧栏状态。生产 `fixtures = 0`，`Category::Accounts` 是唯一产品分类。
- Binding 只持有表单模式、目标、帮助显隐、输入实体与返回焦点。输入为 Ely masked TextInput + PasswordInput，视图展示数据不含 PAT。
- 提交把输入直接移交现有控制器 `submit`，立即重建输入实体以释放秘密与撤销历史。控制器负责验证、提交、in-flight、取消 generation、旧响应与权威对账，详见[现有账户契约](account-ui-preview.md)。关闭表单不宣称在途提交已回滚。
- 成功收起根据权威账户目录的身份/授权代次变化判定，不依赖控制器最新状态；成功提交后的自动仓库读取失败属于独立消息，不会让已提交的表单保持打开或误报 PAT 未保存。控制器自身的失败与部分成功消息保留。完成/取消返回 Add account 或原行 Replace PAT；Enter/Esc 先检查 IME marked range，组合输入不被当作提交/取消。
- 头像直接复用 `ui::avatar` 的圆形绘制与空 mask 防护。主题、字号、字体和公共滚动条均沿用当前 Ely/应用 token；不引入新的主题或 scrollbar 算法。

未来 Shell 宿主由协调者注册本模块，传入同一 app 级控制器并保留实体生命周期。宿主隐藏/释放时调用 `SettingsView::cancel_interactions` 清理滚动交互；本任务不负责 Shell 会话集成。Rust 可见性沿用仓库内部组件的 `pub(crate)`，不是另发公共 crate API。

## 可重复离线预览

```sh
node scripts/verify.mjs --no-lock -- cargo fmt --all
node scripts/verify.mjs -- cargo check --locked --all-targets
node scripts/verify.mjs -- cargo clippy --locked --all-targets -- -D warnings
node scripts/verify.mjs -- cargo build --locked
node scripts/verify.mjs -- cargo test --locked
node scripts/verify.mjs -- cargo build --locked --example settings
node scripts/verify.mjs -- cargo test --locked --example settings settings::
git diff --check
```

只为原生检查运行 preview：`GITHUX_SETTINGS_PREVIEW_FORM=replace`（或 `add`）会让独立预览在 fixture 自动读取完成后直接打开对应表单；不设置时从正常收起状态启动。该变量只由 `examples/settings.rs` 读取，不进入产品 binary。

预览窗口保留 200px 展开侧栏空间，最小 880px；侧栏块只表示空间预算，不模拟正式 Shell 接线。顶部选择 Normal、Expired、Missing credential、Restored、Network failure、Repository read failure、Slow / cancel，每次重新建立隔离场景。Fixture categories 开关在实际 Accounts-only 配置与五个布局 fixture 分类之间切换。

[`settings_fixture.rs`](../src/bin_support/settings_fixture.rs) 是预览和测试共用的可重复传输。按请求路径与合成输入应答，不使用一次性响应队列：`preview-alex`、`preview-sam`、`preview-jordan` 分别对应虚构身份；其他输入返回 401，错身份替换由真实 AccountService 拒绝。`/user/repos` 明确返回 fixture 空集合，未建模请求返回开发错误 599。

所有操作复用真实 Accounts controller、AccountService、后台执行器、SQLite 目录与 InMemoryCredentialStore。每个场景只使用系统临时目录 `githux-settings-<UUID>`；不调用应用数据目录或 WindowsCredentialStore，不读取真实账号、不访问 GitHub。场景销毁先关闭服务，再异步清理临时文件。帮助按钮可以由操作者打开官方 PAT 创建页，但不创建 token。不要在该离线窗口输入真实 PAT。

## 验证边界

行为测试位于组件旁，使用现有 Cargo example 的聚焦测试命令 `node scripts/verify.mjs -- cargo test --locked --example settings settings::`，不新增 `tests/settings.rs` 整应用测试 target。测试检查真实服务回调、非当前替换目标、身份不匹配、取消/显隐/掩码/焦点、重复提交、授权状态和窄态布局。

GPUI 测试平台不是 Windows 原生输入、IME 或真实绘制证据；HTML/PNG 仅作批准设计参考。实际执行结果、原生预览记录及未覆盖项维护在[本任务开发日志](devlog/2026-11-01-settings-accounts.md)。真实 PAT、正式 Shell 组合和非 Windows 产品验收不在本任务的证明范围。
