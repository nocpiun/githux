# Notifications 离线阅读

正式 Notifications 页使用现有账户业务缓存，在首次网络刷新前尝试展示该账户最近已加载的条目。文件存储、账户代次、仓库门禁及聚合失效由现有缓存服务管理。缓存投影限制为最近 100 条、单条 4 KiB 和每账户 256 KiB。不同可见性的仓库共享该私有聚合。任何成员缺少缓存授权时，整个投影不可复用。

验证使用临时账户目录和缓存库、内存凭据、合成响应。正式页面挂载用例覆盖保存条目与两个 typed 路由先于被阻塞的刷新、失败后的历史显示、显式重试和权威空结果。控制器用例覆盖账户隔离、明确撤权、`no-store`、凭据和 SSO 拒绝，以及已完成动作的持久失效。验证范围为模拟传输和 GPUI 测试平台。

合并前协调者明确了保留范围。投影包含已获取的通知，保留上次观察的 `unread` 状态。正式挂载回归使用原 `all=false` 查询与全未读的 Issue/PR 响应，核对两个目标路由、被动呈现没有写请求、首次刷新和 Retry 期间保留行且隐藏历史标记、短暂失败后显示历史标记与 Retry，以及显式 Retry 后的权威空结果。

Windows 本地通过 `node scripts/verify.mjs -- cargo test --locked --bin githux -- notifications issue_detail_offline`（67 项，run `fb72235d`，构建槽等待约 521 秒，运行约 80 秒），并通过 `node scripts/verify.mjs -- cargo fmt --all` 和 `git diff --check`。
