# 仓库顶部资料组件

更新：2026-11-15。负责人明确要求按 `02-repository-header.png` 重做为紧凑摘要条；此前三卡片、有界滚动面板和 notes popover 方案已废弃。当前组件位于 `src/ui/repository_overview/**`，已内嵌于 repository-browser 并挂载正式 Shell 的 Repository 页面，使用既有账户作用域 page-data 服务读取。2026-11-15 的 `repository-summary-polish` 让空 Releases 摘要整段省略，并把语言条改为 GitHub Linguist 真实语言色。

## 已确认的展示

Releases、Contributors 是一个横向自然高度摘要条，嵌在仓库 identity/description **之后、内容 tabs 之前**。沿用 Ely palette、Noto Sans / Google Sans Code。桌面两个小组按内容从左紧凑排列，组间约32px，取消等宽flex-grow；长版本组宽度有界，窄窗口自然换行。无独立卡框、固定半窗高度、最小高度占位、内部滚动、常驻刷新图标、history、Notes/Open 按钮或 popover。下方 Code 阅读区保持主导。

- Releases：完整 list 观察才在标题显示本次可见 Release 数；Partial 不显示伪精确总数。仅显示 supplied latest 的 Tag图标+version link，稳定可信latest旁为公共Success Outline胶囊Latest，保留 draft/prerelease且不冒充稳定Latest；不从列表排序推断 latest，不重复版本、名称、正文。latest 成功返回 404 时显示中性 `No latest release`，latest 读取失败时只显示 warning 图标+Retry，不把 404 当成失败。宿主可为对应 release ID 提供相对日期；组件不制造时钟或日期。
- **空 Releases 整段省略**：只要拿到一份**成功、完整（非 Partial）、列表为空**的 `Releases` 观察，整个 Releases 块（标题、0 计数胶囊、行、加载标记与组间距）就不渲染，`render` 不追加该子项，兄弟组直接从条带左沿开始。判据只看 list，不看 latest 是否已落定、不看来源：GitHub 的 `/releases` 本身已包含 draft 与 prerelease，完整空页不可能再有 `/releases/latest`，所以缓存首帧（`SectionState::Historical`）、后台重校验中（latest 仍 `Loading`）、离线回退（latest 变为历史空值或 Error）都不应画出 `Cached`/spinner/warning/retry 后再消失，也不能先占位后跳变。仍然保留块的情形：完全没有 list 观察（Idle、首次 Loading、首次 Error）；Partial；`data.list.releases` 非空（含仅 draft/prerelease，其 latest 为 `None` 但要看计数/标签/`Cached`/重试）；以及 list 说空而 latest 明确带回一个 release 的矛盾观察（不静默丢掉屏幕上唯一的 release）。`RepositoryOverview::releases_confirmed_empty` 是唯一判定点；数据、状态归属与重试仍全部留在宿主。仓库导航中的 Releases tab/页面不受影响。
- Contributors：最多 5 个横向 Ely Avatar。关联用户且有 html_url 才可导航；匿名或无地址者不可点击/Tab 激活。名字置于 tooltip/accessibility，不显示 email/raw JSON、贡献次数、总人数或永久 API caveat。`+N` 只计当前观察中没画出的条目，非仓库总人数。Partial 显示短标识；204 显示 Empty repository，成功空集合是 None returned，错误是 Unavailable · Retry。
- Languages：数据仍归同一个 RepositoryOverview state；`language_bar(cx)` 小接口由RepositoryPage在页面最底边渲染4px纯色条。顶部不再保留标题/图例/百分比或第三列占位。checked字节总和/可信total，按每种实际语言各一个比例色块，不合并Other、没有伪最小宽度；hover完整名称与比例使用Ely tooltip，零/未知/error不画条也不画文本。内部作用域重试保留，无常驻Refresh/Retry。色块用 GitHub 自己的语言色：`language_colors::linguist_color(name)` 查询固定版本的 Linguist 静态表（源码、修订、SHA-256 与许可证见[第三方资产](third-party-assets.md#41-github-linguist-语言颜色表)），不再用名字哈希取 `theme.colors.chart[i]`。Linguist 没有给颜色的语言（如 COBOL）与表中不存在的名字一律用主题中性色 `fg_subtle` 作回退，不自行推算色值；排序、字节比例、tooltip、溢出裁切与无障碍名称都不因换色改变，运行时无网络查询。
- 每段状态和失败独立，Ready 不带刷新 chrome。Releases/Contributors 失败时原位 Retry 发出该段的 scoped Refresh；Languages 只保留内部真实失败与 Refresh 能力，无常驻文案；不会触发 HTTP、URL opener 或图像下载。
- **缓存提示（2026-11-18 返工）**：历史来源与当前请求失败分开保存。正常缓存首帧及后台验证静默；只有本次对应请求失败且保留可用旧值的区域显示 Cached 和分组刷新。Releases 的 list/latest 独立记录，任一失败不能把成功兄弟分区标为缓存；Contributors、Languages 同样局部处理。真实无值失败提供 Retry、不报 Cached；完整空 Releases 仍整组隐藏。重试保留内容、清除旧提示，成功静默，再失败可继续重试；换绑定清除失败记录。宿主始终在发布前校验账户/请求/授权，来源 provenance 不因静默 UI 改成 fresh。
- 后续 Packages、Environments 等摘要块沿用同一规则：只在成功且完整地确认“没有内容”时整段省略（标题与间距一并省略），包括缓存/历史快照首帧，不能先画空壳再随刷新消失；未知/加载/失败/部分结果与非空快照继续显示各自的加载、重试或新鲜度入口。本轮只实现 Releases；Packages/Environments 本身仍未实现，本节不是实现授权。

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

标准 Cargo example，通过 `src/bin_support/repository_overview_modules.rs` 的薄 path 装配复用实际组件。fixtures/场景开关在 example/test，不进入产品 target。场景：正常短值、latest 404 且 list 非空、完整空 list（`No releases`，整段省略）、Partial、单段错误、204、零语言、长名称/300 人/20 种语言；语言色用真实 Linguist 名称（含无色 COBOL 与未知名回退）。组件已随 Repository 页面挂载正式 Shell。

2026-10-28 本机真实 Windows 原生窗口通过 DPI-aware PrintWindow 截取并实际查看 Ready、窄窗、长名称、单段 Error、Latest unavailable、Partial、204、Zero 场景；与任务提供的原始图并排检查布局层级。当前 144 DPI 桌面 Ready 在 identity 和 Code 文本间显示一条紧凑横带，Code 内容空间占主导；窄窗换行，无卡框/固定半窗带。截图保存在任务目录并随交接提供，不把进程响应当视觉证据。鼠标真实点击验证了场景切换；IME、多屏 DPI 切换和原生键盘辅助技术没有完整验收，不能由 GPUI 测试推断。

GPUI 用例覆盖顶部两组、底边语言条与阅读区空间、自然高度窄布局、顶部一个 version，页面底边一个 bar、没有旧 popover/list chrome、绑定和 payload 隔离、linked/anonymous action 归属、每次点击一次、Partial/204/空/零/错误、latest 不从 list 推断、checked language overflow/逐语言identity。删除旧 popover/scroll 相关用例，因为对应交互已按负责人要求移除。

本轮 fmt、locked all-target check、clippy -D warnings、相关/全套 tests、产品与 example build、diff check 的具体结果见本任务开发日志。`blocked` 由负责人管理；新预览准备后才加 pending，不合并。

2026-11-15 `repository-summary-polish`：空 Releases 整段省略与 GitHub Linguist 语言色的契约见上文；GPUI 用例新增 `confirmed_empty_repository_omits_the_whole_releases_block`、`cached_empty_releases_never_paints_the_block_before_or_during_revalidation`、`cached_non_empty_releases_keep_their_block_and_cached_mark`、`conflicting_empty_list_and_supplied_latest_keep_the_block`、`no_latest_stable_release_is_not_a_repository_without_releases`、`unknown_loading_failed_and_partial_releases_keep_their_block`、`language_colors_come_from_the_pinned_linguist_table`、`colorless_and_unknown_languages_still_draw_one_segment_each`；真实 `RepositoryPage` 层的 `cached_complete_empty_releases_never_paint_the_group` 用 fixture 的零 Release 仓库验证缓存首帧、网络挂起中与刷新后都不画该组。真实 Windows 窗口与完整仓库页的视觉验收仍由负责人在 `--release` 构建上确认（`cargo build --locked --release --bin githux` 后运行），本轮不以 GPUI 测试代替。


公共Badge来自`ui::badge`（见browser主题文档的API/上游归属）；Release完整观察数量采用Neutral Filled Pill，Partial/未知不伪总数；完整且成功确认的空 list 则整块省略而不是画 `0`。Latest不由列表第一条/版本/日期推断，独立latest状态优先；404/Loading/error/空/旧binding无Latest，独立恢复不会遗留旧错误。它是静态标签，不额外成为按钮或焦点。


后续联合页的Star/Fork数量由browser有效RepositoryDetails提供，胶囊复用公共Badge，仍仅typed宿主动作。README留白修订与overview自然高度/底边language状态独立；本轮没有改变overview数据请求或来源。当前最新契约见browser与content-reader主题，旧无数字说明仅为历史。
