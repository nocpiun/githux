# 正式 Shell 的 Home / Profile / Repository 页面接入

更新：2026-10-29。状态：实现完成，等待负责人真实验收。本文记录正式 Shell 中页面 host 的挂载、生命周期、导航和验证边界；Home/Feed、Profile Overview、Repository 页面组件本身分别见 [home-view](home-view.md)、[profile-view](profile-view.md)、[repository-browser](repository-browser.md)，账户服务仍见 [账户预览](account-ui-preview.md) 与 [账户目录](account-persistence.md)。

## 这是正式入口，不是预览

`src/app/pages/**` 把已有页面组件挂到正式 `Shell` 的 workspace 标签上。产品默认启动仍是 Home；侧栏仓库、Home/Feed 内的用户与仓库链接、Profile 仓库卡片及 repository contributor 都通过同一个 `Shell::navigate` 打开或激活会话。

页面 host 只取得生产已有的能力：

- `AccountServices`：由 `app::accounts::Accounts` 提供的 `Arc<AccountService>`、`repository service` 与同一个 `BackgroundExecutor`。
- `RepositoryPageServices`：仓库页面按既有接口接收同一账户服务与 executor。
- `app::avatar` / `ui::avatar`：唯一头像 loader/cache 与圆形呈现，页面只接收已解码图片。

没有新增 PAT、HTTP 客户端、SQLite、测试账户或页面级缓存。`examples/*` 与 fixture 仍是独立开发 target，不在产品模块树或路由中。

## 页面生命周期

| 事件 | Home | Profile | Repository |
| --- | --- | --- | --- |
| 首次挂载 | 账户可用后创建 `HomeView`，分别请求 Home/Feed | 账户可用后创建 `ProfilePage`；先读基本资料，再由真实服务器 id/canonical login 绑定 `ProfileView` | 账户可用后创建 `RepositoryPage`，`activate` 捕获新 `RepositorySession` |
| 普通标签切换 | 保留两栏结果、滚动、焦点和请求身份，只停止时间更新/手势 | 保留 sections、滚动和 reader，停止背景 UI 工作 | 保留已验证内容、分支/路径/展开/reader 选择，不重读 |
| 关闭页签 | 释放 host，取消自己的请求 | 同左 | 同左；关闭一页不影响另一页 |
| 账户/授权变化 | 丢掉 host，在新上下文重建并重读 | 丢掉 host；旧 ProfileTarget/section 不复活 | 保留分支/文件/展开的导航意图；丢弃内容、固定提交、授权上下文后在新会话重读 |
| 重排标签 | 只改变 workspace 顺序，同一 `TabId` 与页面实例不变 | 同左 | 同左 |

`RepositoryPage::suspend` 是本次新增的小接口，仅结束滚动/阅读器手势，供普通标签切换使用；它不是授权失效路径。授权变化仍由 `activate`/`rebind` 的“新 session + 清空数据”语义处理。

页面 host 的读取不发生在 `render`。Shell 只在路由/账户 settle 时创建实例；普通绘制只读取已有实体。Home/Profile 在后台结果到达后经受锁队列和前台轻节拍发布。

## 结果发布与旧响应

每个 host 在请求前固定：

- 服务上下文（账户、host、授权 epoch、selection、服务实例）与 `PageReadSession`；
- workspace `TabId`、路由/目标资源、workspace incarnation；
- 该 section/列自己的请求代次。

应用成功或错误前同时复核服务 `is_current`、session 上下文、登录/canonical 目标和请求代次。任一不匹配都丢弃，错误与成功同等处理。由此覆盖：

- A→B→A 与同路由旧响应；
- PAT 替换、重新验证、切账户；
- 关闭后重开与标签重排；
- 快速切换分支、文件、Home/Feed 列；
- Profile 基本资料返回前后不同的 target 身份。

Profile 不把 hash/login/0 当作稳定数字 id：路由先保留请求 login；只有 `GET /users/{login}` 返回后，才用服务器 `id` 与 canonical `login` 建立 `ProfileTarget`/`ProfileBinding`，并让 Shell 校验 `TabId` 后再更新该会话标签。旧 login 的 UI 状态不会冒充新的数字身份。

