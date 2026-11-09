# Native Shell 原位迁移：Home/Feed 与 Profile 页面

日期：2026-11-09。任务：`nsh-home-profile`，分支 `migration/nsh-home-profile`，基线
`01decd2c6aa1331d0172d6281875a2bd7649828f`。本文记录本轮实际做了什么、证据在哪、留下了什么。

## 范围与决定

按 [Native Shell 原位替换](native-shell-inplace.md) 执行：**不新建第二个应用/包**，在
`src/ui/home/**`、`src/ui/profile/**`、`src/app/pages/home.rs`、`src/app/pages/home/**`、
`src/app/pages/profile.rs` 原路径上换成 NSH 实现并接真实服务。业务层（auth/content/data/
domain/github/network/storage）零改动；账户、缓存、授权、取消语义沿用。

页面契约按文档实现，没有自带 Window/App/Entity：

- `HomePage::new(PageContext, NavigationSink)`、`ProfilePage::new(PageContext, login, NavigationSink)`。
- 页内状态是 NSH signal，只在页面自己的 reactive root 中创建；`element()` 只读 signal。
- 每个异步 section 一个 `PageSlot`；`poll(&mut, &RuntimeHandle)` 是唯一发布点，
  用 `runtime.sessions()` 复核 ticket。已显示内容和在途回复同等校验（L022）。
- 用户动作入页内队列，`poll` 中执行；render 不重入、不做 I/O。

## Home / Feed

保留的行为：

- 两栏独立加载/错误/Partial/空态/刷新；一栏失败不擦另一栏（各自 slot + signal）。
- Home 只显示 open 项：`HomeView::apply_home` 再过滤一次 `HomeItem::is_open`，防御非符合响应。
- Feed 只保留关注账号的三类动态（数据层策略不变）；合并 PR 卡片署名 PR 作者（`activity.subject`），
  不是点合并的 actor。
- 星标仓库描述仍走 `repository_descriptions` 的既有有界读取，缺失静默（L014）。
- Partial 文案、错误文案、相对时间都沿用原实现（`presentation.rs`、`ui::relative_time`）。
- 历史快照显示 "Saved" 标记与真实抓取时间；网络恢复后不把历史值当成新鲜值。
- Feed 追加：`feed_source`（Network/Historical + collection）、`feed_next_page`、`feed_seen_ids` 保持；
  替换式首屏读取会 `cancel` 追加与富化并清空描述；追加失败保留列表 + 紧凑 warning + 单击重试；
  历史快照缺失下一页时显示 "Page N is not available offline." 且不再自动续读。
- Feed 冷却：`X-Poll-Interval` 到期只恢复刷新按钮，不自行轮询。
- Read More 打开应用内 `Route::PullRequest`（不跳浏览器），有 headless 回归。

与旧实现的两处技术调整：

1. **Feed 列用 `scroll()` + 逐卡渲染，不再用回收式 `virtual_list`。** 在 NSH v0.4.0 上把
   `virtual_list` 放进 `dynamic` 区域后，列表节点与行 cell 没有进入挂载树（同一次构建里
   Home 列的列表正常，Feed 列 `count("id:feed-rows")==0`，而 Partial 提示和列表外层都在），
   换成 `scroll()` 后同一渲染代码立即正确。Feed 的数据上限是 30 天/300 事件，逐卡布局在
   有界范围内；恢复虚拟化需要先解决这个挂载问题，并重新验证“稀疏首屏续读”和“追加保持锚点”。
2. **近底检测用 `watch_scroll` 的真实 `content_size/size/offset`**，另在 `poll` 中处理
   “首屏过滤为空（0 行）”的续读；两者都调用同一个幂等的 `request_more_feed`（L015）。
   不依赖滚动条 extent，也不依赖定时器。

Row 呈现：Home 的 Involved 行消费共享 `ui::work_item_row`（不 fork）。GPUI 版回调带
`Window/App`，NSH 版需要 `Rc<dyn Fn()>` / `Rc<dyn Fn(bool)>`，最小签名写在
`.agents/task/integration-notes.md` §3.3。

## Profile

- 基本资料返回前不伪造数字身份：先按请求 login 读取，拿到服务器 `id` 后才建 `ProfileTarget`
  （host + id）与 `ProfileBinding`（account + epoch），并发出 `PageNavigation::CanonicalProfile`
  供 Shell 改写 TabId 对应的路由。
