# R4 仓库顶部资料独立 API

任务：`repository-extras`；基线 `42291f79bbe73188852bb375fd296623885ce9b8`；分支 `feature/repository-extras`。

## 实现

新增 `github::repository_extras` 的四个只读方法：Releases 完整有界分页、latest release、含匿名贡献者的 Contributors 分页、Languages 字节统计与派生比例。领域 outcome 都保留仓库身份；Release/Contributor 同时保留完整 raw JSON，assets 只作为字段不下载。无 Packages/Environments、管理写操作、新依赖、网络/Cargo/data/UI 改动。

`RepositoryExtrasLimits` 限定每页 100、最多 10 页/1000 条/每响应 2 MiB。分页沿用现有 Link/SSO/错误/取消/路径/同源安全设施，以 `execute_bounded` 接收响应；页数、条目和响应超限有明确类型，先前页失败为 Partial，取消为硬错误。204 空仓库、200 空数组、403 权限失败、latest 404 可见性歧义分别表达，贡献者条目数不冒充总数。Languages 的总量使用 checked add，零数据与失败分离，零总量不生成比例。

响应元数据保留 ETag/Last-Modified/Cache-Control/可选 poll interval。单值方法接受条件验证器；low-level 没有缓存值，304 明确失败而不伪造空值。未实现调度、自动重试、轮询或缓存。官方依据、调用边界和后续 data/auth context/UI 顶部栏整合步骤见[主题文档](../repository-extras-service.md)。

## 验证

专项 fixture 覆盖：Release 草稿/预发布/assets/未知字段保留、空列表、多页、首页/后续页错误、分页页数/条目/循环防御及恶意同源或跨源 URL、SSO partial、主/次限流、取消；latest 成功与 404 歧义、403/SSO；Contributors 200 空数组、204、匿名、权限失败与短页观察数量；Languages 空对象、全零、比例、u64 溢出、非法计数/网络失败；响应字节限制、locator segment 编码与真实 localhost 条件请求/Authorization。

测试使用合成凭据及 MockTransport/localhost，不读取真实用户 PAT/gh token/Cookie，不访问真实 GitHub 业务资源。没有 GUI、真实 GitHub、私有仓库、组织 SSO 或 TLS 联调证据；UI 不在本任务范围。

2026-10-25 Windows 本地最终检查（每次重型 Cargo 使用 `CARGO_BUILD_JOBS=1` 和 `-j 1`）：

- `cargo fmt --all -- --check`
- `cargo check --locked --all-targets -j 1`
- `cargo clippy --locked --all-targets -j 1 -- -D warnings`
- `cargo test --locked -j 1`（207 passed，0 failed）
- `cargo build --locked -j 1`
- `git diff --check`

均通过。构建检查与 fixture 通过不替代 PR 最新 head 的四平台 CI、Codex 审查或后续真实产品集成验证。

## 后续与交付边界

低层 API 尚未连接 data/auth context。后续必须固定账户/授权上下文并做成功/失败后的旧响应拦截，再按明确缓存政策处理 304、验证器与存储，最后将三类真实数据组合到顶部信息栏。当前没有顶部 UI、Packages/Environments、Release 管理、资产下载或外部 URL 执行。

创建 PR 时关联 First Release milestone #1；当前无待裁决项。最新远端 head、检查、审查与合并信息由 PR 和 `.agents/task/delivery-state.json` 维护，不为事后日志额外制造未经检查提交。
