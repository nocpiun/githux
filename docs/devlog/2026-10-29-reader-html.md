# reader-html：Markdown 中的 HTML 与注释

- 日期：2026-10-29（Asia/Shanghai），任务 `reader-html`，PR [#47](https://github.com/nocpiun/githux/pull/47)，分支 `fix/reader-html`。
- 状态：解析、复制和图片接口回归已实现；真实 Reader fixture 原生预览已核验，等待当前 head 审查/CI 与负责人验收。保留 `pending`，不合并、不 auto-merge。

## 行为与范围

`ast.rs` 原先把块级 HTML 生成 `Block::Html`，行内标签直接追加到可复制正文；同一 fixture 在修复前真实窗口显示了 `<div align="center">`、注释里的 table/img 和标签正文。

新增 `src/ui/reader/markdown/html.rs` 做子集分词、属性/实体解码和容错树。HTML 事件转换为已有 `Paragraph`、`Heading`、`Quote`、`List`、`Table`、`Code`、`Image` 及 `Inline` 样式/链接；注释、声明及 script/style/template/iframe 等丢弃元素的内容不进入模型。行内丢弃状态跨事件保存，抑制 Markdown 文本、图片、数学和样式输出，因此隐藏内容不会产生图片请求。Markdown 代码事件不参与 HTML 解析。

`<img src alt>` 与 Markdown 图片都生成 `Block::Image`，地址由既有 `links::resolve` 与图片板解析；不新增网络路径。真实 Reader 测试记录 provider 收到的请求，核验 owner/name/固定 commit/相对目录与 scope，确认注释和脚本内图片没有请求；同一视图中 `Ctrl+A/C` 的剪贴板内容保留可见正文而没有隐藏内容。

HTML 树与跨事件行内栈都有 256 层上限；表格补齐单元格前检查 65,536 格预算，超限回退为按行文本；实体扫描窗口有界；畸形结束标签/声明消费剩余片段；ordered-list start 保留足够的加法余量。相关回归覆盖审查中发现的栈溢出、二次扫描、表格内存膨胀、状态泄漏和 UTF-8 边界。

详细支持子集和未支持项只维护在 [阅读器文档](../content-reader.md#html-子集2026-10-29)。`div`/`p` 的 align 视觉布局忽略，内容保留；不支持 CSS、表格跨度、SVG/MathML 布局或 details 折叠交互。表格内图片沿用现有 alt fallback，段落内图片沿用段落后的图片块。

无新依赖，未改 Cargo.toml/Cargo.lock、Ely revision 或依赖缓存。任务实现未编辑 `reader/mod.rs`、`markdown/document.rs` 或图片网络模块。main 的 reader-images PR #45 合入后，正常 merge `9522eee`，解决两份主题文档冲突；图片板与 HTML 转换在同一组合上验证。`Block::Html` 暂保留兼容分支，但本解析器不再产生它。

## 验证与证据

所有重型 cargo 命令经本 worktree 的 `.agents/task/cargo-serialized.ps1`，独立 target，`CARGO_BUILD_JOBS=1`。

- `cargo fmt --all -- --check`、`git diff --check` 通过。
- `cargo check --locked --all-targets`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo test --locked` 在与 reader-images 组合后通过；本轮 Reader 目标 222 项通过，包含抑制事件、真实 provider/剪贴板、单元格关闭范围和 inline title 回归，完整检查结果在 PR 与本地 delivery-state 绑定最新 head。
- `cargo build --locked --example reader` 通过。运行 `reader.exe 1` 展示 HTML fixture，不注入图片 provider，因此明确显示“no image service attached”，不把占位当作真实网络图片验收。
- 真实 Windows 窗口，同一 fixture，1280×900、DPI 144：修复前截图 `.agents/output/evidence-before.png`；本轮修复后 `final-html-top.png` 显示图片占位、强调/链接/代码、表格列对齐和列表；向真实 HWND 投递 wheel 消息后 `final-html-bottom.png` 显示代码中的标签/注释原样、正文注释消失、尾部文字可达。这些帧已实际查看。
- 截图曾因 CopyFromScreen 抓到锁屏壁纸，已改用 Win32 PrintWindow 重新核验。壁纸帧不作为验收证据；截图/辅助脚本仅留本地 `.agents/output`/`.agents/task`。

未覆盖真实账户/网络 README、多 DPI、跨屏、实体鼠标手感和 IME；synthetic fixture、provider 记录测试与真实实网验收分别说明。最终人工验收仍须由负责人完成。

## 审查处理与后续

2026-10-30 继续处理 `706b629` 的预先打开行内 span 审查：抑制期间新开的 Markdown span 只记账不输出；没有隐藏 span 对应的结束事件则闭合抑制前已经打开的 span，避免强调/链接延伸到 after。新增强调与链接两种范围回归。

2026-10-30 继续处理 `5eda30d` 的文档级 raw-text 审查：已处于 script/style/title/textarea 的状态时直接找同名真实结束标签，不把后来块中的同名开始标签再计为嵌套；新增四种 raw-text 标签的跨段恢复与扫描深度回归。

2026-10-30 继续处理 `c561891` 的两个移交入口：单元格带 raw-text 状态时也完整移交，移除旧的 is_none 排除条件；块级 outstanding_span 返回 raw-text 上下文并保存到文档状态，suffix 新开元素也遵循相同契约。新增单元格/块级两种入口的隐藏图片回归。

2026-10-30 继续处理 `b3c6616` 的跨段 raw-text 审查：段落和单元格结束时把内层 raw-text 名字一同移交到文档状态，块级扫描在它真正关闭之前忽略外层关闭字符串；新增跨空行 script 内伪关闭与隐藏图片回归。

2026-10-30 继续处理 `847db64` 的行内嵌套 raw-text 审查：builder 保留内层 raw-text 标签名，结束前不接受外层关闭字符串；行内扫描器通过可变深度回写嵌套状态，避免局部加深后在下一事件重置。恢复原有嵌套 template 和 raw-text 同名字符串测试后，218 项 Reader 回归全部通过。

2026-10-30 继续处理 `d4e66b9` 的两条审查：raw-text 元素正文的同名标签不增加嵌套深度；抑制期间创建的代码块、列表、引用、表格、脚注仍保持容器平衡，但结束后撤销完全隐藏的空壳和标签，避免语言标签/空项目泄漏到可见模型。两项均有回归。

2026-10-30 继续处理 `3f9bec6` 的两条审查：单元格结束时把 pending builder 的抑制名字/深度移交到文档状态；丢弃子树在构造普通元素树之前完整跳过，深度限制不会再把 template/iframe/object 的开标签展平而留下内部图片。两项均有回归。另对 script/style/title/textarea 的边界扫描直接检查开启状态与完整关闭后的 suffix，避免共用扫描器只跳过 raw-text 正文却没有记录其自身起点。

2026-10-30 继续处理 `93ea6c6` 的两条审查：抑制期间统一维护所有块容器开闭（含表格与脚注），不再只维护列表/引用，关闭标签落在表格单元格时不会弹出根容器；查找未闭合起点时跳过此前已闭合的同名片段，因此前面的 visible 段落不会被吞掉。两项均有回归。

2026-10-30 继续处理 `775c492` 的两条审查：丢弃状态改为 `(name, depth)`，跨块与行内转移都保留同名嵌套深度，内层 `</template>` 不再提前释放外层；`dropped_span` 在寻找关闭标签时跳过其它 raw-text 元素体，script 字符串里的 `</template>` 不算关闭。两项均有回归。

2026-10-30 继续处理 `c922dd6` 的两条审查：抑制期间不再逐行重扫累积缓冲，只在 `End(HtmlBlock)` 判定一次，避免二次复杂度（新增 2 万行抑制块的时间上界回归）；`template` 不再按 raw text 处理，嵌套 template 按深度计数的普通元素解析，内层 `</template>` 不会提前放出外层的图片。行内抑制状态同时记录嵌套深度。

2026-10-30 继续处理 `520dbc9` 的两条审查：抑制期间的块级 HTML 同样在完整缓冲片段上判断关闭，多行注释里的 `</template>` 不再提前结束抑制；关闭标签之后的可见后缀若自己开启新的丢弃元素（如 `</template><iframe>`），会重新进入文档级抑制。两项均有回归。

2026-10-30 继续处理 `c40cb49` 的三条审查：丢弃开启判断改为在 `End(HtmlBlock)` 用完整缓冲片段完成，多行注释与属性值里的 `<script>` 不再触发抑制；void 元素（meta/link/base/embed）不再被当作未闭合；行内开启的抑制在段落结束时转移到文档级状态，后续段落中的内容与图片继续被抑制直到真实结束标签。三项均有回归。

2026-10-30 继续处理 `433df51` 的两条审查：抑制期间同时识别 `Event::Html` 与 `Event::InlineHtml`，段落内到达的 `</template>` 也能结束抑制；`textarea` 纳入整段丢弃集合，其内容不再成为可复制正文。两项均有回归。

2026-10-30 继续处理 `5ca540c` 的两条审查：块级丢弃片段按节点边界切分，丢弃元素之前的可见内容保留，关闭标签之后的可见内容恢复显示，而不是整片段跳过；抑制期间仍执行列表/引用的开闭记账，避免 `- <template>` 这类场景留下 stale container 导致文档变空。两项均有回归（前缀/后缀内容、列表关闭）。

2026-10-30 继续处理 `c645711` 的两条审查：块级丢弃元素（template/iframe/object/script/style）现在跨 `HtmlBlock` 边界保持抑制，空行后的 Markdown 图片与文本不再生成或请求；只有真实同名结束标签才恢复内容，注释或 `</templates>` 不算。HTML 空白折叠改为只处理 ASCII 空白，`&nbsp;` 等 Unicode 空白保留原作者间距与不换行语义。两项均有回归（分别覆盖五种标签、注释/长名干扰与普通 ASCII 折叠）。

2026-10-30 继续处理 `2655ede` 的两条审查：单元格从起始就创建统一行内 builder，让 HTML 关闭标签和 Markdown 强调共享同一范围状态，关闭后的 outside 不再被链接/样式覆盖；raw-text 扫描显式返回是否真正找到结束标签，title 等丢弃元素在分段事件中保持抑制，且片段末尾的完整结束标签不会被误判为未关闭。两项均有独立回归。

所有成立的反馈按已有要求直接修复，未发生需求争议或待裁决。`delivery-state.json` 与 PR 保留 `arbitration_ever_required: false / merge_owner: worker`；任务明确要求人工验收，因此不会自行合并。

可选后续：删除不再生成的 `Block::Html` 兼容分支；正式 Shell 接线后用真实 README 验收 HTML 图片加载与账号/commit 身份。

候选教训交给本批 workflow-lessons 汇入：只限制一次性解析树不足以约束跨事件状态，必须同时限制保留栈和输出膨胀预算；抑制不可信内容必须覆盖结构输出，不能仅过滤文本。证据是本 PR 审查中实际复现的深层 HTML、跨事件标签、宽×长表格和隐藏 Markdown 图片。未在他人拥有的 LESSONS.md/工作流文档追加。
