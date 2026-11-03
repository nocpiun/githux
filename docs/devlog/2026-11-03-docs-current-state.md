# docs-current-state：校准项目概述、路线图与缓存政策

- 日期：2026-11-03（Asia/Shanghai）；任务 ID：docs-current-state；分支 `docs/current-state`，base main `d217116`。
- 目标与状态：文档校准，待审查。目标是让概述、路线图和缓存政策准确区分“组件/适配器已合并”“正式 Shell 已接线”“原生验收未完成”，并清除过期未决项。不改代码、AGENTS、工作流或其他主题文档。

## 改动与决定

- 核对截止 base main `d217116`（2026-11-03）及已合并 PR：#42 正式页面接线、#54 缓存存储核心、#55 Notifications、#56 Settings Accounts、#57 全局 Issues/PR、#58 README 图片缓存、#59 页面缓存、#60 头像缓存、#61 账户生命周期、#62 仓库缓存、#63 仓库 Star/Unstar。
- `docs/README.md`：更新状态日期与 base；把“当前状态”拆成“正式 Shell 已接线 / 组件已合并但未挂载 / 缓存适配器已合并但应用接线未完成”三层，明确应用离线阅读仍未交付；阅读路径表补充 Settings、Notifications、全局 Issues/PR、缓存核心/生命周期/适配器与 Star 的权威主题链接。
- `docs/product.md`：删除“正式 `ui::page` 仍是 Home fixture 与 identifier stubs”的过期结论，改为 `app::pages` 已挂载 Home/Feed、Profile Overview 与统一 RepositoryPage，并标注 Star 已实现、Fork 未实现；H7 明确只实现 Star/Unstar；从“仍待决定”移除已由 C1–C5 决定的私有内容保留/清理策略，改为链接[缓存策略](../cache-strategy.md#离线授权与清理)。
- `docs/roadmap.md`：更新 base 与日期；证据表把 Repository 行的 Star 从“仅显示计数/按钮禁用”改为真实写路径（PR #63），新增“全局组件已合并但未挂载”行，更新缓存行为“适配器已合并、应用未组装”；“下一步依赖”与 R5/H7 同步。
- `docs/cache-strategy.md`：更新日期；把“正式 Shell、RepoService 与账户事件尚未接入”改为“适配器已合并、正式 Shell 未组装应用级 `CacheStore`”；“待接入”只保留应用组装与页面消费。
- `LESSONS.md`：作为本批唯一归并者，新增 L019（写意图代次拦截写旁旧读取）、L020（组合读取任一依赖 no-store 都失效旧快照）、L021（跨存储安全操作失败需只能拒绝的持久屏障，恢复标记与目录变更同事务）。三者均有已合并 PR 的回归证据，来源为本批已合并任务 devlog 中标记“交协调者评估”的候选。并行集成任务尚无新候选，未等待阻塞。

## 验证

- `git diff --check` 与 `git diff --cached --check` 通过。
- 对本次改动涉及的 Markdown 相对链接做解析检查，全部指向存在的文件；未新增 private path、凭据或原始日志。
- 文档任务不触发 Rust 检查与四平台构建（CI 范围策略按 `docs/**` 与根 Markdown 跳过），按实际 CI/review 结果闭环。
- 未做 Cargo 全量构建；未做人工 UI 验收（无界面改动，不添加 `pending`）。

## 后续

- 本批并行集成（shell-global-pages、cache-runtime、sidebar-cache、repository-offline、home-offline、profile-offline）合并后，需再次核对“组件/适配器已合并”与“正式接线”的边界，届时由协调者或后续任务更新。
- Settings 最终设计、缓存验证参数、H1–H4 与 H7 其余动作仍是未决项，本任务只清理已由 C1–C5 决定的部分，不新增产品承诺。
