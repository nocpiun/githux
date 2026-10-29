# README 图片加载与 Reader 自然高度布局

- 日期：2026-10-29（Asia/Shanghai）。任务 ID `reader-images`。执行者：基层 Agent（派发会话）。
- 分支 `feature/reader-images`，base `730d2292dcd5bb981928341259c971e42ff497b5`。PR/Issue 链接见 GitHub 记录，不在本文杜撰。
- 目标与状态：为可复用只读 Reader 增加有界 README 图片能力（固定 commit 的仓库相对图片、公开 HTTPS 图片、失败/拒绝占位、可取消异步生命周期），并为「reader 是页面一节」的场景提供自然高度布局。**待审查/待负责人验收**；正式 Shell 接入仍未完成（见「后续」）。

## 改动与决定

### 图片：分层与新模块

- `src/content/images/`（契约与预算，无 I/O、无 GPUI）：`ImageLimits`（响应/解码/像素/单边/超时/并发/在途/保留/重定向）、`ImageTarget`/`ImageKey`、`ImageScope`、`ImageRequest`/`ImageCompletion`/`ImageSink`、`ImageProvider`，以及 `decode.rs` 的格式嗅探与有界解码（先读 header 尺寸再判预算，之后才在 `image::Limits` 下完整解码验证）。SVG 明确 `Unsupported`。
- `src/network/reader_images.rs`：独立的无凭据外部读取（`ExternalImageBytes` + `ReqwestExternalImages`）。只 HTTPS、`.no_proxy()`、手动逐跳校验重定向（同一 `reqwest::Url` 解析器、拒绝 local/userinfo）、总超时、流式字节上限。
- `src/data/reader_images.rs`：`RepositoryImageService` 复用已捕获的 `RepositorySession`，固定 commit → 根树 → 逐级目录 → `BlobIdentity` → blob 字节；符号链接/submodule 不跟随；owner/name 不符直接 `StaleContext`；不持有 PAT，不读 `download_url`。实例内小缓存（4 snapshot / 8 目录 / 4096 条目）。
- `src/ui/reader/images.rs`：每个 reader 每个文档代次一个 `ImageBoard`——请求去重、在途预算、代次校验、保留淘汰、拒绝与占位绘制。
- `src/ui/reader/markdown/document.rs`：`Block::Image` 走 `picture_unit`（解析目标 → 状态 → 元素），图片落地后 `remeasure_block`。
- `src/ui/reader/mod.rs`：`set_image_provider` / `set_image_limits` / `image_limits` / `image_scope`，图片泵（20ms 轻节拍、有工作才跑、结清即停）、`advance_images`（换文档/换 provider/隐藏窗口取消旧 scope）。

### 关键取舍

- **不能假报成功**：没有 provider 时每张图片显示 `no image service is attached`；没有网络/解码发生在 render。
- **代次与归属**：`ImageScope { reader, generation }`，两个 reader 共享一个 provider 时也不会互相取消或接收对方完成（`cancelling_one_scope_does_not_cancel_another_readers_work`）。
- **取消**：换文档/换 provider/隐藏窗口/服务 Drop 都取消对应 scope；取消的 scope 不再发布完成。
- **固定 commit**：图片请求只用 snapshot/tree/blob，绝不回落到默认分支或 contents API 的文本结果。
- **预算先于分配**：header 尺寸检查在像素分配之前（`a_huge_header_is_rejected_before_any_allocation` 用真实 GIF 逻辑屏尺寸构造）。
- **图片盒子**：宽度是唯一固定维度，高度由比例推导，图片绝对定位填充。"让 img 参与正常流" 的第一版实测让 320×180 的图变成 320×479；用真实窗口帧与 `flow/picture` 布局测试双向确认后修正。
- **Flow 布局**：`MarkdownLayout::Flow` 用同一 `item()` 逐块构建自然高度列，不复制第二套渲染；`on_layout_changed` 只在高度变化时通知一次宿主。Repository/Default 的有界滚动与滚动条完全不变。

## 验证

实际命令与结果（Windows，本 worktree 独立 `target/`，`-j 4`；与 shell-pages 并行期间未改共享 target）：

```sh
cargo fmt --all -- --check                 # 通过
git diff --check                          # 通过（仅 LF→CRLF 提示）
cargo check --locked --all-targets        # 通过
cargo clippy --locked --all-targets -- -D warnings   # 通过
cargo test --locked                       # 全部通过：635/635 bin、635 account、54 home、184 profile、163 reader、469 reader_images、536 repository、16 repository_overview
```

