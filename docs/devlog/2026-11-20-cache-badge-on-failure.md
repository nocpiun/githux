# 缓存标记按请求结果呈现

## 修改前审计

| 位置 | 基线行为 | 处理 |
| --- | --- | --- |
| Profile header · Basic | `is_historical` 直接触发 Cached，预读与失败回退同标 | 修复 |
| Profile `render::provenance_badges` · README、Pinned/Popular、Contributions | 同上。Partial 有独立条件；README 确认缺失会省略 | 修复 Cached 条件并接入分区重试 |
| Home / Feed 两列 | 历史来源直接触发 Saved，首屏和回退同标，现有列刷新可重试 | 修复两列标记条件 |
| 侧栏仓库列表 | `historical_status` 将预读误报成 Offline snapshot，已有列表级重读入口 | 修复标记条件 |
| Repository 摘要、分支列表、Code 树及正文 | 请求失败状态和保留内容共同决定标记，分区刷新清除失败状态；固定对象命中可静默复用 | 沿用并核查 |
| Repository overview · Releases / Latest、Contributors、Languages | 各端点失败单独跟踪；展示还需可用值，空 Releases 保持省略 | 沿用并核查 |
| Repository overview · Issues/PR counts | 历史计数隐藏，无 Cached 标记 | 不适用 |
| Notifications、全局 Issues/PR、Settings、账户凭据、头像及 README 图片 | 无页面历史快照标记；Saved accounts 指已保存账户，Feed 续页未缓存提示是分页结果 | 不适用 |

基线最小场景复用 Profile 的 `cached_sections_paint_before_the_network_reads_finish_and_are_then_replaced` 组合用例。先预热四个分区，再打开同一 Profile 并挂起 transport；基线断言要求在 README 已 Ready 且网络仍在途时看到 `profile-readme-provenance-historical`。首次基线 Cargo 命令中断，runner `a28dc3cb` 记录等待构建槽约 381 秒后开始编译，无测试退出结果。基线现象依据原断言和渲染条件核对。

## 复现与验收

正式应用入口 `cargo run --locked`。联网打开带公开个人 README 的 Profile，等资料加载后关闭标签。重新打开时，已缓存的正文可在网络请求期间阅读，标题行没有 Cached。离线重开同一 Profile，等待请求失败后，保留的正文会显示 Cached 与 Retry。恢复联网后点 README 标题行的局部重试。按同样方式可核对 Home/Feed 和侧栏仓库列表。使用原生应用验证需要可用账户及一份已经在线获取的快照。

离线组合回归通过以下入口使用临时缓存和可挂起的传输 fixture，无需 GitHub 账户。

```sh
node scripts/verify.mjs -- cargo test --locked --bin githux -- app::pages::profile app::pages::home a_cached_repository_list a_network_failure_falls_back ui::profile failure_indicator_tests
```

回归覆盖 Profile 四区缓存首屏、README 单独失败与同页其他分区成功、局部重试进行中/再次失败/恢复、无缓存失败、迟到快照与撤权。Home 两列和侧栏覆盖缓存首屏静默与网络替换；Repository 沿用既有各区失败指示组合回归。Windows 离线回归 `f902af87` 通过 94 项，构建槽等待 134 秒，Cargo 执行 63 秒。`cargo fmt --all` 由 runner `b76304b2` 通过，`git diff --check` 通过。负责人原生视觉验收待完成。
