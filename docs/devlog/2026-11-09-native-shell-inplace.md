# Native Shell 原位替换：六任务组合交付

- 日期：2026-11-09。协调者集成到 `migration/native-shell`；[Draft PR #75](https://github.com/nocpiun/githux/pull/75)，不合入 main。
- 状态：六个任务已接入默认应用，真实根包验证通过；原生视觉和交互最终由负责人验收，无 Computer Use。

## 集成来源

| 任务 | 原执行者提交 |
| --- | --- |
| 基础 Shell / runtime | `e28ef8d` |
| Reader | `bb90cf3` |
| Home / Profile | `fc21d1b`，补充真实 Profile 回归 `4c5326f` |
| Repository | `5dad4c9` |
| Notifications / 全局 Issues、PRs | `4e6c623` |
| Accounts / Settings | `f100d5e` |

按各任务文件所有权集成快照，同时恢复基础清理时删除、但叶子任务仍需要的未修改模块；因此不把“叶子提交的 diff 能应用”当成完整模块已接齐。原执行者分支保留。

## 结果行为与组合修复

- 默认 `githux` 入口和原 `src/ui` 使用纯 Rust NSH 原生 Windows 后端；删除双轨 `src/nsh` / `native/nsh`，根依赖不再包含 Ely、GPUI、QuickJS。保留 NSH 默认主题和原有字体。
- 账号、Home/Feed、Profile、Repository/Reader、Notifications、全局工作项和 Settings 均挂载真实服务页面，保持已有凭据/缓存/授权/旧响应控制。
- Settings 不再阻止另一个新页面首次挂载；未登录表单不随每次 tick 重建。Search 打开可过滤的本地页面/仓库选择，补上侧栏折叠和当前页刷新。仓库贡献者导航到对应 UserProfile。
- Profile 与 UserProfile 接入同一个 README 图片工厂。恢复头像的真实下载、准备和账户缓存控制，供侧栏、Accounts、Feed、Profile、贡献者复用；实际读取仍在已有后台 executor 中。
- Shell 空闲 tick 不再重建整页；只在实际快照/挂载变化时更新宿主，避免点击目标失效。Home 测试也保留稳定挂载，仅更新导航证据；其活动状态时钟不再每 tick 重置。
- 退出在 NSH owner cleanup 中退役页面，避免在响应式运行时销毁后写信号；事件循环返回后等待账户持久清理。启动时异步恢复账户清理标记。
- 独立测试/示例改为引用正式库；原业务和 Reader 内联测试在库中执行一次，不再在 Reader 图片入口重复跑数百项业务测试。图片预算与隐藏恢复测试按资源身份控制完成顺序，不依赖 HashMap 枚举顺序。
- 删除 Ely Windows App SDK 启动/准备脚本与 CI 打包步骤；保留 Linux 库检查、测试，发行构建限于已实现的 Windows 原生应用。测试进程显式限制 4 个并发测试，测试内部的真实并发/交错断言不变。

## 验证

- `cargo clippy --locked --all-targets -- -D warnings`：通过（`e28a6594`），同时编译正式库、默认程序、所有测试与示例。
- `cargo build --locked`：通过（`ab4cfc5f`），产物 `target/debug/githux.exe`。
- `cargo tree --locked -e normal`：通过（`727b3b04`），无 Ely/GPUI/QuickJS/nsh-js。
- 包含真实 Shell 登录、Settings 后首次挂载、搜索导航和退出的组合回归通过；原 Profile 执行者已补齐在线、README 缺失、离线回退、撤权四项真实服务/headless 用例。
- 根包完整测试结果：`cargo test --locked -- --test-threads=4` 通过（`357a91a5`）：812 passed，0 failed；1 个原有示例 doctest ignored。。
- `cargo fmt --all -- --check`（`c19335f1`）及 `git diff --check` 通过；CI scope/PR policy 的 14 项 Node 测试通过。所有 Cargo 操作经 `scripts/verify.mjs`，没有调用真实 GitHub 或使用真实 PAT 做测试。

默认高并发的初次全量测试曾出现本地 HTTP socket fixture 的间歇失败；对应有界读取测试隔离复查通过（`4becab59`），四线程全量回归通过。未修改网络实现或放宽断言，不声称已定位 Windows 高并发 socket 失败根因。

## 验收与限制

- 这次集成不等于迁移已完成最终产品验收。Windows 实机视觉、鼠标拖选/Ctrl+C、IME、多 DPI、长文性能仍待负责人检查；没有使用 Computer Use。
- 标签拖拽/完整键盘焦点与超长 Reader 选区的完整对等性尚未证明或仍有缺口（Reader 只保证已挂载窗口内选区）；不能把 headless 通过替代这些验收。
- 内部 PR 详情仍按原功能范围留空；不扩展产品范围。macOS/Linux 原生应用尚未迁移。
- PR 保持 Draft + pending。整体替换超过现有 PR Check 的新增/删除各 20,000 行限制；不修改或绕过规模门槛，不据此合并 main。当前 PR 供审阅整合后的 diff，最终主线交付需要处理拆分方案。
