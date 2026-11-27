# Notifications 离线阅读

正式 Notifications 页使用现有账户业务缓存，在首次网络刷新前尝试展示该账户已加载的最近已读条目。文件存储、账户代次、仓库门禁及聚合失效由现有缓存服务管理。缓存投影限制为最近 100 条已读条目、单条 4 KiB 和每账户 256 KiB。不同可见性的仓库共享该私有聚合。任何成员缺少缓存授权时，整个投影不可复用。

验证使用临时账户目录和缓存库、内存凭据、合成响应。正式页面挂载用例覆盖保存条目与两个 typed 路由先于被阻塞的刷新、失败后的历史显示、显式重试和权威空结果。控制器用例覆盖账户隔离、明确撤权、`no-store`、凭据和 SSO 拒绝，以及已完成动作的持久失效。验证范围为模拟传输和 GPUI 测试平台。

Windows 本地通过 `node scripts/verify.mjs -- cargo test --locked --bin githux notifications`（66 项，run `47240635`，构建槽等待约 239 秒，运行约 77 秒），并通过 `node scripts/verify.mjs -- cargo fmt --all` 和 `git diff --check`。
