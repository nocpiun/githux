# ui-row-links 开发日志

任务：用户问题 1（紧凑 owner/repo）、2（文本范围 hover/tooltip/click）、8（共享两行条目与选中背景几何）。分支 `fix/ui-row-links`，PR #83，First Release #1；仅协调者合并。

## 2026-11-10 首轮与 2026-11-11 验收否决

首轮提交 `2b5d7f9` 将伸缩移到链接外层 wrapper，并把斜杠两侧 gap 从 8px 改成 2px。编译、局部回归、CI 与 Codex 均通过，但负责人原生验收否决：`#1136 alibaba / open-code-review` 仍有分隔空隙，空白区仍显示仓库 tooltip，选中背景仍过高。首轮交付所称“已修复三个问题”不成立。

根因：block wrapper 内的 block 链接仍铺满宽度；标题的 `flex_1` 同样扩大命中范围。旧测试只证明 repo.right < time.left，仍允许 repo 铺满整个仓库 slot；`repo.right + 30` 的点已经跳出了错误命中框。首轮保留 72px 而没有改几何，因此无法解决问题 8。`.justify_center()` 只平衡行盒，不证明字形留白平衡。此处记录实际失败，不将旧测试通过当成视觉验收。

## 返工实现

- owner、`/`、repo 之间不加任何 gap/padding；保留独立 Profile/Repository 动作、键盘入口和 native truncation。
- 伸缩只放在非交互的横向 flex slot；子链接没有 flex-grow，短文字按 intrinsic width 排版，长文字按受限显示宽度截断。tooltip、underline、pointer 与 activation 均留在该文字元素上。标题（含无动作标题的 tooltip）应用同样布局。
- Feed owner/repo/actor 改用 native truncation；PR 标题的 slot 与文字分离；纵向容器中的 Read more 使用 self-start，避免跨整行拉伸。卡片/列表原本的选择与激活归属保留。
- Home 与全局 Issues/PR 仍消费一个 renderer，行高由 72 收紧为 56 logical px；两行 gap 3px，水平 padding 8px，metadata 起点对齐标题文字（图标槽 + gap 之后）。Noto Sans SC 的实际墨迹低于行盒中心，按原生测量对整个内容块上提 1 logical px（顶部 padding 7、底部 9），行盒上下不再强求相等；原生 raster 断言独立检查墨迹平衡。ListState 两端都引用共享高度，没有改 viewport/gutter。
- tooltip 内容增加稳定 debug selector，回归直接观察真实延迟 tooltip 的出现/消失。

## 回归设计与证据边界

`src/ui/home/row_tests.rs` 用真实 Home/Feed 树与主题 family，包含截图同形短仓库、长中英标题/仓库。独立 shaping 测量断言短 repo 的命中宽度等于文本 advance，而不是以被测错误框决定空白位置；测试指针进入真正 trailing blank、等待 tooltip、点击，再移回文字检查 tooltip/导航。覆盖标题、Feed actor/repo/PR title/Read more。缩放 1.0/1.5/2.0，宽/窄布局，检查实际行边界、紧凑留白、斜杠零布局间距和长 repo 的非零目标。

Windows 专项用 `TestAppContext::build_with_text_system` 注入平台原生字体系统，直接嵌入与生产相同的五份字体资源及同一 family 契约（Home 独立 target 无 platform 模块，避免在主程序 tests 再次 `#[path]` 加载生产 fonts 模块而触发 `duplicate_mod`）。按实际 shaped glyph、行盒和原生 glyph raster alpha 计算已显示短行的墨迹上下边界，避免只断言 style 常量。它仍是 GPUI 模拟事件，不代表 Windows 原生输入或负责人视觉验收。

`src/ui/work_item_row_tests.rs` 通过完整共享 renderer 覆盖 global Whole 模式、Issue/PR glyph、宽/窄、实际 56px 背景边界与留白。空白只触发行选择，文字只触发仓库导航，真实 tooltip 在空白不出现、文字出现。

最终本地检查见下节，远端 CI/review 与当前 head 见 PR 交付记录。保留负责人 `blocked`；准备验收时加 `pending`，不合并。字体任务先合入 main 且也使用 `2026-11-10-TASK.md`，因此本任务日志迁为当前 `2026-11-10-ui-row-links.md`，保留已验收字体日志。

