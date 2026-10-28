# 仓库顶部资料组件

更新：2026-10-28。状态：可复用的 Releases / Contributors / Languages 顶部资料组件已实现于 [`src/ui/repository_overview/`](../src/ui/repository_overview/mod.rs)，通过独立 Cargo example 与集成测试装配。它**尚未挂载到正式 Shell 或 repository-browser 页面**，也没有真实 data/auth 接线；产品 binary 的模块树不导入该组件。真实数据接入是后续集成任务。

本主题只维护该组件自身的输入、动作、布局与验证边界。low-level REST 读取与语义仍以 [仓库顶部资料 low-level API](repository-extras-service.md) 为准；滚动外观与输入以 [统一滚动](scrolling.md) 为准，本组件只消费公共 `ui::scroll`，不新增第二套滚动实现。

## 边界与职责

| 位置 | 职责 |
| --- | --- |
| [`src/ui/repository_overview/mod.rs`](../src/ui/repository_overview/mod.rs) | 公共类型：`OverviewBinding`、`ReleasesData`、`SectionState`、`OverviewAction`、`OverviewUpdate`、`RepositoryOverview` |
| [`src/ui/repository_overview/view.rs`](../src/ui/repository_overview/view.rs) | 三段资料的实际渲染、bounded 列表、popover 焦点/关闭、共享滚动条与 wheel 输入 |
| [`src/ui/repository_overview/fixtures.rs`](../src/ui/repository_overview/fixtures.rs) | 仅预览/测试使用的确定性场景，不进入产品模块树 |
| [`examples/repository_overview.rs`](../examples/repository_overview.rs) | `cargo run --example repository_overview` 的独立原生预览 |
| [`tests/repository_overview.rs`](../tests/repository_overview.rs) + `src/ui/repository_overview/tests.rs` | GPUI 测试装配与组件用例 |
| [`src/bin_support/repository_overview_modules.rs`](../src/bin_support/repository_overview_modules.rs) | example/test 的薄 `#[path]` 装配，复用真实组件与公共滚动模块 |

组件不读取凭据、不调用 HTTP、不下载 asset、不打开 URL、不写磁盘缓存、不轮询，也不做 Packages/Environments、Release 管理或仓库 Settings。它只渲染调用方已经取得并校验过的领域观察值。

## 输入 API

宿主为一次仓库/账户/请求范围构造 `OverviewBinding`，并在请求开始前固定它：

```rust
let binding = OverviewBinding::new(account_key, repository_identity, request_generation);
let view = cx.new(|cx| RepositoryOverview::new(binding.clone(), window, cx));
```

`OverviewBinding` 同时包含：

- `account: AccountKey`：稳定账户隔离身份；
- `repository: RepositoryIdentity`：仓库身份；
- `generation: u64`：宿主自己的页面/请求代次。

组件在 `apply_releases` / `apply_contributors` / `apply_languages` / `apply` 中比较完整绑定；不匹配时返回 `false`，不改变当前状态。这阻止旧仓库、旧账户或旧请求代次的结果覆盖新视图。它**不是授权证明**：宿主仍须在发布成功或错误前用同一个 `RepoService::is_current(&context)` 复核服务授权上下文。

三段各自独立：

```rust
pub type ReleasesState = SectionState<ReleasesData, Vec<ExtrasIncompleteReason>>;
pub type ContributorsState = SectionState<ContributorsOutcome, Vec<ExtrasIncompleteReason>>;
pub type LanguagesState = SectionState<LanguagesOutcome, String>;

pub enum SectionState<T, P> {
    Idle,
    Loading,
    Ready(T),
    Partial { value: T, reason: P },
    Error { message: String },
}
```

`apply_latest_release(binding, LatestReleaseState, cx)` 可独立应用 latest endpoint 的 Idle/Loading/Ready/Partial/Error；它覆盖 `ReleasesData` 中用于成组快照的 latest 展示，列表失败也不会抹掉独立 latest。各 `apply_*` 同时校验 outcome 自带的仓库身份，拒绝绑定与 payload 不一致。领域 outcome 自身的 Partial 不会因外层使用 Ready 而丢失。

`ReleasesData` 分开保存 `list: ReleasesOutcome`、`latest: Option<LatestReleaseOutcome>` 和 `latest_error: Option<String>`。latest 使用 GitHub 的 latest 语义，不由客户端排序推断；`None` 只表示 404 可见性歧义，列表仍可显示 Release。单个 section 失败不会清空另外两个。

## 动作 API

组件只发出类型化动作，不自行执行：

```rust
pub enum OverviewAction {
    Refresh { binding: OverviewBinding, section: OverviewSection },
    OpenRelease { binding: OverviewBinding, release_id: u64, tag_name: String, html_url: Option<String> },
    OpenContributor { binding: OverviewBinding, login: Option<String>, html_url: Option<String> },
}
```

宿主通过 `view.on_action(Rc::new(|action, window, cx| { ... }))` 接收。刷新动作携带当前绑定和 section，由宿主决定是否发起请求；`OpenRelease` / `OpenContributor` 只携带 URL 数据，宿主决定是否交给外部浏览器。组件不会自动打开任何 URL 或下载 asset。

## 渲染行为

