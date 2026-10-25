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
cargo test --locked                         # 144 passed / 0 failed（110 → 122 → 125 → 133 → 137 → 138 → 144）
cargo build --locked                        # 通过
git diff --check                            # 通过
```

新增用例覆盖：Authorization 构造与固定头、`GET /user` 身份与 401、两页合并、部分分页失败、SSO 部分结果、限流/Retry-After/不同 403、超时、取消不影响独立请求、换账户/换 PAT/移除账户后丢弃旧响应、后台执行器收敛与并发上限、凭据保存失败不宣称持久化、token 不泄露、真实 Windows Credential Manager 读写删、跨源重定向不转发凭据、多账户与代次、最小 scope 建议。

其中真实 socket 用例通过仓库内的最小 localhost HTTP/1.1 mock 服务（`src/network/test_server.rs`，仅测试编译）驱动生产 `ReqwestTransport`，检查实际收到的请求头；不是只测 mock trait。

## 首轮审查的 7 个缺陷与修复

Codex 首轮审查在 `44e0559` 上给出 7 条意见，全部成立、全部属可直接修复的实现缺陷（不涉及产品取舍），因此按工作流整批修复并重新送审：

| 意见 | 实际缺陷 | 修复 |
| --- | --- | --- |
| P1 `extract_host` 把路径里的 `@` 当 userinfo | `https://attacker.example/https://api.github.com/user` 会被判定为受信，PAT 会发往攻击者 host | 改为结构化 `Authority` 解析，只取 authority 内的 scheme/host；新增回归测试覆盖 7 种 hostile URL |
| P1 替换 PAT 未校验期望账户 | `login()` 对已有账户走 replace，但只按 token 自身身份匹配，账户 A 的“替换”可以被账户 B 的 token 变成新增/替换 B | 新增 `AccountService::replace(expected, pat)`：验证后比对 `expected`，不符返回 `IdentityMismatch` 且不改动任何账户；未知账户返回 `NotFound` |
| P1 重新加入账户复用代次（ABA） | 新记录从 1 开始，移除后用不同 PAT 重新加入，旧的 epoch 捕获又能匹配 | 改为注册表级单调分配器 `next_epoch`，删除 `AuthEpoch::bump/initial`；新增 ABA 与重复验证不重用的测试 |
| P1 过期错误未按代次处理 | `?` 在核对上下文前就把旧 PAT 的 `Unauthorized` 返回给调用方，可能误提示替换新凭据 | 请求结束后对成功与失败都核对 `AuthContext`，过期一律转 `StaleContext` |
| P2 凭据删除失败被吞掉 | `remove()` 忽略 delete 错误仍报告成功，秘密可能残留且失去重试引用 | 改为移除注册表条目后把删除失败作为 `CredentialStore` 错误返回 |
| P2 第 2 页起传输失败丢弃已取数据 | 与“部分结果”契约矛盾 | 传输/解析失败在已有数据时转为 `Partial { PageFailed }`；仅第一页失败是硬错误 |
| P2 取消不覆盖 body 读取 | header 到达后 `response.text().await` 无条件执行，停滞大响应要等到超时 | body 读取也放进 `tokio::select!` 与取消竞争；新增“header 已到、body 停滞时取消”的真实 socket 测试 |

修复后 `cargo test --locked` 全绿，并新增 16 个回归用例。连续 8 次全量运行与 20 次单测重复运行结果一致。

“取消覆盖 body”这个新用例调了两轮才稳定：第一版用固定延时取消，并行负载下偶尔先撞上连接错误；第二版改成服务端写完后用 channel 通知再取消，仍偶发 `Network` 而不是 `Cancelled`，根因是 mock 服务端在**读取请求之前**就写响应，客户端还没写完请求头，取消时 teardown 产生 IO 错误。最终让服务端先读完请求头再写 header + 停住 body，并且传输层在被取消时把 teardown 造成的 IO 错误统一报告为 `Cancelled`（调用方要求停止，这才是有意义的结果）。

## 第二轮审查的 4 个新缺陷与修复

第二轮（head `3594699`）确认上一轮 7 条已解决，同时提出 4 条新意见，全部成立：

