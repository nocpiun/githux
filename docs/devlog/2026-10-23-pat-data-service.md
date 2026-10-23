# PAT 数据服务：认证、凭据与可取消读取

日期：2026-10-23。任务：`pat-data-service`，分支 `feature/pat-data-service`，基线 `b00a9e3`。交付方式：PR（milestone #1）。

## 目标与取舍

兑现路线图 R2 的首个可复用实现：Classic PAT 验证、Windows 安全凭据存取、活动账户代次、可取消的 `GET /user` 与 `GET /user/repos` 读取服务。它是下一轮 UI 接入的真实服务基础，不是空 trait，也不冒充完整登录产品已经交付。

明确不做：SQLite/账户目录落盘、私有内容缓存政策、UI 接入、H1/H7/GraphQL、自动重试调度。原因分别是另任务确认、本 PR 范围外、需要最小规格。

## 实现要点

- **稳定身份**用 host + 数字账户 ID（`GET /user` 的 `id`），`login` 仅显示。替换 PAT 时先重新验证身份并要求 `AccountKey` 一致，否则 `AccountIdentityMismatch`。
- **授权代次** `AuthEpoch` 在替换/移除时推进；读取前捕获 `AuthContext`，返回后比对，不一致返回 `StaleContext` 并丢弃数据。
- **凭据**只进 `CredentialStore`：Windows 用 `CredWriteW`/`CredReadW`/`CredDeleteW`（generic，local machine），其他平台 `UnsupportedStore` 明确不支持且**无明文回退**。注册表只保存 `CredentialRef`。
- **`Pat`** 无 Debug/Display/Serialize，`expose()` 是 crate 内可见，`Drop` 覆写缓冲区。
- **网络**在独立线程的 Tokio current-thread runtime 上运行，`BackgroundExecutor` 提供 `handle()`、共享 `Semaphore`、`shutdown()`（宽限期内排空，超时取消并 join）。取消用 `watch` + keepalive，只有显式 `cancel()` 才生效。
- **重定向**由自己跟随：只允许同 scheme+host+port，跨源/HTTP 降级/超过 5 跳都不跟随，3xx 原样返回给上层拒绝；分页 `Link` 也要过 `url_is_trusted`。
- **错误分类**区分 401 / 主限流 / 次级限流 / SSO / 普通 403 / 404 / 5xx / 超时 / 断网 / 取消 / 解析失败。403 不会删除 token 或注销账户。
- **部分结果**：中途页失败与 `X-GitHub-SSO: partial-results` 都返回 `ReposReadiness::Partial` + 已取数据，第一页失败才返回错误。
- **最小权限建议**只在 `auth::capability` 里作为建议返回，不作为登录硬门槛；`repo` 含写权限这点写进文案，不把 token 说成只读。
- **写权限**只有占位：所有 `WriteAction` 都是 `Unknown`，读取成功不构成写允许的证据。

## 依赖决策

选择 `reqwest 0.13.5`（`native-tls` + `http2` + `charset` + `system-proxy`，`default-features = false`）与 `tokio 1`。实验结论：

- GPUI 的执行器是 smol 系，不驱动 Tokio future，因此必须在独立 runtime 上跑 reqwest，不能用 GPUI 任务直接轮询。
- reqwest 提供显式 `redirect::Policy` 控制，便于实现“只同源才转发 Authorization”。
- `Cargo.lock` 变更只有新增 38 个包与新增 `base64 0.23.1`（reqwest 需要）；原有依赖版本一个都没改（已逐条比对 HEAD 锁文件）。
- 没用 `blocking` 特性，避免在 GPUI 线程上误用同步接口。

## 验证

```sh
cargo fmt --all -- --check                  # 通过
cargo check --locked --all-targets          # 通过
cargo clippy --locked --all-targets -- -D warnings   # 通过，无警告
cargo test --locked                         # 107 passed / 0 failed
cargo build --locked                        # 通过
git diff --check                            # 通过
```

新增用例覆盖：Authorization 构造与固定头、`GET /user` 身份与 401、两页合并、部分分页失败、SSO 部分结果、限流/Retry-After/不同 403、超时、取消不影响独立请求、换账户/换 PAT/移除账户后丢弃旧响应、后台执行器收敛与并发上限、凭据保存失败不宣称持久化、token 不泄露、真实 Windows Credential Manager 读写删、跨源重定向不转发凭据、多账户与代次、最小 scope 建议。

其中真实 socket 用例通过仓库内的最小 localhost HTTP/1.1 mock 服务（`src/network/test_server.rs`，仅测试编译）驱动生产 `ReqwestTransport`，检查实际收到的请求头；不是只测 mock trait。

## 验证边界（不得省略）

- **真实 GitHub 未联调**：没有读取 `gh auth token`、Cookie 或用户文件，没有向 `api.github.com` 发过请求。真实 PAT 联调要等 UI 显式提供 token。
- **真实 Windows Credential Manager**：只在本机用唯一测试账户项做了 save/load/delete 并自清理；产品级“登录→保存→重启读取”因 UI 未接入未验证。
- **HTTPS/TLS**：本地 mock 是明文 localhost；生产 TLS 由 reqwest + native-tls 承担，CI 不真实连通 GitHub。
- **四平台 CI**：本地 Windows 结果不能推出 Linux/macOS 结果，以 PR 检查为准。非 Windows 平台走 `UnsupportedStore`，编译通过但没有真实凭据后端。
- **UI/GTK 输入**：本任务不涉及真实窗口交互，因此没有 GUI 验收。

## 剩余问题（交接）

1. 账户元数据持久化与重启恢复、SQLite 版本/迁移、私有内容保留政策：另任务确认后再做。
2. UI 接入：登录表单、账户切换、仓库列表、错误与“部分结果”呈现，由后续 UI 任务负责；本任务只提供 `AccountSummary`/`AuthEpoch`/`RepoSummary`/`GitHubError` 与调用示例（见 [认证与网络文档](../auth-and-network.md#8-调用示例给后续-ui-任务)）。
3. 自动重试/退避调度：目前只返回 `RetryAfter`，是否在数据层做有界退避需与缓存策略一起定。
4. 在途请求合并：多页面/多账户同时读同一集合时目前会各发一次请求。
5. 真实组织环境（SAML、禁止 classic PAT 的组织）下的错误形状仍需真实账户验证。

## 过程中形成的教训

实现中真实踩到两个问题，已写入 [LESSONS](../../LESSONS.md)：

- **L005 取消令牌**：第一版 `CancellationToken::default()` 只留 receiver，sender 被丢弃后 `changed()` 立刻返回 `Err(Closed)`，被当成“已取消”，导致所有正常请求都报 `Cancelled`（12 个用例失败）。修复是 token 内持有 keepalive sender，并补“未取消时不解析”的超时断言。
- **L006 模块级 allowance**：服务层先于 UI 落地，用写在最内层并注明原因的 `#![allow(dead_code)]`，而不是 crate 级屏蔽。

## 相关文档

- [认证、网络与账户上下文](../auth-and-network.md)
- [架构方案](../architecture.md) · [产品约定：登录与未来写能力](../product.md#登录与未来写能力)
- [GitHub API 能力核验](../github-api-capabilities.md)
