# 2026-11-01：全局 Issues / Pull Requests

任务 `global-work-items`，基线 `997f6e947fced45ca00b728fcea4756957f36061`，分支 `feature/global-work-items`。授权范围是 API、账户服务、两种列表及联合预览，由单一执行者交付；正式 Shell 集成与业务缓存均不在此任务。行为契约见 [全局列表](../global-work-items.md)。

## 实现

- 新领域模型、白名单查询、完整身份 cursor；Assigned/Created、Open/Closed/All、Issues/PR 共用同一单页适配器。保留共享网络、错误分类、SSO 和分页头解析，Home 实现未改。
- 新 WorkItemSession 复用 RepositorySession::read，捕获权威 login、账户 epoch、selection 和服务实例；没有复制凭据加载/semaphore。
- Controller 将请求身份与代次关联，支持显式刷新/追加/重试、稳定 id 去重、partial 与 typed error 保留，拒绝旧成功与旧错误。
- 有界 ListState + 共享 Bars/Motion，Ely 控件与现有字体。仓库名提供 typed 回调；标题没有详情路由、外跳或替代导航。正式 ui/mod.rs 与 Shell 入口未改。
- 专用 example 用真实账户服务、facade、adapter 和 controller，共用按请求身份应答的 mock transport。支持三页、重复操作、失败恢复、空、部分、超过 1,000 及延迟筛选。没有新增复制整套应用的 integration test target。

## 外部契约证据

2026-11-01 只读抓取 Search 官方文档，确认 1,000 上限、incomplete_results、pull_request.merged_at schema。无凭据请求公开 rust-lang/rust Closed PR 搜索（每页 2），代表项 state=closed 且 merged_at 非空、repository_url 为预期 repos 路径。未读取真实 token、私人仓库或通知，也无远程业务写请求。完整公开原始响应留在忽略的任务目录，不上传。

## 验证记录

所有 Cargo 命令通过 `node scripts/verify.mjs -- ...`。首轮 example check 发现 3 个 GPUI 编译问题（stateful Div 缺 id、列表 processor 类型、闭包所有权），逐项修复；view 测试最初因真实 background executor 跨线程唤醒 GPUI test scheduler 被判定非确定性，改为受控 runtime 取得结果后经内部应用入口发布，产品路径仍使用真实 executor。

最终聚焦 `cargo test --locked --example global_work_items work_items`：18 passed / 0 failed（run `69e7ce6e-031a-4aa4-b9ab-a2eb7c2dbcb7`）。主二进制账户隔离 `cargo test --locked --bin githux data::work_items`：1 passed / 0 failed（run `28f05a62-a49f-43cd-9b28-51eefd276bce`）。最终必需检查：

| 检查 | run | 结果 |
| --- | --- | --- |
| `cargo fmt --all -- --check` | `d28da10f-f292-4c1b-adf4-88c5d61c4ca6` | success, exit 0 |
| `cargo check --locked --all-targets` | `75e45619-5770-4058-a10c-957a9e98090c` | success, exit 0 |
| `cargo clippy --locked --all-targets -- -D warnings` | `bac642cc-3575-4378-b7f8-182d832b6d78` | success, exit 0 |
| `cargo test --locked` | `576d2cfe-81e6-4744-8c7d-6aec1bbaf1c4` | success, exit 0；713 + 54 + 246 + 225 + 551 + 614 + 16 passed，0 failed |
| `cargo build --locked` | `9d196c2e-69b8-4a3f-a71f-df2c400a0a4a` | success, exit 0 |

一次聚焦构建曾因仍在运行的 example 进程占用 `.exe` 报 LNK1104（run `ce820a9c-6ef4-4ef3-ae57-b29aabb523da`），结束旧进程后重建通过；不是源码失败。`cargo test --lib` 在单 bin package 上不存在 library target（run `53fe04c5`），改用 `--bin githux` 回归。所有测试均为离线 mock / GPUI 测试平台，不含真实 GitHub 请求。

### 原生预览

使用专用 `global_work_items.exe`（synthetic PAT、临时账户库、公开无凭据之外的零真实请求）打开真实窗口，Build long / 1,000 场景，以 `SetWindowPos` + `PrintWindow` 取得 960×720 与 430×520 帧。筛选、真实行 id、状态图标、仓库名、编号、更新时间、窄窗口有界滚动均出现；点击仓库名通过真实 callback 显示目标事件由 GPUI 测试单独覆盖。

审查 P2 正确指出最初从窗口宽度派生行宽，不能适配窄 pane。修复后行宽取自 `ListState` 自身 viewport（首次布局前仅以前一帧默认值兜底），列表测量不再把窗口宽度写回行。最终复跑的 fmt/check/clippy/test/build 均通过：`45c068b9`、`a9e57af6`、`18afad03`、`644922ab`、`fb1221d7`；聚焦 example suite 为 `16946e3c`（9 项 view）与 `252c57ab`（bounds），全量为 `644922ab`。

