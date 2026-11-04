# 2026-11-03 开发日志：RepositoryPage 持久缓存与跨重启离线阅读 (repository-offline)

## 任务目标与范围

在正式 `RepositoryPage`（`src/ui/repository/**`、`src/ui/repository_overview/**`）中接上已合并的 `CachedRepositorySession`、`CachedPageReadSession` 与 `RepositoryImageService::with_cache`：
- 保持 `RepositoryPageServices::new(accounts, executor)` 等既有构造兼容；页面不自行打开 `CacheStore`，只从 `RepoService::cache(session.context())` 取得当前已保存账户的 `(CacheStore, CacheScope)` 绑定，无绑定时保留原有在线路径。
- 连接仓库摘要、分支列表、分支 revision、目录树、README 发现、文件正文、overview（Releases/Latest release/Contributors/Languages）与 OPEN Issue/PR 计数缓存路径。
- 明确区分在线最新验证（`Network`）与离线历史快照（`Snapshot`/`HistoricalSnapshot`）：历史快照展示 `Cached` 胶囊（悬停显示同步时间）与手动 `Refresh` 按钮，历史 Releases 不伪造 `Latest` 绿色认证徽章或完整数量胶囊，未缓存目录/文件诚实报 miss 而不编造空内容。
- 将 `RepositoryImageService::with_cache` 通过 `ReaderView::set_image_provider` 挂到 `RepositoryPage` 的真实 `ReaderView` 上（传入可信 `RepositoryDetails` 的数字仓库 ID 与私有属性），复用固定 commit 的解析映射与图片字节缓存，外链继续走无凭据 `ReqwestExternalImages`（详见 [README 图片](../reader-images.md)）。
- 离线历史快照下 Star 不发送状态/写请求、不乐观改数，在线 `Refresh` 后恢复权威读/写；Fork 保持禁用。
- 复核缓存门禁与账户上下文：换 PAT 隔离、凭据拒绝或 `revoke_repository` 立即清空页面已展示的摘要、树、正文、图片 provider 与 overview。

## 关键实现

- **`src/ui/repository/locator_index.rs`（新增）**：
  - `RepositoryPage` 以 `RepositoryLocator`（`owner/name`）打开，而 `CachedRepositorySession` 以数字仓库 ID（`RepositoryIdentity`）做键。新增账户聚合级（`repository = None, private = true`）的 `repository-locator-index` 辅助映射：在线成功解析仓库详情后记录 `locator -> RepositoryIdentity`，跨重启离线激活时先在后台 blocking worker 读取该映射，再调用 `CachedRepositorySession::repository`。
  - 该映射仅作定位提示，不绕过任何仓库门禁或 DTO 数字 ID 校验；账户聚合被隔离或因某仓库明确撤权失效时一并清除。
- **`src/ui/repository/mod.rs` & `view.rs` & `navigation.rs`**：
  - `activate` 捕获 `RepositorySession` 后查询 `RepoService::cache(session.context())` 构建 `CachedRepositorySession`。
  - 当摘要或 revision 返回 `RepositoryCacheOrigin::Snapshot` 时，`current_read_mode()` 自动切换为 `OfflineOnly`，使后续根目录、已展开子目录、README 发现与文件正文直接从本地持久缓存读取，未访问过的目录/文件立即返回诚实的 `PageError::from_cache(&RepositoryCacheError::Miss(CacheMiss::NotCached))`。
  - 顶栏新增 `cached_marker`：在 `is_showing_cached_snapshot()` 为真时渲染 `Cached` 描边胶囊（tooltip 标注同步时间）与 `Refresh` 控件；点击调用 `refresh_repository` 在线重验并在返回 `Network` 后移除历史标记。
  - `apply_content` 创建 `ReaderView` 时调用 `image_provider_for` 注入 `RepositoryImageService::with_cache`，并在 `deactivate` / 权限失效时显式卸下 provider。
  - `check_authorization` 与 `apply` 额外检查 `cache_allowed(repo_id)`，在仓库撤权或缓存分区失效时立即清空页面数据。
- **`src/ui/repository/extras.rs` & `src/ui/repository_overview/**`**：
  - `start_overview` 在有缓存绑定时构建 `CachedPageReadSession`；当页面处于离线快照态时直接以 `PageReadMode::Offline` 读取，在线态优先 `PageReadMode::Online` 并在网络/超时/限流/5xx 时回退 `PageReadMode::Offline`。
  - `SectionState` 新增 `Historical { value, sync_label, reason }`：Releases 与 Contributors 在历史快照下展示 `Cached` 胶囊与 scoped retry 图标，历史快照不渲染 `Latest` 绿色徽章和完整数量胶囊；Languages 底条在历史快照下于 tooltip 中标注 `cached`。

## 应用的教训 (LESSONS)

- **L002（模拟验证写清边界）**：离线测试使用真实 `RepositoryPage`、`AccountService::open_with_cache`、真实 `CacheStore` 与受控传输，不宣称已完成真实 Shell 原生窗口或真实 PAT 验收。
- **L010（fixture 支持反复操作）**：测试中的离线开关和图片分支通过包装原 `FixtureTransport` 按请求身份重复应答，未修改共享 fixture。
- **L012（组件与整页联合验证）**：在同一 `RepositoryPage` 实例内同时验证摘要、分支、树、README 正文、源码、README 图片、overview 与 Star 按钮的离线/刷新行为。
- **L014（可选内容与诚实状态）**：历史快照以紧凑 `Cached` 胶囊与 tooltip 表达，不堆砌长段落；未缓存目录/文件明确报告未缓存，不伪装成空目录。

## 验证记录

所有 Cargo 命令均通过 `node scripts/verify.mjs -- cargo ...` 执行：
- 聚焦离线组合测试 `cargo test --locked --test repository offline`：8 项全部通过（含 `locator_index` 单元测试及 6 项整页 GPUI 离线组合测试：跨重启离线浏览仓库/分支/目录/README/源码与未读目录 miss、手动 `Refresh` 恢复在线 provenance、固定 commit README 图片跨重启离线复用、换 PAT 隔离阻止旧缓存读取、extras 历史快照标记、离线 Star 禁用不发请求且在线刷新后恢复写操作、明确 `revoke_repository` 清空已展示页面并跨重启阻断）。
- `cargo test --locked --test repository` 與 `cargo test --locked --test repository_overview` 全部通过。
- 全量 fmt / check / clippy / build / test 在提交前通过共享 runner 完整核验。

## 剩余依赖与原生验收边界

- `cache-runtime`（应用启动时打开唯一 `CacheStore` 并通过 `AccountService::open_with_cache` 注入正式 Shell）目前仍在等待上游 `sidebar-cache`（PR #70），尚未合入 `main`。因此当前 `main` 的正式二进制启动仍走无缓存 `AccountService::open`，本页面在此状态下保持原有在线路径；待 `cache-runtime` 合入 `main` 并 rebase 后即可在正式 Shell 中端到端启用跨重启离线仓库浏览。
- PR 标记 `pending`，等待负责人进行真实 Windows 原生窗口验收。