## Home 与 Feed

- Home 固定 `involves:<当前认证 login>`、无时间窗口、`updated` 降序。
- Feed 使用服务端顺序和已有 typed event；未知类型保留简短 fallback；不自动轮询。Poll-Interval 只控制刷新按钮何时重新可用，到期不会自行发起读取。
- 页面组件发出的 action 在组件更新栈结束后才交给 host，避免 refresh/navigation 重入正在更新的同一实体。
- 两栏独立加载、错误、Partial、空态和刷新；一栏失败不擦另一栏。
- 刷新按钮由 host 提供可用性。Feed 收到 `X-Poll-Interval` 后在该窗口内禁用刷新，不另建计时轮询。
- 登录前、凭据不可用或离线时显示“无当前数据/不可用”，绝不把 synthetic items 当结果。

## Profile

- 范围沿用已确认首版：基本资料、同名公开仓库根 `README.md`、repository pins（有 gist 时提示）、无任何 pin 时 Popular、host 生成的最近滚动一年贡献区间。
- README 的 `Missing`/`Unavailable`/`NotEligible` 是中性“未提供/无法确认”，不是读取错误；真实网络/权限/解析失败才显示对应错误。
- 每个 section 独立失败，不擦除其他成功结果。
- 头像通过 `app::avatar` 加载并由组件接收已解码图片；失败/加载中保持首字母与固定尺寸。

## Repository

- 复用已验收布局：摘要、Star/Fork 只读计数、Releases/Contributors 紧凑组、页内 Tabs、真实文件树与 README/源码、Issues/PR badge、底部 language bar。
- 两个仓库标签的分支/路径/tree/reader/scroll 相互独立。
- 切分支只取消 Code 相关读取，不清除同仓库 overview；overview 的各端点继续各自独立。
- Star/Fork 没有写 API；本任务只接整页导航，不把未实现的回调报成成功。产物中它们的入口保持既有禁用语义。
- contributor 与 release 的 typed action 只在当前 binding/请求仍匹配时转发；contributor 进入统一 UserProfile 路径，外链只打开 http(s)。

## 导航

`Route::Profile` 仍是本人；新增 `Route::UserProfile { host, login }` 承载他人页面，key 使用 host + 小写 login。相同 alias 复用同一会话，服务器返回后只更新该资源的 canonical 展示名。

Issue/PR 详情、通知、搜索和所有写操作未扩展。未设计路由继续按产品约定留空。

## 滚动

Home、Profile 和 Repository 都拥有自己的局部 viewport、`Motion`/`Bar`/`Bars`。Shell 不会在外层再给它们包一个抢占 wheel 的页面滚动器；该外层 scrolling 现在只用于 Settings/尚未设计的空白页。页面内滚动条仍使用共享 `ui::scroll`，没有第二套几何或 fade。

## 验证与限制

正式集成测试（`src/app/pages/tests.rs`）使用临时 SQLite、内存凭据、可控 transport、真实 `Accounts` 控制器、正式 `Shell` 与真实页面组件，覆盖：

- 登录后侧栏仓库打开真实 RepositoryPage，并保留普通激活状态；
- Home/Feed 两栏真实 page-data 请求与结果；
- 他人 Profile 在服务器身份返回前没有数字 ProfileTarget，返回后 canonical login 更新目标会话；
- 同一资源激活/重排复用同一页实例与导航状态；
- 关闭一个仓库标签不关闭或清空另一个；
- 重新验证推进授权 epoch 后旧页面 host 被丢弃。

真实 Windows 窗口对正式 `target/debug/githux.exe` 已核对启动、Home/Feed 真实数据、侧栏仓库 README/树/分支、Profile 头像/README/pins 与滚动；截图只保留在被忽略目录。未由本任务完成：负责人输入真实 PAT 的完整验收、原生滚轮到贡献图底部、IME、多 DPI/跨屏、触控板，以及 Settings 的最终设计。Settings 仍是过渡页，不能以本轮状态视为最终验收。
