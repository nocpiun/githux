# 发布计划共享事实校准

- 日期：2026-10-29；任务 ID：release-plan；执行者：基层 Agent。
- 范围：仅 `docs/README.md`、`docs/product.md`、`docs/architecture.md`、`docs/roadmap.md` 与本日志；不修改源码、依赖、CI/skills、其他主题文档或设计资料。
- 基线：main `730d2292dcd5bb981928341259c971e42ff497b5`，即 [PR #38](https://github.com/nocpiun/githux/pull/38) 的 squash 合并提交。
- 状态：本轮是已授权的共享事实更新，不借文档批准新功能；PR/CI/review/合并结果以 GitHub 交付记录为准。

## 改动与决定

- 将“已确认产品规则”“已合并组件”“正式 Shell 接线”“原生已验证”分开表述，并以实际源码为准：正式 [`ui::page`](../../src/ui/page.rs) 仍是 Home fixture 与 Repository/Profile identifier stubs，不能把独立 examples 或组件内部真实数据读取写成产品页面已接通。
- 校正账户现状：PR #38 已将真实账户/仓库、SQLite 账户目录恢复、自动读取、每账户独立内存 workspace、过渡 Settings › Accounts 和真实头像接入正式 Shell。头像包括圆形裁切、空 mask 防护与 15 秒有限总超时；真实 PAT 和部分原生产品交互仍有未覆盖项。
- 明确 Settings 的负责人新决定：接受过渡账户功能合入不等于最终设置设计验收，最终版本不能直接沿用；后续独立设计与验收。local-only 设置提案未获批准，不写成共享要求。
- 清除旧总览中的“业务未开始、账户未接 UI、账户元数据未落盘、reader 未实现”等过期描述，同时保留真实缺口：业务内容磁盘缓存/离线阅读、README 图片、正式三页面接线、最终 Settings、H1–H4/H7、输入/DPI 与整体验收。
- 对照已合并组件记录当前已确认规则：Home 无时间窗口、按 updated 降序；Feed 服务顺序与手动刷新；Profile 公开同名根 README、repo pins/gist 提示、仅无 pin 才 Popular、最近一年；统一 repository browser + overview，Languages 在底边，Star/Fork 尚无真实写操作。
- 路线图改为当前证据表与依赖顺序，首要是 shell-pages 正式接线；没有为让进度好看而删除首版剩余能力，也没有替负责人选择写操作、隐私保留政策、设置首批项目或缓存参数。

主要维护位置：[文档索引](../README.md)、[产品约定](../product.md)、[架构方案](../architecture.md)、[开发路线图](../roadmap.md)。

## 核验

- `git status`、`git rev-parse HEAD` 与 GitHub merged PR 查询确认当前分支基线为 `730d229`；PR #34/#35/#36/#38 均已合并，PR #37 的独立 overview 交付由统一 PR #35 取代，没有将关闭写成合并。
- 读取 `Cargo.toml`、`src/main.rs`、`src/app/mod.rs`、`src/app/accounts/**`、`src/app/avatar.rs`、`src/ui/avatar.rs`、`src/storage/accounts.rs`、`src/content/mod.rs`、`src/ui/page.rs`、`src/ui/mod.rs` 以及 Home/Profile/Repository/reader/page-data 主题文档，区分真实依赖、账户目录、组件和产品模块树。
- 对本轮修改文件执行本地 Markdown 链接目标/锚点检查与 `git diff --check`；纯文档任务不重复运行 Rust 全量测试，CI 走仓库正常文档 scope，不使用 skip 绕过。

## 后续

- shell-pages 负责正式 production 入口、workspace/adapter 和其主题文档；本任务不占用或修改这些文件。
- Settings 独立设计待确认；私有内容授权失效/账户移除保留政策、缓存参数和 H1–H4/H7 等未决项仍交负责人/主 Agent。
- 本任务没有新增待裁决事项；如后续 review 出现真正需要产品/架构取舍的争议，按工作流永久登记并交协调者。
