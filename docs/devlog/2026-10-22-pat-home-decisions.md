# 确认 PAT 登录与 Home/Feed 的产品方向

负责人在 API 能力调研 PR #6 合并后确认以下选择；本次只更新设计文档，没有接入真实认证或改动 UI。

- 首版使用 PAT 正式登录，不建设服务器托管 client_secret。为覆盖保留的 Notifications，落实为 Classic PAT；OAuth 优先候选已被产品决定取代，但不删除其接口研究事实。
- 当前先实现阅读，同时为未来写动作保留权限判断、提交结果、失败处理与缓存失效边界。具体 H7 动作、最小权限引导仍待细化，不因 token 有写权限就自动增加功能或预索取所有管理权限。
- 侧栏采用当前账户和 token 授权下 `/user/repos` 的完整分页可见集合。
- 首页左栏标题严格为 Home，展示最近参与且有更新的 Issues/PRs；右栏标题严格为 Feed，展示关注动态，接受 Events API 延迟。Home 的时间窗口、额外参与关系和 Feed 的卡片过滤尚待具体规格。
- gist-only 的 Pinned/Popular 回退和个人 README 遗留规则延后到对应 Profile 任务。

主要产品事实维护在 [product.md](../product.md)，架构更新 [认证与外部边界](../architecture.md#认证与外部边界)，路线图和 [API 能力表](../github-api-capabilities.md) 同步清除已解决的选择项。核对范围是文档一致性、相对链接/锚点和 `git diff --check`；没有 Rust/UI 变更，不运行应用编译或实机测试。