| 意见 | 实际缺陷 | 修复 |
| --- | --- | --- |
| P1 自写 URL 解析器与 reqwest 不一致 | `https://evil.example\@api.github.com/user` 被自己的解析器判为受信，但 WHATWG 把 `\` 当路径分隔符，reqwest 实际连 `evil.example` | 直接用 `reqwest::Url` 做信任判定，与真正连接方同一套解析规则；同时要求 scheme/host/port 三者一致（`:8443`、`HTTP`、`//host`、`ftp://` 均不受信） |
| P2 替换的保存与注册表提交未序列化 | 两个并发 `replace` 可交错：存储最终是 B 的 token，注册表却报告 A 的 epoch，之后读取会在 A 的 epoch 下用 B 的 token | `AccountService` 增加 `credential_commit` 互斥锁，把“写凭据 + 提交注册表”作为单个临界区；提交前在锁内重新确认账户仍存在 |
| P2 分页中途取消被降级为部分结果 | 第 2 页取消时若已有数据，被转成 `Ok(Partial)`，调用方会渲染自己已取消的请求结果 | `Cancelled` 保持硬取消，不因已有页而变成 `Partial` |
| P2 `Drop` 可能 join 自己的线程 | 最后一个 `Arc<BackgroundExecutor>` 可能由网络线程上结束的任务释放，`Drop` 里 `join()` 自己会死锁 | 网络线程用 thread-local 标记身份；`Drop` 检测到同线程时只发关闭信号并分离 handle，不 join 自身 |

新增 4 个回归测试。其中“并发替换”最初写成单线程 `join!`，即使去掉锁也能通过（current-thread runtime 不会在临界区内交错），改为 `#[tokio::test(flavor = "multi_thread")]` + 双 spawn + 会在两次 `save` 重叠时报警的 instrumented store 后，去掉锁会稳定失败、保留锁稳定通过，已在本地双向验证。为此在 `[dev-dependencies]` 增加 `tokio` 的 `rt-multi-thread` 特性，生产 runtime 仍是 current-thread。

## 第三轮审查的 5 个新缺陷与修复

第三轮（head `27fc0cd`）确认上两轮意见已解决，新提 5 条，全部成立：

| 意见 | 实际缺陷 | 修复 |
| --- | --- | --- |
| P1 重定向 origin 仍用手写解析器 | `Location: https://evil.example\@api.github.com/steal` 被自己的解析器判为同源，随后把原始 Authorization 重新发给 `evil.example` | `Origin::parse` 改用 `reqwest::Url`，`Location` 用标准 `Url::join` 解析；端口比较用 `port_or_known_default`（`:443` 与无端口等价）。上一轮只修了 `GitHubClient`，漏了 `ReqwestTransport` 内部的同一类解析 |
| P2 `remove` 未与 `replace` 串行 | replace 通过存在性复查后，remove 删旧凭据并移除账户，replace 再写入新 PAT 并返回 `NotFound`，留下无账户引用的孤儿秘密 | `remove` 也取 `credential_commit` 锁，把删除凭据与移除注册表记录纳入同一临界区 |
| P2 切换账户未使旧响应过期 | A → B → A 恢复同一 `{account, epoch}`，切换前发出的请求返回后仍被接受 | `AuthContext` 增加 `selection`（活动账户选择代次），在活动账户真正变化时推进；重复选中同一账户不推进 |
| P2 `shutdown_timeout` 不是宽限期 | Tokio 文档说明 shutdown 开始后 spawned task 一旦 yield 就被丢弃，所谓“5 秒排空”实际立刻杀任务，且 `set_grace` 未被使用 | 改为先拒绝新工作、**继续驱动 runtime** 直到跟踪任务清零或宽限期到期，之后才 `shutdown_timeout`；引入 `BackgroundExecutorHandle` 对派发任务计数，`set_grace` 通过共享值真正生效 |
| P2 组织 scope 把 `user` 当替代 | `user` 只覆盖 profile，仅带 `user` 的 token 拿不到组织成员/项目/团队数据，等于给出错误的最小权限建议 | 组织读取只接受 `read:org`，文案明确 `user` 不能替代 |

新增 8 个回归测试。其中三处做了反向验证（去掉修复后测试必须失败）：重定向 origin 比较（旧手写解析器在 `https://api.github.com:443/login` 上就失败）、`remove`/`replace` 串行化（去掉锁稳定报告 overlap）、以及执行器排空（多次 yield 的短任务能跑完、超出宽限的长任务被丢弃且 shutdown 不等待）。

## 第四轮审查的 5 个新缺陷与修复

第四轮（head `c3d097b`）确认前三轮意见已解决，新提 5 条，全部成立（均为 P2 级细节）：

| 意见 | 实际缺陷 | 修复 |
| --- | --- | --- |
| body 阶段超时被降级为网络错误 | `response.text()` 的错误分支把所有非取消错误都转成 `Network`，body 停滞超时时调用方拿不到 `Timeout` | body 分支也检查 `e.is_timeout()` 并保持 `Timeout` |
| 超时永远报告 0 秒 | `classify_transport` 把 `TransportError::Timeout`（单元变体）固定映射成 `seconds: 0`，而实际配置是 30s | `TransportError::Timeout` 携带配置时长；向上取整到整秒，600ms 报 1s 而不是 0s |
| 等并发许可时不可取消 | `acquire_owned().await` 不参与取消竞争，许可耗尽时已取消的读取会一直排队 | `tokio::select!` 让许可获取也参与取消 |
| 关闭后仍接受 spawn | `BackgroundExecutorHandle::spawn` 不检查 `accepting`，存活到关闭后的 handle 会把任务投给正在销毁的 runtime | 共享接受标志，关闭后 `spawn` 返回 `SpawnError` |
| `HttpRequest` 的 Debug 泄露凭据 | 派生 `Debug` 会把 `Authorization: Bearer <PAT>` 原样打印，绕过 `Pat` 的脱敏 | 手写 `Debug`，脱敏 Authorization / Proxy-Authorization / Cookie |

