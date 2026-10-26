# Markdown 三项视觉反馈的依赖补丁方案

更新：2026-10-26。任务 `content-reader`，PR [#27](https://github.com/nocpiun/githux/pull/27)。**状态：方案待裁决，未实现。** 本轮没有修改 Cargo 缓存、Ely checkout、依赖版本，也没有 vendor 或重写 Markdown；`docs/content-reader.md` 与开发日志同样把这三项列为未完成。

## 1. 为什么现有接口做不了

Ely 固定 revision `da3fdcaf39f2fa85ce1dd9cad07c3c81cf1dd721` 的三处限制（已逐文件核对）：

| 需求 | 现状 | 位置 |
| --- | --- | --- |
| 正文纵向/横向滚动条 | `MarkdownDocument` 的真实 `ListState` 是私有字段，没有读取或绑定 `ScrollHandle` 的公开接口 | `src/documents/document/mod.rs:101`（`list: ListState`） |
| fenced-code 语言标签上移 | 预览走 `document/visual.rs` 内部的 `draw_nodes`，**不转发** `MarkdownRenderer::code(...)`；渲染器已有的 `DrawCode` 钩子在文档模式下不可达 | `src/documents/document/visual.rs:149`、`src/documents/render.rs:79` |
| inline code 圆角/小 padding/代码字体 | `Style::Code` 只产出 `HighlightStyle`（背景色 + 前景色），没有独立字体、圆角或 padding，且不暴露每个代码段的排版位置 | `src/documents/render.rs:136`（`Style::Code | Style::Math =>`） |

`HighlightStyle` 是 gpui 的类型，只有 `color/background_color/font_weight/font_style/underline/strikethrough/fade_out`。因此“背景 + 代码字体 + 圆角 + padding”不可能只靠项目侧传参实现；在外层再画圆角背景又没有字形位置，只能靠估算宽度，会与真实换行和选择错位。

## 2. 最小可审查补丁（三处，均在 Ely 内，均为新增公开入口）

保持现有行为与签名默认不变，只增加可选入口，避免影响 Ely 其它使用方与已合并的 workspace tabs。

**(a) `src/documents/document/mod.rs`**

```rust
/// The block list's own scroll state, for an owner-drawn scrollbar.
pub fn scroll_handle(&self) -> gpui::ScrollHandle;   // 返回 self.list 使用的句柄
```

实现方式二选一，二者都能不改语义：
1. 把 `MarkdownDocument::new` 里的 `ListState::new(...)` 改成同时保留一个 `ScrollHandle`，用 `list(...).track_scroll(&handle)` 绑定（`List` 支持 `track_scroll`，见 gpui `elements/list.rs`）——推荐，`ListState` 仍归文档所有。
2. 或加 `pub fn list_state(&self) -> &ListState`（`ListState` 已是 `pub`，只缺访问器），由调用方自行 `list(...)`。**不推荐**，因为它会让调用方重建列表元素。

**(b) `src/documents/document/visual.rs` + `src/documents/document/mod.rs`**

把 `MarkdownRenderer` 已有的 `DrawCode` 钩子（`Rc<dyn Fn(ElementId, Option<SharedString>, SharedString) -> AnyElement>`）透传到文档状态：

```rust
// document/mod.rs
pub fn code(&mut self, draw: impl Fn(ElementId, Option<SharedString>, SharedString) -> AnyElement + 'static);

// document/visual.rs：把 self.code 放进 Hooks，交给 draw_nodes
Hooks { .., code: &self.code }
```

`render.rs` 的 `Hooks` 已有 `cell` 字段先例，新增一个 `code: Option<&'a DrawCode>` 是同一形态的最小改动；`Draw::drawn` 里已经存在 `if let Some(draw) = self.code` 分支，只需让它也能被文档路径取到。这样语言标签位置由项目侧的代码块渲染器决定，不改 Ely 默认外观。

**(c) `src/documents/render.rs`：inline code 的可选渲染钩子**

在 `Hooks` 增加：

```rust
pub inline_code: Option<&'a dyn Fn(&str) -> AnyElement>,   // 传入代码 span 的纯文本
```

`Draw::inline` 的处理顺序：先把 `Style::Code` 的范围从 `StyledText` 高亮里剔除，再按这些范围把段落切分为 `[文本, 代码, 文本, 代码, ...]` 的行内序列（div 水平排布），代码段交给钩子。**这一处是真正的布局改动**，因此方案明确列出影响：

- **换行**：行内序列用 `div().flex().flex_wrap()` 时，代码段作为不可分割的整体参与换行，长 inline code 会整体换到下一行而不是断词。若要保留逐字符换行，需要把钩子改为返回 `(StyledText runs, 间距)` 由 Ely 自己拼，代价更高。
- **复制/选择**：现有选择是按单个 `StyledText` 的 `TextLayout` 反查字节偏移（`render/point.rs` 的 `point_text`）。拆成多个元素后，跨代码段的拖选不再由同一次布局完成；需要为每段保留 `(起始字节, TextLayout)` 并在 `OnPoint` 里二分定位。**这是本方案最大风险项**，也是它必须单独裁决的原因。
- **padding 不进入文本**：padding 用容器 `px` 实现，绝不写入文本，因此复制内容不变。
- **虚拟化**：只在可见 block 内进行，不增加全量节点，2 MiB 上限与虚拟化不受影响。
- **性能**：每个含 inline code 的段落多出若干元素；不含代码的段落路径不变。

替代的更低风险选项（也是备选）：只给 `Style::Code` 增加 `font_family: Option<SharedString>` 到 `HighlightStyle`——**不可行**，gpui 的 `HighlightStyle` 没有字体字段，无法设置 Google Sans Code。因此“代码字体”这一项必须走上面的布局钩子，或接受仅换色。

## 3. 固定版本与干净 CI 复现

不允许直接改 Cargo 缓存，因此补丁必须以可追溯方式进入构建：

1. 在 Ely 上游仓库提交上述三处改动（或我方 fork），打 tag，例如 `reader-code-hooks.1`，记录完整 commit SHA。
2. 本仓库 `Cargo.toml` 只把 `ely-gpui-component` 的 `rev` 改成该 SHA，其余依赖与 Rust 1.98.1 不变；`Cargo.lock` 由 `cargo update -p ely-gpui-component` 生成并提交。
3. 干净 CI 复现：CI 的 `swatinem/rust-cache` 按 lockfile 缓存，Git 依赖以 `rev` 定位，因此干净 runner 会从上游 Git 拉取该 SHA；`cargo check --locked` 会因 lock 与 manifest 不一致而失败，从而保证 SHA 一致。验证命令：删掉本地 `~/.cargo/git/checkouts` 后 `cargo check --locked` 仍能构建。
4. 回滚：把 `rev` 改回 `da3fdcaf…` 并恢复 `Cargo.lock`，项目侧钩子调用点改为无钩子默认路径。

## 4. 裁决需要回答的问题

1. 是否允许把 (a)(b) 两处（纯新增访问器/透传，无视觉与选择语义变化）先合入上游并按 SHA 固定？
2. 是否接受 (c) 的行内序列方案及其“跨段选择需重做定位”的代价，还是本轮只做 (a)(b) 并把 inline code 样式延后？
3. 若选择 (c)，长 inline code 允许整体换行（不接受断词）是否可接受？

**在这三个问题有结论前，本轮不实施 Markdown 三项，也不宣称完成。**
