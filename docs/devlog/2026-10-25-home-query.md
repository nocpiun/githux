# Home / Feed 查询适配（home-query）

- 日期：2026-10-25（Asia/Shanghai）；任务 ID：home-query；执行者：基层 Agent（派发会话）。
- 分支 `feature/home-query`，base `42291f79bbe73188852bb375fd296623885ce9b8`；delivery=pr，First Release milestone #1。PR 链接创建后补。
- 目标与状态：**实现中 → 待审查**。交付 `src/github/home/**` + `src/domain/home.rs` 的低层、有界、可取消查询适配；不含 UI，不决定首页产品默认值。

## 改动与决定

- 新增 `domain::home`：`Login`（校验，防查询注入）、`DateBound`/`UpdatedWindow`（显式可选 `updated` 窗口）、`HomeQuery`、`HomeItem`/`HomeItemKind`/`HomeRepositoryRef`、`HomeReadiness`/`HomePartialReason`；Feed 的 `FeedActor`/`FeedRepository`/`FeedEventType`/`FeedPayload`/`FeedEvent`/`FeedReadiness`/`FeedPartialReason`/`FeedOutcome`/`FeedFetch`；统一 `HomeError`。
- `github::home::search_involving`：`GET /search/issues`，固定 `involves:<login>` + 可选 `updated:>=`/`updated:<=`，`sort=updated&order=desc`。保留 `total_count`/`incomplete_results`；跨页按稳定 id 去重；本地页/条目预算、服务 1000 上限、`incomplete_results`、后续页失败、不受信/循环/无效 `Link` 均显式 `Partial`；取消是硬错误。`Link` 只取页码，请求 URL 用调用者查询身份重建，链接不能换查询身份。
- `github::home::fetch_received_events`：`GET /users/{login}/received_events`，返回 `etag`/`poll_interval_seconds`，条件请求命中 `FeedFetch::NotModified`；本层不轮询、不带 Cookie/内部端点。常见事件类型保留最小 typed payload，未知类型只留 type 名、不保留整个 payload。
- 复用既有 `GitHubClient`、`execute_bounded`、取消、同源校验与错误/限流/SSO 头；未改 shared 网络或 `Cargo.toml`/`Cargo.lock`。
- 明确未做：Home/Feed UI、活动账户上下文校验（未来 data 接线）、任何产品默认时间窗口、review-requested、未读/浏览历史语义、Feed 卡片过滤与刷新调度。
- 主题文档：[docs/home-query-service.md](../home-query-service.md)。

## 验证

在 `D:\githux-home-query` 执行（`CARGO_BUILD_JOBS=1` / `-j 1`）：

```sh
cargo fmt --all -- --check                 # 通过
cargo check --locked --all-targets         # 通过
cargo clippy --locked --all-targets -- -D warnings   # 通过
cargo test --locked                        # 223 passed, 0 failed（其中 home 34 项）
cargo build --locked                       # 通过
git diff --check / --cached --check        # 通过
```

覆盖：查询转义与固定 `involves`、显式/缺省 `updated` 窗口、issue/PR 区分、跨页去重、`incomplete_results`/1000 上限/本地页预算 `Partial`、后续页失败保留数据、不受信/循环/不改查询身份、空结果非错误、权限/限流、未知事件与缺 payload 不 panic、304/`X-Poll-Interval`/`If-None-Match`（含真实 localhost socket 304 往返）、取消、输入校验。测试只用合成登录名与 mock/localhost，不含真实用户凭据。

**验证边界：** 未对 `api.github.com` 发真实请求，未做真实 PAT 联调；私有仓库、SSO、限流、Events 延迟的真实形状未验证；无 UI、无账户上下文、无缓存。真实 GUI 与账户接线属于后续任务。

## 后续

- 创建 PR、关联 milestone #1、触发 Codex review，按 [githux-pr-review](../../.agents/skills/githux-pr-review/SKILL.md) 等待最新 head 的 CI 与审查。
- 未出现待裁决事项；如出现则永久记录 `arbitration_ever_required=true`/`merge_owner=coordinator` 并停下。
- 合并后由 data/UI 任务接入活动账户、`StaleContext` 与刷新调度。