新增 5 个回归测试。另外发现并修掉一个测试基础设施缺陷：`MockTransport` 在已取消时仍会消费一个脚本响应，导致 `cancelling_one_read_does_not_affect_another` 在并行负载下偶发失败；改为“已取消则不消耗脚本响应 + `select! biased`”，与真实 transport 的行为一致，之后连续 8 次全量运行稳定通过。

## 第五轮审查的 1 个新缺陷与修复

`6238329` 之后，Codex 在 `c3d097b` 的审查线程上追加了一条新 P2 意见（02:23:04），该代码在 `6238329` 中未变，因此仍然成立：

| 意见 | 实际缺陷 | 修复 |
| --- | --- | --- |
| P2 未轮询即 abort 会泄漏 in-flight 计数 | `InFlightGuard` 写在 async block 内部。任务在 runtime 首次轮询前被 abort 时，future 被未轮询地丢弃，守卫从未构造，但计数已经 `+1`；`in_flight()` 永久偏高，关闭时会为一个从未运行的任务白等整个宽限期（默认 5s） | 守卫改为在 `spawn` 之前构造并 move 进 future，未轮询丢弃时同样析构并回落计数 |

回归测试 `aborting_an_unpolled_task_releases_its_in_flight_slot` 把宽限期设为 30s：去掉修复时会在 32s 后失败（已反向验证），保留修复时约 20ms 通过。

### 关于“审查已通过”的判断

本轮我先核对了证据而不是直接采信：审查摘要评论当时仍写着 `has_unresolved_findings: true`，`6238329` 上没有新的 review 记录，但 `c3d097b` 的审查线程在 02:23 追加了上述意见，且它指向的代码在当前 head 仍然存在。其余 5 条旧意见是 `isOutdated=true` 的历史项。因此按“当前 head 的对应审查已闭环”这个门槛，这条必须修完才算通过。

## 第六轮审查的 1 个新缺陷与修复

`3a39276` 之后，Codex 又提了一条 P2，指出的问题直接影响分页完整性：

| 意见 | 实际缺陷 | 修复 |
| --- | --- | --- |
| P2 `Link` 目标里的逗号会破坏分页 | `parse_link_next` 无条件按逗号切分头部。GitHub 的分页 URL 本身可能含逗号（本客户端首请求就带 `affiliation=owner,collaborator,organization_member`），切分后第二段不以 `<` 开头，`?` 直接返回 `None`，`fetch_user_repos` 停在第一页并报告 `Complete`，**静默漏掉后续仓库** | 改为按 RFC 8288 形状解析：只在 `<...>` 目标外、且不在引号参数值内处分段；`rel` 同时接受带引号与不带引号，并按空白分隔的多值判定 `next` |

顺带修掉同一个函数里的相邻缺陷：旧实现用 `strip_prefix('<')?`，任意一段不以 `<` 开头就整体提前返回 `None`；现在非 next 段会被跳过而不是中止扫描。

新增 6 个用例（含 `Link` 解析单测与一条走 `fetch_user_repos` 的端到端用例）。已反向验证：换回旧实现时 `next_link_with_commas_inside_the_target` 与 `pagination_follows_a_next_link_with_commas_in_its_query` 都会失败。

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

- **L007 URL 解析**：安全判定与真正连接方必须用同一个解析器；自写解析器在两轮审查里各暴露一个绕过。
- **L008 并发测试**：并发正确性测试必须真的并发，且反向验证“去掉修复就会失败”。
- **L005 取消令牌**：第一版 `CancellationToken::default()` 只留 receiver，sender 被丢弃后 `changed()` 立刻返回 `Err(Closed)`，被当成“已取消”，导致所有正常请求都报 `Cancelled`（12 个用例失败）。修复是 token 内持有 keepalive sender，并补“未取消时不解析”的超时断言。
- **L006 模块级 allowance**：服务层先于 UI 落地，用写在最内层并注明原因的 `#![allow(dead_code)]`，而不是 crate 级屏蔽。

## 相关文档

- [认证、网络与账户上下文](../auth-and-network.md)
- [架构方案](../architecture.md) · [产品约定：登录与未来写能力](../product.md#登录与未来写能力)
- [GitHub API 能力核验](../github-api-capabilities.md)
