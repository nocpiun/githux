# Repository Star / Unstar

更新：2026-11-09（Native Shell 原位实现）。状态：真实服务链已接到 `src/ui/repository` 的 NSH
页面；真实 GitHub 写操作保留给负责人明确验收，自动测试不读取真实 PAT，也不 Star/Unstar 真实仓库。

## 范围与 API 证据

本任务是 G3 的第一个真实写动作，只包含仓库页 Star/Unstar。Fork、Create Issue、通知已读与通用
mutation 框架没有借此启用。

实现前只读核对了 [GitHub Starring REST](https://docs.github.com/en/rest/activity/starring?apiVersion=2022-11-28)，
沿用客户端固定的 `X-GitHub-Api-Version: 2022-11-28`：

- `GET /user/starred/{owner}/{repo}`：204 表示当前账户已 star；404 表示未 star，但对不可见仓库也有歧义，
  不能据此宣称仓库消失。
- `PUT` / `DELETE` 同一路径：仅 204 确认对应动作。成功 body 不提供新的 stars count。
- fine-grained token 的状态读取需要 Starring read；写需要 Starring write + Metadata read。Classic PAT
  的实际仓库权限和组织策略仍由服务端判定。
- 403、429/限流、404、超时和连接中断保持类型化错误，不清除账户或推断撤权。

`src/github/starring.rs` 使用 `reqwest::Url` 编码独立 owner/name path segments，并调用既有可信 origin
检查。`src/network/transport.rs` 只增加该动作需要的空 body PUT/DELETE；保留 Authorization redaction、
超时/取消/有界读取、并发限制和同源策略。所有非 GET 请求都不跟随重定向，PUT/DELETE 不会自动重放。

## 数据与确认契约

`src/domain/repository_actions.rs` 把 `StarredState`、`StarObservation`、`StarActionOutcome` 与
`RepositoryActionError` 分开建模。`src/data/repository_actions.rs` 的 `RepositoryActionService` 由页面
`PageContext::repository_session()` 捕获的 `RepositorySession` 和同一 `AccountService` 构造，复核账户
上下文/服务实例，并复用 `RepositorySession::read` → `RepoService::read_authenticated` 的凭据、并发、
取消和旧响应门禁。没有新 PAT、数据库、网络栈或 RepoService 字段。

正常写路径：

1. 以已验证的账户上下文 + canonical repository numeric id + desired starred state 发起一次 PUT/DELETE。
   状态从不由 stars count 推断。
2. 204 给出服务端确认；非 204 不本地翻面。
3. 再读权威仓库摘要并核对 repository id，更新 stars count。没有本地伪 +1/-1。
4. 摘要失败不撤销已确认的 starred state；保留该状态并显示紧凑警告，计数保留最后权威值。

超时或连接中断时结果未知，只发 GET starred 对账：

- GET 204/404 得到实际状态后，仍读取摘要；`StarConfirmation::Reconciled` 区分于普通 204 写确认。
- 对账也失败时返回 `UnknownOutcome`，不自动重放原写请求。用户点击紧凑 warning 的 Retry 也只重新 GET，
  不重新构造 PUT/DELETE。
- 主动取消、账户/授权变化不启动对账或摘要写入；旧成功和旧错误都不能进入新账户页面。

## Native Shell 页面与 UI

`RepositoryController`（`src/ui/repository/mod.rs`）在 canonical repository summary 就绪后通过
`RepositoryActionService` 读取初始状态。Star 有自己的请求代次、取消源与队列，不随分支切换取消。
账户 A→B→A、`retire`、重建页面、切仓库都会推进/清理 Star 状态并取消旧工作；取消不等于撤销已到达
GitHub 的写，也不会在重开时自动重放。

NSH 顶部入口（`repository-star`）：

- 文字为服务端观察到的 `Star` / `Starred`，保留星形图标与数字徽章；数字来自当前有效
  `RepositoryDetails.stars`，0 与大数原样。
- 初始读取和写处理中显示紧凑 loading 标记（`Progressbar` 语义 + tooltip），忙碌时抑制重复点击，
  不做乐观切换。
- 失败使用黄色 warning + tooltip；可点击时只发起新的 GET 对账；不堆长文案或 toast-only 成功。
- Starred 状态与计数都有可访问标签；Fork 使用独立且恒为 false 的能力位，`PageView`/`PageAction`
  中没有 Fork 写路径，不随 Star 启用而启用。
- 显示离线历史快照时不发起初始 `GET /user/starred`，按钮禁用、点击不发请求、不改数；用户通过顶部
  `Refresh` 在线重验后才恢复状态读取与权威写操作。

较早的 repository summary 携带 Star 写代次，不能回盖写后的状态或计数（`star_write_generation`）。

## fixture、预览与验证

`src/bin_support/repository_fixture.rs` 与数据预览 `cargo run --example repository` 共用可重复、按
method/path/account 应答的 fixture。Normal 场景可反复 Star/Unstar；另有初始 starred/unstarred、403、
超时已生效、超时未生效、写及对账均超时的场景。计数只由后续 repository summary 提供；fixture 未建模
请求仍是 599/开发配置诊断，不伪装成权限失败。

自动验证（`src/ui/repository/star_tests.rs`，离线）：

- 初始未 star → 观察到 `Unstarred`、count=128；按一次只发一次 PUT，写中再次点击不重复提交；成功后
  `Starred`、count 来自权威摘要 129；再按一次 DELETE、回到 128。
- 403：状态与计数保持服务端观测值，失败可操作；Retry 只增加 GET、PUT 仍为 1（不重放）。
- 超时未知：写只发一次，失败不宣称成功；重试只发 GET。
- 离线快照：不发 Star 请求、按钮 disabled（`StarPresentation.enabled == false`），`Refresh` 在线后恢复。

边界：以上全部离线确定性验证，不替代真实 Windows 原生输入、tooltip、最终视觉/交互和真实 GitHub 写入
验收；真实写入不在自动验收范围内。
