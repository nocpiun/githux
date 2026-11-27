# Repository Pull requests：离线重开已读页（repo-pr-offline）

- 日期：2026-11-27。任务 `repo-pr-offline`，分支 `feat/repo-pr-offline`，基线 `main@322b3cb`。目标：关闭 #112 留下的阅读路径缺口——仓库 `Pull requests` Tab 在重启断网后仍能从同一仓库、同一筛选的列表找回读者此前翻到过的 PR，包括只在第二页出现的 PR。
- 状态：实现与离线证据完成，待 PR 审查；真实窗口与真实账户验收未做。

## 触发问题

`docs/devlog/2026-11-26-repo-pr-list.md` 明确记录 PR 列表没有持久缓存。仓库 Issues 列表（#112 基线）已有“首批快照 + 离线首屏 + Cached/Retry”的成熟路径；PR 列表则每次打开都访问 GitHub。重启断网后，即使 PR Conversation 已缓存，读者也无法从仓库 PR 列表定位到该 PR。

只缓存第一页不足以修复验收场景：仅出现在第二页的 PR 仍会丢失。任务要求在有界范围内保留同一首屏代次下**实际成功翻过的连续页**，换首屏/筛选不得串代，离线不续拉未知页。

## 实现

| 文件 | 内容 |
| --- | --- |
| `src/data/page_reads/repository_pulls.rs`（新增） | `PageReadSession::repository_pulls`：与 `repository_issues` 同形的受控页面读取入口 |
| `src/data/page_cache/repository_pulls.rs`（新增） | 首屏 `repository-pulls` 与续页 `repository-pulls-page` 快照；首屏写入 collection generation；续页按 `collection.suffix()` 绑定；离线恢复循环 |
| `src/data/page_cache.rs`、`src/data/page_reads.rs` | 各一行 `mod repository_pulls;`，复用既有缓存机制 |
| `src/ui/repository_pulls.rs` | `PullListError`（Read/Cache/Context）；请求携带 `collection`；`restore()` 返回存储页与存储代次；`apply_cached` 只在替换请求仍拥有列表时发布；成功替换安装自己的代次；离线/失败/撤权语义 |
| `src/ui/repository/pulls/mod.rs` | 首次替换并行启动磁盘恢复与网络读取；恢复结果与网络结果分开 drain；旧代次恢复不得覆盖网络结果，网络成功即丢弃更慢的恢复；`Cached` 徽标与 Retry 只在失败后出现 |
| `src/ui/repository/pulls/view.rs` | 失败且为历史前缀时显示 `Cached`；历史前缀不提供 Load more；历史空前缀显示 “No cached pull requests”；Partial 徽标包含“有未读续页”的存储前缀 |
| `src/ui/repository/pulls/pull_offline_tests.rs`（新增） | 真正退出重启的双 GPUI 上下文集成测试与两个筛选用例 |
| `README.md` | 更正页面内容段：Home/Feed 标题入口、仓库 Issue/PR 列表、Conversation/Files changed、Notifications、全局列表、Settings，以及已验证的离线路径 |

缓存身份沿用既有 `CachedPageReadSession`：账户分区 + 数字仓库 id + owner/name + 筛选 + 页大小 + 响应上限 + 表示版本。请求集合由首屏代次绑定，首屏替换（刷新、筛选切换）后旧续页不可再读。恢复预算为最多 4 页 / 200 行，避免把列表当完整镜像；partial 页的续链不被信任，恢复在该页停止。`no-store`、撤权、凭据拒绝、原子发布与 lease 校验全部复用缓存核心，没有新增缓存 schema。

页面行为：

1. 打开筛选时若模型为空，同时启动“磁盘恢复”和“在线替换”。恢复可以先画出页 1 + 已翻页；网络仍未结束时**不**显示 `Cached`，也不提供 Retry。
2. 网络失败且恢复内容仍在：保留行、标 `Cached`、给一个 Retry；Retry 是替换读取，成功后原位替换旧行并清除标记。
3. 网络成功：安装网络首屏的 collection generation；旧恢复或旧续页结果被代次校验拒绝，不能再拼接。
4. 只有成功替换才安装新代次；撤权/凭据拒绝清空行、代次与历史标记；筛选切换丢弃旧行并开始新代次。
5. 历史前缀没有 Load more：其续页虽已保存到恢复边界，但不能再向未读页续拉；离线不产生额外请求。

## 验证

| 轮次 | 命令 | 结果 |
| --- | --- | --- |
| 定向模型/GitHub/数据/缓存 | `node scripts/verify.mjs -- cargo test --locked --bins repository_pulls` | 40 passed |
| 仓库 PR 页面接线 | `node scripts/verify.mjs -- cargo test --locked --bins repository::pulls` | 通过 |
| 真正重启离线集成 | `node scripts/verify.mjs -- cargo test --locked --bins pull_offline` | 2 passed |
| 静态 | `node scripts/verify.mjs -- cargo clippy --locked --all-targets -- -D warnings` | 通过 |

重启集成测试的冷阶段在线读取 PR 页 1 与页 2（#10 只在页 2），关闭账户服务并 `quit` 整个 GPUI 上下文；热阶段在同一目录重新 `CacheStore::open`（成功即证明旧独占租约已释放），断网并暂扣 `/pulls` 请求。断言：

- 网络仍挂起时 3 行（含 #10）已从磁盘恢复，`Cached`/Retry 都未出现；
- 恢复请求只包含 `page=1`/`page=2`，没有未读页；
- 放开后网络失败，才出现 `Cached` 与 Retry，行仍可用，点击 #10 仍产生带验证身份的 `OpenPullRequest`；
- 另一用例冷阶段读 Open 与 Closed，重启断网后两个筛选各自恢复自己的列表，互不串联。

## 边界与未覆盖

- 真实 Windows 窗口、真实 PAT、限流、半页滚动、中文 IME、多 DPI 未复测；按负责人决定在整批交付后统一视觉验收。
- 恢复是“上次读到的有界前缀”，不是完整仓库镜像：未读页不预取，离线不续拉，超过 4 页/200 行的已读页不再恢复。
- `Files changed` 的离线读取、远程写操作与搜索不在本列表范围，也没有离线承诺。
- 真实磁盘重启已由集成测试用新 `CacheStore` 验证；同一进程内“新 AccountService 复用旧 store”不算数，测试没有把它当作重启证据。