新增测试（均在 `tests/reader.rs` 与 `tests/reader_images.rs` 的模块装配中运行）：

- `src/content/images/tests.rs`（13）：格式嗅探、四种格式真实解码、SVG Unsupported、截断/垃圾 Malformed、编码/像素/解码预算、巨大 header 在分配前拒绝、key 稳定与 commit 隔离、sink、队列预算。
- `src/network/reader_images_tests.rs`（9）：真实 localhost socket——非 HTTPS/本地 scheme 无请求即拒绝、无 Authorization/Cookie/Proxy-Authorization、同源重定向跟随、file/data 重定向不接触目标、重定向循环有界、Content-Length 缺失的流式上限、停顿超时、取消不连服务器。
- `src/data/reader_images_tests.rs`（11）：固定 commit 的 tree/blob 链路与 URL 断言、目录缓存复用、符号链接拒绝、跨仓库 StaleContext、外链不触 GitHub、取消不发布、账户切换丢弃在途、超限、畸形、无效 session、按 scope 取消不影响另一 reader。
- `src/ui/reader/images_tests.rs`（12）：请求去重、无 provider 不请求、在途预算、跨代次/跨 reader 完成丢弃、目标块重测、保留淘汰、suspend、地址边界、两 commit 两源。
- `src/bin_support/reader_images_view_tests.rs`（13，真实窗口视图）：Healthy 全部就绪、宽图/高图在列内、失败与拒绝占位、超限、长文按需加载、切文档丢弃旧在途图片、无 provider 诚实占位、两个 reader 独立、图片落地不移动阅读位置、图片测量与相邻段落。
- `src/ui/reader/layout_tests.rs` 新增 3 项 Flow：自然高度超出宿主视口且块数不减、Flow 下选择与复制、Flow 高度变化报告一次且稳定后不再报告。

原生窗口验证（Windows，消息驱动，非负责人手感验收）：
`cargo run --example reader_images -- "<scene>"`，用 Win32 `PrintWindow` 取帧并实际查看（帧只保存在本地 `.agents/task/reader-images-observation/`，不入 Git）。

- Healthy：相对图片 320×180 正确比例显示、根路径宽图铺满内容列、行内 alt 作为图注；滚动可见后续图片。
- Extremes：1600×200 横幅在列内、120×900 高图被收窄（不再撑满视口）。
- Failure：四个块分别显示 `picture not found`、`unsupported picture format`、`local or unsupported source`（`file:` 被拒绝）、外链 404；alt 全部保留。
- Oversize：`picture too large`（编码预算）与 `picture too many pixels`（header 像素预算）。
- Long document：滚动到末尾才请求远端横幅图片。
- Natural height (page scroll)：README 完整展开，**只有外层页面滚动**；滚动 3 次后仍见中段图片与后续段落，末段页脚可见。

## 审查修复轮次（同一 PR，第二、三轮）

第一轮 Codex 审查提出 3 条，均核实成立并已修复 + 回归测试：

1. **图片泵启动顺序（P1）**：首帧构建子块时排队请求，发生在该帧的 pump 结算之后，没人重启它 —— 图片会停在 `loading`，直到无关重绘（原测试的重复 `refresh()` 掩盖了它）。现在帧末同步 dispatch 队列，并由根元素的 deferred prepaint hook 保证接收者在跑。`pictures_load_without_an_unrelated_redraw` 只推进 pump 时钟、从不 `refresh()`。
2. **替换仍附着的 provider（P2）**：`set_attached(true)` 因已附着直接返回，旧的 `Loading`/在途计数/旧 scope 都留着，新 provider 永远不被询问，旧完成还能匹配。替换现在推进文档代次并重置未完成需求（`forget_unfinished`，与 suspend 共用）。
3. **一个块多张图（P2）**：`blocks` 是「块索引 → 单个 key」，引用/列表/脚注里多张图会互相覆盖，后完成的图找不到块可重测。改为「key → 块列表」。

第二轮提出 3 条，同样核实成立并修复：