**修复与原生复测（后续轮）：** 先用 DPI 感知 + `PrintWindow` 工具重建对应关系，发现旧 `CopyFromScreen` 帧在 150% 显示器上产生缩放错位且混入被遮挡桌面。真实现象是列表 intrinsic width 被 Ely `Ellipsis` 隐藏全文污染，且旧 viewport/整窗宽度兜底在窄窗或放大 resize 后失效。修复为：标题/仓库/更新时间改用 GPUI 原生文本 `truncate()`（隐藏全文不参与测量），并移除固定行宽与窗口宽度兜底，让行由列表真实可用宽度约束。原生 DPI 感知帧（144 DPI、1440×1080 / 645×780 / 1800×1200，与窗口当前 `scale_factor` 一起记录）显示窄窗、放大 resize、宽窗、Load more、键盘末项场景下长文本均正确省略，未再越界。

**新增回归：** `long_text_stays_inside_a_constrained_pane_and_after_resize_and_append` 用真实 460px 受限 pane + 1200/1300 宽窗口，断言列表、行、标题、仓库名、更新时间都落在 pane 内，且 append 后键盘 End 命中末项。聚焦 `work_items` 用例现为 19 passed。

## 边界与教训应用

按 L002 将公开 API 形状核验、mock/GPUI 和原生交互证据分开记录。按 L007 复用 reqwest URL 同源判定；按 L010 预览与行为测试共用可重复 fixture，未建模请求为 599 开发错误；按 L012 在一个联合 example 验收 API→controller→视图。没有新教训需要直接修改共享 LESSONS。

UI 要求负责人原生验收，交付 PR 保持 pending，不自动合并。真实 PAT、私有授权/SSO、正式 Shell 挂载以及真人输入手感不由 mock 测试推导。

## 相对时间返工（负责人反馈“时间要调整，同上”）

主行时间由 `Updated <ISO>` 改为相对年龄，Issues 与 Pull Requests 共用同一行实现；精确 RFC3339 值仅保留 tooltip 与 `aria_label`。为避免第二套 parser，本模块用 `#[path = "../home/time.rs"] mod time;` 直接编译 Home 现有纯函数源文件，不编辑 `src/ui/home/time.rs`、`shots`、Home 引用或通知 worktree。这是过渡 consumer 接点：Notifications 正在抽取共享 helper，待其进入已提交基线后由协调者把引用统一到公共模块，本任务不读取其未提交文件、不阻塞等待。

视图新增可注入 wall clock（`set_now`）与低频 30 秒刷新任务；固定 clock 不建任务，窗口失活/隐藏、`set_active(false)`、视图释放都会停止。没有新增 GitHub 刷新、轮询或写操作；请求、过滤、分页、导航和旧响应防护未改。

fixture 时间改为相对预览固定时钟 2026-11-02T12:00:00Z 的确定性分布（Just now 边界、分钟、小时、Yesterday、多天、+09:00 offset），预览使用同一固定时钟，因此原生窗口能看到真实相对文本且可重复。测试断言 Issues/PR 两种列表的真实行标签包含分钟/小时/天/Yesterday，主文本不含 ISO 或 `Updated`，tooltip 保留精确值，invalid/future/offset 由共享 formatter 处理（未镜像重写 parser 测试）；并验证失活、隐藏、固定 clock 与释放后的计时停止。

验证：聚焦 `cargo test --locked --example global_work_items work_items` 为 24 passed / 0 failed（run `0b413c96-1280-41fa-a544-3ff3654dc90f`）。原生 DPI 感知宽/窄帧与最终必需检查记录见本轮 PR 评论与交付状态。

## 编号前移与基线微调（负责人截图反馈）

「把 issue/PR 的编号移到前面，其他的不用改」：仅调整元数据行子节点顺序为 `#number → 仓库名 → 状态 → 相对时间`，Issues 与 Pull Requests 共同生效。编号字体、字号、颜色、gap、行高、图标、标题、原生长文本截断、仓库链接语义、状态、时间 tooltip 与筛选/分页/键盘行为均未改；没有把编号移入标题或重复显示。

原生复核后负责人反馈编号与仓库名上下未居中：数字为大写高度、仓库名为小写高度，即使基线相同，仓库名在视觉上仍低约 1 logical px。改为 `items_baseline()` 对齐基线，并对仓库名链接使用 `relative().top(px(-1.0))` 做纯绘制上移（不影响布局与横向位置）；负责人确认「这次可以了」。此前尝试给编号加 `mt` 会造成观感偏差，已撤销。

验证：`work_items` 聚焦测试（含编号位于仓库名之前的断言）通过；原生 144 DPI 帧覆盖 Issues/PR 宽窄窗口，确认编号在前且仅一次、长仓库名正确截断、状态与时间未被挤出。最终必需检查记录见交付状态与 PR 评论。
