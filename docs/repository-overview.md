# 仓库顶部资料组件

更新：2026-10-28。负责人明确要求按 `02-repository-header.png` 重做为紧凑摘要条；此前三卡片、有界滚动面板和 notes popover 方案已废弃。当前组件位于 `src/ui/repository_overview/**`，独立 example/test 装配，**已内嵌于 repository-browser，使用既有账户作用域 page-data 服务读取；尚未挂载正式 Shell**。负责人将两任务合并由 browser 单写入者交付，PR35 为唯一联合验收 PR；原 PR37 的紧凑实现及历史保留在 browser 分支。

## 已确认的展示

Releases、Contributors、Languages 是一个横向自然高度摘要条，嵌在仓库 identity/description **之后、内容 tabs 之前**。沿用 Ely palette、Noto Sans / Google Sans Code。桌面一行三个小组，只有小标题和摘要；窄窗口按每组约 190px 的内容预算换行。无独立卡框、固定半窗高度、最小高度占位、内部滚动、常驻刷新图标、history、Notes/Open 按钮或 popover。下方 Code 阅读区保持主导。

- Releases：完整 list 观察才在标题显示本次可见 Release 数；Partial 不显示伪精确总数。仅显示 supplied latest 的 version link，保留 draft/prerelease；不从列表排序推断 latest，不重复版本、名称、正文。404 显示 `Latest unavailable`，不是“仓库没有 Release”。宿主可为对应 release ID 提供相对日期；组件不制造时钟或日期。
- Contributors：最多 5 个横向 Ely Avatar。关联用户且有 html_url 才可导航；匿名或无地址者不可点击/Tab 激活。名字置于 tooltip/accessibility，不显示 email/raw JSON、贡献次数、总人数或永久 API caveat。`+N` 只计当前观察中没画出的条目，非仓库总人数。Partial 显示短标识；204 显示 Empty repository，成功空集合是 None returned，错误是 Unavailable · Retry。
- Languages：从 checked u64 字节总和派生比例，只画一个 segmented bar。最多三个 legend 项；超过三种语言时前两种 + 其余字节合并 Other。颜色由名称稳定映射到现有 Ely chart palette。长名称省略，比例独立保留，tooltip/accessibility 带完整标签。零值不伪造 0% 分布，溢出或 total 不一致为 unavailable。无 byte/total/rounding 段落、独立语言进度条。百分比为近似整数，小于 0.5% 用 <1%。
- 每段状态和失败独立，Ready 不带刷新 chrome。失败时原位 Retry 发出该段的 scoped Refresh；不会触发 HTTP、URL opener 或图像下载。

## 复用与自然高度嵌入

`RepositoryOverview::new(binding, window, cx)` 创建 GPUI Entity。宿主提供有界宽度，将 entity 作为正常 `flex_none` 子项嵌入页面头区，让其自行测量高度；不套 `size_full`、固定 height 或另一个右栏。组件不拥有 identity/header/tabs/tree/reader；这些由 repository-browser / 正式宿主组合。示例中的 identity、Code 文本和少量说明仅说明放置位置，不是第二套仓库产品页面。

`OverviewBinding { account: AccountKey, repository: RepositoryIdentity, generation: u64 }` 固定账户、仓库及宿主请求代次。`apply_releases`、`apply_latest_release`、`apply_contributors`、`apply_languages` 和 `apply(OverviewUpdate)` 比较完整 binding，并复核 payload 自带仓库身份。旧绑定或不匹配 payload 返回 false、不修改现有状态。`set_binding` 清空所有段及本地 image/date presentation。宿主仍必须在发布成功/错误前复核同一个服务授权 context；UI binding 不是授权证明。

`SectionState<T, P>`：Idle / Loading / Ready(T) / Partial { value, reason } / Error { message }。ReleasesData 保留 list、可选 latest 和 latest_error。`LatestReleaseState` 可独立应用；显式 latest 更新覆盖成组快照的 latest 展示与错误，list 失败不抹掉它；独立 latest 成功后不再显示旧快照 latest_error 的 Retry。领域 list/contributors 自带的 Partial 与外层 Partial 均保留。

`on_action(OnAction)` 接收：

- Refresh { binding, section }
- OpenRelease { binding, release_id, tag_name, html_url }
- OpenContributor { binding, login, html_url }

version 和 linked avatar 本身是 action 目标，每次激活一次。宿主执行网络或导航；组件不做自动动作。

### Avatar 与日期窄接口

Ely Avatar 已支持实际图像渲染。`set_avatar_image(&binding, user_id, Arc<RenderImage>, cx)` 接收宿主已经取得/解码的图像，不接 URL。组件不会把 domain avatar_url 交给 GPUI 的自动远程加载器，避免另建匿名 HTTP/凭据旁路。未提供图像时用真实 name/login 首字母；仓库没有现成头像预览资产，不伪造照片。后续宿主须提供账户范围内的图像获取与解码（不属于本组件/本 PR）。

`set_release_date_label(&binding, release_id, String, cx)` 接收宿主基于实际时间字段和时钟格式化的简短相对日期，仅对当前展示的 release ID 生效。预览用确定性合成日期，非实时数据。

## 预览、验证边界

唯一完整页联合验收入口：

```sh
cargo run --locked --example repository
```

`RepositoryPage` 拥有 `Entity<RepositoryOverview>`，在详情后启动四个独立 PageReadSession 读取；比较 repository/page session 的完整授权 context，发布与动作再次复核。分支只刷新 Code，不重取 overview；各组 Retry 不影响文件导航。详见 [统一仓库页契约](repository-browser.md)。

`cargo run --locked --example repository_overview` 仅保留为局部开发探针，不再作为独立验收交付或并行页面。

标准 Cargo example，通过 `src/bin_support/repository_overview_modules.rs` 的薄 path 装配复用实际组件。fixtures/场景开关在 example/test，不进入产品 target。场景：正常短值、latest 404 且 list 非空、Partial、单段错误、204、零语言、长名称/300 人/20 种语言。当前没有产品 Shell 挂载；browser 数据接线已实现。

2026-10-28 本机真实 Windows 原生窗口通过 DPI-aware PrintWindow 截取并实际查看 Ready、窄窗、长名称、单段 Error、Latest unavailable、Partial、204、Zero 场景；与任务提供的原始图并排检查布局层级。当前 144 DPI 桌面 Ready 在 identity 和 Code 文本间显示一条紧凑横带，Code 内容空间占主导；窄窗换行，无卡框/固定半窗带。截图保存在任务目录并随交接提供，不把进程响应当视觉证据。鼠标真实点击验证了场景切换；IME、多屏 DPI 切换和原生键盘辅助技术没有完整验收，不能由 GPUI 测试推断。

GPUI 用例覆盖紧凑三组与阅读区空间、自然高度窄布局、一个 version/一个 bar、没有旧 popover/list chrome、绑定和 payload 隔离、linked/anonymous action 归属、每次点击一次、Partial/204/空/零/错误、latest 不从 list 推断、checked language overflow/grouping。删除旧 popover/scroll 相关用例，因为对应交互已按负责人要求移除。

本轮 fmt、locked all-target check、clippy -D warnings、相关/全套 tests、产品与 example build、diff check 的具体结果见本任务开发日志。`blocked` 由负责人管理；新预览准备后才加 pending，不合并。
