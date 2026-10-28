# Profile Overview 展示组件

更新：2026-10-28。已实现可复用的只读 Profile Overview，位于 [`src/ui/profile/`](../src/ui/profile/)。独立开发预览为 `examples/profile.rs`，测试装配为 `tests/profile.rs` 与 `src/bin_support/profile_modules.rs`。**尚未接入 Shell、路由、workspace、账户服务或真实页面数据；预览中的合成数据不表示生产集成完成。**

## 已实现的展示范围

顺序固定为：

1. 公开基本资料：显示实际提供的 name/login/bio、公开 company/location/website、followers/following/public repos/public gists/created_at。空字符串与缺失字段不显示；name 缺失时回退 login，头像只显示首字母/字素，**不请求 avatar_url，也不推断私有资料**。
2. 个人 README：消费 host 准备的 `PreparedContent`，直接复用现有 `ReaderView` 的 Markdown、选择、链接与有界滚动行为。显示区域限高 280px，长正文不会挤掉后续区块。
3. Pinned/Popular repositories：保持服务顺序，显示公开仓库摘要。没有 star 排序、自动导航或自动网络调用。
4. 贡献热力图：消费明确 `ContributionRange` 与 `ContributionCalendar`，展示服务总计和真实 day/count tooltip。键盘聚焦后方向键/Home/End 与 hover 都更新同一天数据说明；左右移动一周、上下移动一天，键盘操作同时滚动显示选中的周；长区间使用共享横向滚动。

不包含 Profile 其他 Tabs、Contribution Activity、gist 浏览、写操作、图片下载、磁盘/离线缓存、新主题或另一套 inline-code/Markdown 渲染器。

## Host adapter / mounting 职责

正式集成任务必须把组件声明进自己的 `ui` 模块树（本任务没有改 `src/ui/mod.rs`），并在 Profile 页面会话中持有 `Entity<ProfileView>`：

```rust
let view = cx.new(|cx| ProfileView::new(target, binding, range, window, cx));
let request = view.read(cx).request();
// 后台读取完成后，先复核服务上下文和页面请求，再发布成功或错误。
view.update(cx, |view, cx| {
    view.update_basic(&request, basic_state, cx);
    view.update_repositories(&request, repositories_state, cx);
    view.update_contributions(&request, &range, contributions_state, cx);
});
view.update_in(cx, |view, window, cx| {
    view.update_readme(&request, readme_state, window, cx);
});
```