4. **过期完成消耗新代次名额（P2）**：`apply` 先 `in_flight -= 1` 再校验 scope；`begin` 重置计数后，一个旧完成会把新请求的在途名额吃掉，pump 提前停止。现在先校验 scope 再计数。回归测试反向验证：把顺序改回去测试即失败。
5. **reader 释放未取消图片 scope（P2）**：`on_release` 原来只取消滚动。`RepositoryImageService::start` 每个在途任务持有一个服务 clone，所以丢掉 provider 字段不会停止加载。现在 release 钩子显式取消本 reader 的 scope（不影响共享同一 provider 的另一 reader）。测试用「不完成、只记录 cancel」的 provider，先证明无修复时失败、加上修复通过。
6. **淘汰已解码图片后未重测（P2）**：`evict` 把旧 Ready 换成更矮的 `Released` 占位，但不返回受影响的块，列表保留旧高度。现在 `evict` 返回受影响的块，`dispatch_images` 用同一条 `remeasure_block` 路径重测。

第三轮提出 3 条，均核实成立并修复：

7. **挂起未作废旧 scope（P2）**：取消 provider 不能移除已进入 sink 的完成；`suspend()` 重置了计数但没改 scope，恢复可见后旧完成会通过 scope 校验并消耗新工作的名额。`suspend_images` 现在推进代次（`ImageBoard::advance`，保留已就绪图片），旧完成因此被拒绝。
8. **换预算未取消旧板（P2）**：`set_image_limits` 丢弃旧 board，但新 board 拿到新的 reader 标识，之后文档变化/释放只会取消新 scope —— 旧任务继续跑并占用 provider 的 semaphore。现在替换前先取消当前 scope。
9. **仓库图片缺少总超时（P2）**：`total_timeout` 原本没作用于仓库路径（commit + 每一级目录 + blob 各自只有 transport 的单请求超时），一段慢而成功的序列可以远超预算并长期占住并发名额。`load` 现在用一个 deadline 覆盖全部请求与解码；`the_total_timeout_bounds_the_whole_repository_load` 反向验证（去掉 deadline 测试会卡到超时）。

第四轮提出 1 条，核实成立并修复：

10. **解码后未复检授权（P2）**：`spawn_blocking` 解码是另一个 await 边界；此前只在解码前检查 session。账户在此期间切换/撤销时，旧授权下解出的仓库字节仍可能作为成功结果发布。现在解码后再 `session.verify()`（与 `RepositorySession::content` 的准备后复检一致），陈旧优先于图片、取消与 worker 错误。测试用 per-service 的 publish probe 精确停在「解码完成、尚未发布」处切换账户；去掉复检后该测试确实拿到 `Ok(DecodedImage)`，证明缺陷真实存在。

第五轮（第四轮修复后的 head）的 CI 与最终回执以 PR 记录为准。

## 后续

- 正式 Shell 未接图片：`src/ui/repository/mod.rs`、Profile、Home 仍用旧 Reader API（无 provider → 诚实占位）。最小接线契约见 `docs/reader-images.md#后续正式接线契约`；`ReaderImageService` 与 `ReqwestExternalImages` 都已是生产实现，不是 mock trait。
- Profile README 的 280px 视口属于 shell-pages；本分支只提供 `MarkdownLayout::Flow` + `on_layout_changed`，**未把该缺陷说成已修**。宿主接入步骤见本地交接 `.agents/output/reader-flow-contract.md`。
- reader-html 任务拥有 `markdown/ast.rs`。本任务未改 AST、未新增 `Block` 变体；它若需要新的渲染分支，先协调再在同一处补 `document.rs`（清单在交接文件第 7 节）。
- 表格 cell 内图片仍只显示 alt；inline 图片成为段落后的图片块。这是当前 AST 的支持边界，不声称已覆盖。
- 候选教训（供协调汇入共享 LESSONS，本任务不自行添加条目）：
  1. 「占位盒子给比例 + 子元素参与正常流」会让孩子尺寸反过来决定父高度，比例失效；需要绝对定位/显式尺寸，且必须用真实窗口帧确认，不能只看元素树的 shape 断言。
  2. 同一 provider 被多个消费者共享时，取消键必须含消费者身份（`ImageScope.reader`），只用「代次数字」会在两个消费者同时从 0 开始时互相取消。
  3. GPUI 测试平台中 `on_children_prepainted` 之后不能再同步改自身状态并期望同帧生效；用 `window.defer` 在下一 dispatch 回报，否则宿主拿不到高度变化。
