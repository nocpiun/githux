# Profile README 自然高度 Flow 与 Pinned 简介三行

- 2026-10-30，任务 `shell-pages`（PR #42 续作），执行者：基层实现 Agent。
- 状态：实现与自动检查完成，等待负责人验收；PR 保持 `pending`，不合并。

## 背景

上一轮把 Profile README 的固定 280px 高度移除后，正文区域塌缩成空壳：ReaderView 根节点仍是 `size_full()`，而 MarkdownLayout 只有 `Default`/`Repository` 两种有界视口，父容器的自然高度与内部虚拟列表不匹配。当时把该条目标记为"依赖未接入"，属于已知未完成，不是已完成。

`reader-images` 的 Flow 能力已随 PR #45 合入 main（`9522eee`）。本轮在此基础上真正接入，不再保留临时移植。

## 改动

1. **Profile README 使用 `MarkdownLayout::Flow`**（`src/ui/profile/view.rs`）：构造 Profile 的 ReaderView 时设置 Flow，并注册 `on_layout_changed` 让 Profile 页面在文档自然高度变化时重新布局。正文随 Profile 外层页面滚动，reader 不再自持视口、内滚动条与滚轮捕获。
2. **Pinned 简介自然换行、最多三行**（`src/ui/profile/render.rs`）：移除卡片固定 `h(px(94.0))` 与单行 `Ellipsis::new`，改为 `line_clamp(3)` 真实排版换行，并用与同一字号/行高推导的三行高度作为 `max_h` 上限。1–3 行完整占高、卡片随内容增长；超过三行在第三行末省略且不再撑高卡片。名称截断、Public badge、footer 左对齐与卡片边框保持不变。
3. **不再重复移植**：本地曾临时把 Flow 补丁搬进分支；确认 #45 已合入 main 后放弃该移植，改为 rebase 到 `9522eee` 并只保留 Profile 接入与卡片修改。

## 与已合入 UI 修复的整合

`#44`（repository-header-align，`b85284d`）与 `#46`（repository-language-clip，`9225787`）已被负责人验收并 squash 合入 `feature/shell-pages`。本轮先 `fetch`，把远端分支重放到 main `9522eee` 之上，再恢复本任务 staged 的 Profile 修改，两项修复随基底保留。

## 验证

- 串行 Cargo helper（`CARGO_BUILD_JOBS=1`）、独立 target：`fmt --check`、`check --locked --all-targets`、`clippy --locked --all-targets -- -D warnings`、`test --locked`（主 binary 791、home 55、profile 192、reader 167、reader_images 484、repository 556、repository_overview 18）、`git diff --check` 全部通过。
- 新增回归 `flow_readme_reports_its_whole_document_and_outer_scroll_reaches_later_sections`：880×560 视口下断言 `reader.is_flow()`、README 文档真实高度 > 400px、Profile 包装容器不压缩文档、标题/中段/文末都落在文档边界内，并只用 Profile 外层滚动到达 Pinned 与贡献图。
- 新增回归 `repository_descriptions_use_up_to_three_real_lines_and_grow_the_card`：同一 grid 内 1/2/3/超过 3 行简介，验证两行卡高于一行卡、三行描述真实占多行、超过三行的描述盒不超过三行行高、卡片不按全文高度增长，宽窗口下换行减少。
- Flow 自身三条契约测试（自然高度/无内滚动、选区复制、高度通知去重）来自已合入 main 的实现，本分支随基底运行通过。

## 未覆盖

- 真实窗口的原生鼠标/键盘、IME、多 DPI、触控板仍未由本轮自动化证明；本机远程会话中窗口会被系统最小化，截取的整页画面不能证明原生交互。
- 负责人已在本地查看窗口；本轮不把合成截屏当作验收证据。

## 审查反馈修复（同 head 后续）

- **重复解码 contributor 头像**：`sync_repository_avatars` 由 Shell 每帧调用，原实现每帧把已缓存的 `Image` 重新转成 `RenderImage`，`set_avatar_image` 才丢弃。新增 `RepositoryOverview::has_avatar_image(binding, user_id)`，同步时先查询，已附加者直接跳过，不再解码。回归：`avatar_image_presence_is_queryable_so_hosts_skip_repeated_decoding`。
- **本人 contributor 链接重复建页**：repository contributor 原先硬编码 `Route::UserProfile`，与侧栏/Home 的 `Route::Profile` 键不同，导致本人已开 Profile 标签不被复用。改为统一 `Route::profile(login, active_account)` 归一化；回归：`own_contributor_link_activates_the_existing_profile_tab` 断言路由、复用同一 TabId、只新增仓库标签。
- 检查：fmt/clippy `-D warnings`/`test --locked`（主 binary 795 项及全部 target）通过。