- `551d4c9a`：`cargo test --bin githux row -- --nocapture`，66 passed，包含 Home/global 的既有几何与导航、原生字体回归及相关侧栏/阅读器行测试。100% 上下墨迹留白 12.17/10.20px，150% 为 12.00/10.87px，200% 为 12.42/10.45px。
- `0589a3f1`：仅逆转内部 wrapper 时 Home 仍通过，因为外层 flex 也约束了自然宽度；不是原始两层 block 路径的完整复现。
- `2e9975bd`：两层 wrapper 都逆转为首轮 block 布局后，`row_acceptance_real_render_tree` 在真实指针移入独立 shaping 定位的空白处失败：`repo hovered on blank`。随后立即恢复修复。新回归能识别原误触区，而不是跳到错误 hitbox 右边再断言无动作。
- `0a3d2ed7`：`cargo test --locked --bin githux`，1233 passed，无失败/忽略，含正式 Shell 与页面既有回归。
- `52fa08ea`：首轮 all-target Clippy 因测试再次 `#[path]` 加载 `platform/fonts.rs` 触发 `duplicate_mod` 失败。已改为测试 fixture 直接嵌入同一组生产字体资源，无 lint 豁免。
- `4ea7c432`：字体测试装配调整后的 `cargo test --locked --bin githux row_acceptance`，2 passed，包含原生 DirectWrite、完整 Home/Feed hover/tooltip 与 DPI/宽窄布局。只改测试装配，因此复用此前 1233 项完整测试证据，没有再重复全量套件。
- 最终回归另覆盖 Feed 时间戳 tooltip 的实际范围、Home 两个相邻 item 背景边界连续、每个 Feed 文本目标从空白移入文字后的真实 hover 与 tooltip。

迭代证据：`ab139e38` 在原字体下真实 raster 余量 13.54/9.73px；接入 #78 全量 SC 后 `e6e0b822` 检出 13.17/9.20px。两次都被新增墨迹平衡断言拒绝，随后增加 1px optical lift；未通过修改阈值把失败改为通过。第一次 `6526afe5` 的无限余量暴露 Windows headless 使用假字体系统，已改为 windowed platform 的 DirectWrite text system；无实际 alpha 墨迹时测试失败。

## 最终本地检查与交接边界

集成基线为 main `8cad76b`，包含负责人已验收并合入的 #78 字体、#79 Profile、#81 图片布局。2026-11-11 本轮交付前再次 fetch 时 #80 仍为 OPEN（head `98b890e9c5bfbe591f3c63131227d98966156f27`），尚不能从 main 接入；不从 sibling worktree/未合并分支取代码。若 #80 随后合入，最终集成需按新基线补组合验证。

所有 Cargo 操作经过 `scripts/verify.mjs`，进程级 `CARGO_PROFILE_DEV_DEBUG=0`、`CARGO_PROFILE_TEST_DEBUG=0`、`CARGO_INCREMENTAL=0`，无并发重编译。

- 完整主二进制测试 `0a3d2ed7`：1233 passed。
- 测试装配调整后定向回归 `4ea7c432`：2 passed。
- `cargo clippy --locked --all-targets -- -D warnings`，`5ddb40d2`：SUCCESS。
- `cargo build --locked --bin githux`，`6ea46f4d`：SUCCESS，产物 `target/debug/githux.exe`。
- `cargo fmt --all -- --check`，`6aafa658`：SUCCESS。
- 本轮只在实际修改、迭代失败或基线改变时重跑对应检查，没有为了日志检索重编译。

启动：在本 worktree 执行 `scripts\\run-githux.cmd target\\debug\\githux.exe`。本轮未以真实账户启动应用，未取得原生窗口截图或真人输入验收；原生字体 shaping/raster + GPUI 模拟事件通过，不等于负责人视觉验收通过。PR #83 保留负责人 `blocked`，准备验收后添加 `pending`，不合并。最新 head 的 CI/Codex 收据与最终状态留在 PR 及私有 delivery-state，不为补记结果制造额外提交。

## 可复用教训候选（供协调者汇总）

文本链接修复不能止于“把 flex_1 移到 wrapper”。必须检查 wrapper 的 display 与子项 sizing：block 子项可能继续 stretch。回归应以独立 shaping 宽度定位文字末端，再验证真实 hover/tooltip/activation；用错误元素自身 right 边界选择点，会把误触区一起跳过去。行盒余量平衡也不能替代字体墨迹或原生画面证据。