- **Releases**：显示 GitHub latest 的 tag/name 与 `Draft` / `Pre-release` 标签；latest 404 使用“无可见 latest，不证明仓库没有 Release”的歧义文案；列表显示受服务端 draft/prerelease 可见性约束的观察值。列表最多显示有限行，更多以 `+N more` 提示；Release body 只作为 bounded plain-text popover 的输入，不渲染 Markdown，也不提供发布管理控件。
- **Contributors**：区分 204 `EmptyRepository`、成功空内容和失败；显示 `observed_entries()` 的观察条数，不声称是完整人数；关联用户与匿名贡献者都显示 name/login 首字母（`Avatar`），不显示 email 或 raw JSON，也不下载头像。列表有界，Partial/延迟状态保留。
- **Languages**：显示字节派生的 share，不是 LOC；`total_bytes == 0` 时不推断 `0%` 分布。每个语言行同时有文字名称、share/bytes 标签和 bar，颜色取当前主题的 chart 色板，不替换主题、不使用随机色。超长名称用省略号，列表有界，并注明舍入后不保证总和恰为 100%。
- **布局**：三段位于 TOP 横条，宽窗口并排，窄窗口在顶部区域内换行/堆叠；不建立右侧纵栏。列表行数按窗口宽度收紧（宽窗口 3 行，窄窗口 2 行）；TOP 高度上限为 360px 与窗口高度一半的较小值，堆叠和长文案通过公共滚动 viewport 访问，为下方文件阅读保留空间。溢出内容使用公共 `ui::scroll` 的 `Motion` / `Bar` / `Bars`，没有新滚动代码。
- **Popover**：Release notes 打开为 bounded 本地 popover；面板自身按 viewport 可用空间 clamp 宽高与位置，短/窄窗口下不会伸出窗口外。打开时焦点进入 popover，面板消费 Tab/Shift+Tab 以阻止焦点落到后方 overview 或阅读器，Escape、Close 按钮或点击外部关闭并把焦点还给组件；关闭时取消共享滚动 motion，避免与相邻阅读器竞争。popover 的滚动与相邻模拟阅读器互不影响。

## 独立预览

```sh
cargo run --example repository_overview
```

预览是独立 Cargo example，不是产品启动参数。它用真实组件和 fixture 场景（Ready、Latest 404、Partial、Error、204 empty、Zero languages、Long notes）展示输入/动作，并在下方放一个模拟文件阅读区以核对 TOP 高度和空间保留。预览不连接账户、凭据或网络；`src/app/mod.rs` 不识别预览参数，默认 release binary 也不包含 example 或 preview 控件。

## 验证与边界

`tests/repository_overview.rs` 在 GPUI 测试平台运行 `src/ui/repository_overview/tests.rs`，覆盖：

- 旧仓库、旧账户、旧 generation 的更新被拒绝，当前 section 不被清空；
- 单 section 失败/Partial 不影响另外两段；
- latest 404 歧义且列表仍显示 Release；
- draft/prerelease 标签保留；
- 匿名与大量贡献者、有界行数、email/raw JSON 不显示；
- 204 空仓库与成功空内容、错误分离；
- 零语言不推断 0%、长名称与舍入、有界语言行；
- popover 打开/关闭、焦点返回、Escape 与外部点击、bounded 高度、短窗口内的 viewport 适配与 Tab 焦点保持；
- popover 滚动/关闭不泄漏到相邻模拟阅读器；
- 回调使用当前绑定且每次点击触发一次；
- 桌面与窄窗口下组件保持在 TOP、有界高度、无右侧纵栏，并给阅读器留空间。

已验证（本机 Windows，2026-10-28，提交前工作树）：

- `cargo fmt --all -- --check`
- `cargo check --locked --all-targets`
- `cargo clippy --locked --all-targets -- -D warnings`
- `cargo test --locked`：主 binary 519 项、account 310 项、reader 123 项、repository_overview 47 项，全部通过
- `cargo build --locked --example repository_overview`
- `cargo build --locked`
- `git diff --check`

未执行：真实 Windows 预览窗口的人工视觉/交互验收，以及正式 Shell / repository-browser 中的真实 GitHub 数据挂载。GPUI 测试平台不经过 Windows 原生输入分发，也不替代真实窗口、IME、多 DPI 与鼠标手感验收。真实 data 接线仍需宿主捕获 `RepoService` session/context、调用 low-level extras 读取、在发布前复核 `is_current`，再把结果交给本组件。

## 后续集成

正式仓库页面接入时应：

1. 从仓库页面的 `RepositoryIdentity`、活动账户和页面请求代次构造 `OverviewBinding`；
2. 通过已有 data 服务（或 repository-browser 的 session）发起 Releases/Contributors/Languages 读取，每段独立取消与结算；
3. 在成功与错误发布前复核授权上下文与页面代次；
4. 把 typed outcome 包装成 `SectionState` 交给 `RepositoryOverview::apply`，处理 `Refresh` / `OpenRelease` / `OpenContributor` 动作；
5. 将组件放在仓库页 TOP 区域，而不是右侧纵栏。

本组件不实现上述挂载，也不改变 `src/ui/mod.rs`、`src/app/mod.rs`、`src/workspace/**` 或 repository-browser 的写入范围。