- README 只在“确认存在且可读”时显示：`Missing`/`Unavailable`/`NotEligible`/404 都是正常可选缺失，
  整段省略（不画空框、不显示 Retry，L014）；真实失败保留一个紧凑 warning + tooltip。
- README 用 `ReaderLayout::Flow` 挂在 Profile 的唯一外层滚动里：没有 280px 固定高度、没有内层
  灰色块或嵌套滚动（L013）。Markdown 解析（`ReaderDocument::from_prepared`）在后台 executor 上完成，
  UI 线程只安装文档并 `poll` 图片桥。
- README 图片沿用既有 `RepositoryImageService` + `CacheStore` 路径：Shell 注入 provider 工厂，
  页面把本次回复捕获的 `CacheAuthorization` + `RepoService` 作为桥的能力令牌；令牌失效即撤权、
  迟到图片丢弃、重新授权不复活旧文档（reader 组件语义）。
- Pinned/Popular 卡片描述最多三行（`number_of_lines(3)`），语言/star/fork 一行；卡片按内容自然长高。
- 贡献热力图：已知 level 用稳定色阶，未知 level 不用服务端颜色字符串猜；缺失/非法日期、字段错误
  在摘要里如实说明；选中日详情来自 `DayDetail`。
- 每个 section 独立 `PageSlot`：任何已显示 section 的 ticket 失效（撤销、账户切换、缓存绑定丢失）
  时整体退休并显示可重试占位，而不是只拦新请求。

Profile 的四个读取函数（在线、缓存、离线回退、`CacheMiss`/`SectionOutcome`）从已验收的 GPUI 页面
原样搬到 `src/app/pages/profile/reads.rs`，只改成 `pub(crate)` 供宿主调用。

## 验证

工具链：全部 Cargo 经 `node scripts/verify.mjs -- cargo ...`（共享 mutex，`CARGO_PROFILE_DEV_DEBUG=0`、
`CARGO_PROFILE_TEST_DEBUG=0`、`CARGO_INCREMENTAL=0`），本任务独立 target。

本 worktree 尚无 `src/app/runtime`、`src/workspace`、`src/ui/foundation`、`src/ui/work_item_row`、
`src/ui/compact_status`、`src/ui/page_layout`（集成作者所有），因此按契约在 **被忽略的**
`.agents/task/probe/` 里用临时 manifest 编译**真实源码** + 同签名适配器；探针不是生产包、不提交。

- `cargo check --lib`（probe）：通过。真实业务层 + 真实 `src/nsh/runtime` + 真实 `src/nsh/reader`
  + 本任务两个页面与全部 UI 源码。
- `src/ui/home/tests.rs`（5 项，headless NSH）：open-only 行与类型化链接动作、Feed 卡片描述/
  Read more 内部目标、Partial/Saved/追加失败脚注、文案与相对时间、合并署名作者。**全部通过**。
- `src/app/pages/home/tests.rs`（5 项，真实 `AccountService`+SQLite+`CacheStore`+脚本化 transport+
  `RuntimeHandle`+headless）：真实数据两栏渲染（含激励作者、描述富化、开放项过滤、关注过滤）、
  Read more 内部 PR 路由、过滤为空的首屏自动续读、账户切换后已显示内容在同一 drain 清空、
  离线刷新回退到 Saved 快照。**全部通过**（见 implementation-result 的 run 记录）。
- `src/ui/profile/tests.rs`（4 项，headless）：header/sections/热力图与 provenance、README 缺失整段省略、
  真实 README 失败保留一个紧凑标记、热力图只报告实际观测。

边界（L002）：以上都是离线 headless/服务层证据，不代表 Windows 原生窗口的视觉、IME、多 DPI、
真实鼠标拖选/复制或真实 GitHub 账号验收；最终视觉由负责人看运行结果。

## 未完成与后续

1. `src/app/pages/profile_acceptance_tests.rs`、`tests/home.rs`、`tests/profile.rs`、
   `examples/home.rs`、`examples/profile.rs` 仍是 GPUI 版本（删除 GPUI 后失效），尚未改写为 NSH。
   Profile 的页面级 compose（真实账户 + 撤销 + 离线）还缺一组像 Home 那样的真实服务回归。
2. Feed 列的虚拟化（见上）。
3. Home 的行级方向键选择未逐项复刻；现在是指针选择 + 每个链接/标题自身可获得键盘焦点（Enter/Space）。
4. 头像与 README 图片依赖 Shell 注入的真实 provider（§3.6）；未注入时页面显示首字母/诚实占位，
   不自行联网。
