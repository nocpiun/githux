# Profile README：去掉正文灰底与可选缺省省略

- 2026-10-30，任务 `profile-readme-polish`（base `73a5894`，分支 `fix/profile-readme-polish`，PR base `feature/shell-pages`），执行者：基层实现 Agent。
- 状态：实现与自动检查完成，等待负责人原生视觉验收；PR 保持 `pending`，不合并。

## 背景

负责人 2026-10-30 对 shell-pages 的 Profile README 提出两项调整：

1. 去掉正文的灰色矩形底；正文应与所在 Profile 卡片/页面背景一致。
2. 无可展示的个人 README 时完全不显示该区块（含边框、header、错误文案与空行间距）。

上一轮已把 README 接到 Reader `MarkdownLayout::Flow`（[开发日志](2026-10-30-shell-pages-profile-flow.md)），因此本轮只做呈现层调整，不重做布局。

## 改动

1. **Flow 下不再自绘 surface 底**（`src/ui/reader/mod.rs`、`src/ui/reader/markdown/document.rs`）。
   - `ReaderView` 根在 `MarkdownLayout::Flow` 时使用透明背景，`Default`/`Repository` 仍为 `theme.colors.surface`。
   - 内层 `MarkdownView::render_flow` 根也去掉 `surface`。**为什么必须同时改这里**：只改 reader 根仍会被内层 Markdown 根的不透明 `surface` 覆盖，灰色矩形不会消失。两处都只在 Flow 路径调整，不新增共享 API。
   - 代码围栏、inline code、引用的 `sunken`/`hover` 语义色保持不变。
2. **确认缺失的 README 整段省略**（`src/ui/profile/render.rs`）。`render_readme` 改为返回 `Option<AnyElement>`；`SectionState::Unavailable` 直接返回 `None`，页面用 `.children(readme)` 挂载，因此不产生 border/header/文字/占位，也不留下 `gap_3` 空行。
3. **真实失败改为紧凑指示**（`src/ui/profile/render.rs`）。README 的 `SectionState::Error` 改用 `compact_status::failed` 的单个 warning 图标，原因在 tooltip 与无障碍名称；本组件没有真实 retry 回调，因此不显示 Retry。Pinned/Contributions 仍使用原有共享错误面板。
4. **404 归入可选缺省**（`src/app/pages/profile.rs`）。同名仓库读取返回的 `GitHubError::NotFound`（GitHub 用 404 同时表示不存在与不可见）映射为 `set_readme_unavailable`，不再显示 “not found or is not visible”。网络、认证、限流、解析失败仍映射为 `SectionState::Error`。这是任务授权的 “related profile readme absence mapping”。
5. **example 增加可预览缺省场景**（`src/bin_support/profile_fixtures.rs`）。新增 `Scenario::NoReadme`（“No README (absent)”），让缺省省略可在原生预览中直接切换查看。属于任务授权的 profile example 范围。
6. **无残留旧 README**：`bind`、`set_readme_unavailable`、非 Ready 的 `update_readme*` 都会 `cancel_reader` 并丢弃旧 `ReaderView`；新增测试覆盖 ready→缺失→换账户。

## 验证

在最终代码补丁上用 `.agents/task/cargo-serialized.ps1`（`CARGO_BUILD_JOBS=1`，独立 target）串行执行，全部记录真实退出码：

- `cargo fmt --all -- --check` → exit 0。
- `cargo check --locked --all-targets` → exit 0（约 3m55s，首次含依赖检查）。
- `cargo clippy --locked --all-targets -- -D warnings` → exit 0（约 1m07s；fixture 改动后复跑约 3s）。
- `cargo test --locked` → exit 0。主 binary 799；集成 target 57 / 197 / 168 / 485 / 559 / 20，全部 0 failed。fixture 改动后复跑同样全绿。
- `cargo build --locked` → exit 0（约 2m16s；fixture 后复跑 7s）。
- `git diff --check` → exit 0。

一次 `cargo test --locked` 首跑时 `network::transport::bounded_tests::chunked_and_no_length_stop_before_eof_on_success_and_errors`（真实 loopback socket + 1s 超时）在并行负载下偶发 `Network { error sending request }`；单独 `--exact` 复跑通过，随后整套复跑通过。该用例不经过本轮改动代码，属于既有的负载敏感 flaky，不是回归。

新增回归（`src/ui/profile/tests.rs`）：

- `confirmed_readme_absence_omits_the_whole_optional_section`：`Unavailable` 后 `profile-readme-section`/`reader`/`unavailable`/`error` 全部不存在，Pinned 区块上移到正常间距，仓库与贡献图仍可用。
- `actionable_readme_failure_keeps_a_compact_mark_not_absence`：`Error(RateLimited)` 显示 `profile-readme-error` 紧凑标记、不显示大错误面板、不挂载 reader，且内部仍是 `SectionState::Error`。
- `ready_to_absent_to_another_account_never_reuses_the_old_reader`：ready→缺省后旧 reader 不能再 emit；换 binding 后只挂载新账户的 README。
- 既有 `flow_readme_reports_its_whole_document_and_outer_scroll_reaches_later_sections` 继续断言自然高度与仅外层滚动。

## 原生预览与未覆盖

- 已构建并启动 `target/debug/examples/profile.exe`（`cargo build --locked --example profile`，exit 0）；进程存活至 15s 超时被终止（exit 124），无 panic/启动错误。**本会话没有可人工查看的桌面画面，无法代替负责人确认灰底消失与缺省省略的实际像素。** 请在本机运行：
  - `cargo run --example profile`，在场景 Select 选 “No README (absent)” 验证整段省略；选 “Normal / Design” 验证正文无灰色矩形底、文末内容随页面滚动。
  - `cargo run --example profile -- --long` 查看长 README 的 Flow 滚动。
- 正式 Shell 的 Profile 预览需要真实 Classic PAT 账户；本轮没有可用凭据，未做正式 Shell 的原生 Profile 截图。正式接线仍是 `app::pages::ProfilePage`，本轮只改其中的 README 缺省映射。
- 真实 Windows 原生鼠标/键盘、IME、多 DPI、触控板与中文/emoji 字体回退仍由负责人验收；测试平台不经过原生输入分发。

## 剩余问题

- 灰底是否彻底消失、缺省省略后的间距观感属视觉验收项，自动测试只能断言结构与存在性，不能替代原生像素确认。
- 未改动 `src/ui/home/**`、`domain/home.rs`、`github/home/**`、`app/pages/mod.rs`、`ui/mod.rs`、`docs/product.md`；未改动 HTML 解析、AST 或图片读取逻辑。

## 超出主目录但已报告的改动

- `src/ui/reader/markdown/document.rs`：仅去掉 `render_flow` 根的不透明 `surface`。必要性：只改 `ReaderView` 根会被内层 Markdown 根的同色 `surface` 覆盖，灰底不会消失；该函数只在 Flow 路径调用，不影响有界/Repository 渲染。
- `docs/content-reader.md`：只在 Flow 段落补充 “背景归宿主” 一句并更新日期。必要性：这是 reader 的可观察契约变化，按 [共享文档索引](README.md#文档怎样维护) 应由主题文档维护，而非只留在 Profile 文档。
- `docs/product.md` 未改；按任务分派，包含本条可选缺省规则的产品文档更新属于 home-feed-focus。