- `ProfileTarget { host, id }` 是**被查看用户**的稳定资源身份，不是当前登录账户。`ProfileBinding { account, epoch }` 来自 host 捕获的授权/服务上下文。不能把 login 当授权身份，也不能用固定 SHA 代替服务上下文校验。
- 每个独立页面会话从 `request()` 取得 `ProfileRequest`，将它随基本资料、仓库、README 与贡献读取一起捕获。后台结果先经现有 page-data/服务 fresh-context 核验，再调用窄 `update_*`。组件再检查 target/binding/generation；贡献更新还必须匹配组件当前 range。返回 `false` 表示旧结果已丢弃。
- 切 Profile、切账户、授权 epoch/服务 binding 变化或改贡献区间时调用 `bind(target, binding, range)`，取得新 request。它清空所有旧 section、Reader、焦点动作与滚动状态；同一 login 的另一位用户、A→B→A 与旧错误响应都不能复用旧显示。独立 section 的 Loading/Error/Ready 可以分别更新，不互相清空。
- **README 发现属于 page-data/host：** 已合入的 [`PageReadSession::profile_readme`](page-data-service.md#个人-readme-helper) 可供后续 adapter 调用；尝试该用户同名 **PUBLIC** 仓库根 README，读取固定内容并准备 `PreparedContent` 后供给组件。旧版 Share-to-profile 标记当前 API 不可观察，组件不宣称与网站完全相同的资格判定。未发现、不可见、 denied 与真实空文件必须保留服务语义，不能猜测。
- 组件不选择默认日期。未来 host 按已批准产品策略提供**最近滚动一年**的 `ContributionRange`；本组件不在内部读取时钟或另算默认区间。
- `update_readme_with_context` 只用于没有服务 `PreparedContent` 身份的独立 fixture；正式 adapter 使用 `update_readme`，保留 `ReaderView::from_prepared` 的固定 repository/commit/path 相对链接上下文。
- 通过 `on_action` 接收 `ProfileAction`。每个动作携带产生它的 request 和类型化 `ProfileActionKind::Profile / Repository(RepositoryTarget) / ExternalLink / Readme(ReaderAction)`。host 决定是否打开 Profile、仓库或外链；仓库动作保留已有 `databaseId`（API 允许缺失）与 nameWithOwner，不发明身份，不自动调用系统浏览器。
- Reader 的仓库相对链接仍是原有 `ReaderAction`，由 host 处理。头像 URL 不交给 loader；组件无 PAT、HTTP、storage 或 cache 依赖。

## Pinned / Popular 策略

只显示 repository pins；实际观察到 gist pins 时给出简短提示，不增加 gist 功能。`PinnedRepositories` 使用专用 repository-pin 连接的顺序；部分 GraphQL 结果中该连接为空而 showcase 仍有类型化仓库时，可展示那些已观察仓库，并显示 partial 提示。

| 服务状态 | 展示 |
| --- | --- |
| `has_pinned_items=false` + `PopularRepositories` | 标题为 Popular repositories，保留 GitHub 返回顺序 |
| 完整 repository pins | 标题为 Pinned repositories |
| mixed pins | 只显示仓库，提示观察到的 gist pin 数 |
| gist-only | 显示 gist 提示；没有 repository pins 不等于用户没有仓库，**不转 Popular** |
| truncated | 只显示已观察前缀并明确 truncated，**不据前缀决定 gist-only/mixed，不转 Popular** |
| unclassified / 不一致状态 | 只显示可用类型化仓库，保留不确定性，**不发明 Popular** |
| `errors` 与数据并存 | 保留可用数据并显示 Partial data；不冒充完整结果 |
| 服务错误/denied | 独立错误面板，不显示成功空列表 |

## Contribution 语义

- 日期必须落在 host 明确区间内；范围生成与闰年/年边界经过验证，遵守领域 `MAX_DAYS=366` 的起止差上限（包含首尾时最多 367 天）。非法/重复日期、范围外记录不会被并入其他日期。
- 服务返回的 `count=0` 是真实零贡献；缺失 day 是 Missing，不显示为零。GraphQL field errors、缺失日期、无效/范围外记录都有 partial 说明。
- 已知 `NONE` 与四个 quartile 使用固定展示 ramp；未知 level 显示独立未知色，保留实际 count 与 level unavailable 文本，**不用未知 level 推断贡献数，也不把服务 color 字符串当 CSS**。
- 880×560 下完整一年保持自然 11px cell 与间距；更窄窗口有界横向滚动，不压缩全年 cell。页面纵向与热力图横向分别使用共享 `Motion`/`Bar`/`Bars`。

## 独立预览与产品隔离

```sh
cargo run --example profile
```

预览提供 Complete、Partial + long、Mixed pins、Gists only、Truncated、Unknown levels、Missing fields、Errors、Real empty 等合成场景。窗口默认 1000×760，最小 880×560；底部只报告 typed action，不自动导航。场景选择器、fixture 与 launcher 仅在 example/test assembly，产品 binary 的模块树不含它们，没有生产 debug flag。

正式 Shell 挂载、真实 page-data 接线和系统导航仍是后续集成任务。GPUI 测试平台不等同 Windows 原生输入/视觉验收；真实窗口中的 README 选择/滚动、键盘焦点、长元数据、tooltip、触控板、多 DPI 与中文/emoji 字体回退需负责人验收。

## 验证

覆盖见 `src/ui/profile/tests.rs` 与 `heatmap.rs`：binding/account/target 变化与旧更新、部分 GraphQL 数据/错误、缺失基本字段、pin 顺序、gist-only/mixed/truncated/unclassified、真实空与 denied、Reader 链接、typed repository action、闰年/跨年、zero/missing/unknown levels、键盘/鼠标 day detail、长 README/元数据与有界横向范围。实际完整检查与本机窗口边界见 [开发日志](devlog/2026-10-28-profile-view.md)。
