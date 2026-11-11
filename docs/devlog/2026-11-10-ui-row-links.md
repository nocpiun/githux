# ui-row-links 开发日志

任务：用户问题 1（紧凑 owner/repo）、2（文本范围 hover/tooltip/click）、8（共享两行条目与选中背景几何）。分支 `fix/ui-row-links`，PR #83，First Release #1；仅协调者合并。

## 2026-11-10 首轮与 2026-11-11 验收否决

首轮提交 `2b5d7f9` 将伸缩移到链接外层 wrapper，并把斜杠两侧 gap 从 8px 改成 2px。编译、局部回归、CI 与 Codex 均通过，但负责人原生验收否决：`#1136 alibaba / open-code-review` 仍有分隔空隙，空白区仍显示仓库 tooltip，选中背景仍过高。首轮交付所称“已修复三个问题”不成立。

根因：block wrapper 内的 block 链接仍铺满宽度；标题的 `flex_1` 同样扩大命中范围。旧测试只证明 repo.right < time.left，仍允许 repo 铺满整个仓库 slot；`repo.right + 30` 的点已经跳出了错误命中框。首轮保留 72px 而没有改几何，因此无法解决问题 8。`.justify_center()` 只平衡行盒，不证明字形留白平衡。此处记录实际失败，不将旧测试通过当成视觉验收。

## 返工实现

- owner、`/`、repo 之间不加任何 gap/padding；保留独立 Profile/Repository 动作、键盘入口和 native truncation。
- 伸缩只放在非交互的横向 flex slot；子链接没有 flex-grow，短文字按 intrinsic width 排版，长文字按受限显示宽度截断。tooltip、underline、pointer 与 activation 均留在该文字元素上。标题（含无动作标题的 tooltip）应用同样布局。
- Feed owner/repo/actor 改用 native truncation；PR 标题的 slot 与文字分离；纵向容器中的 Read more 使用 self-start，避免跨整行拉伸。卡片/列表原本的选择与激活归属保留。
- Home 与全局 Issues/PR 仍消费一个 renderer，行高由 72 收紧为 56 logical px；两行 gap 3px，水平 padding 8px，metadata 起点对齐标题文字（图标槽 + gap 之后）。ListState 两端都引用共享高度，没有改 viewport/gutter。
- tooltip 内容增加稳定 debug selector，回归直接观察真实延迟 tooltip 的出现/消失。

## 回归设计与证据边界

`src/ui/home/row_tests.rs` 用真实 Home/Feed 树与主题 family，包含截图同形短仓库、长中英标题/仓库。独立 shaping 测量断言短 repo 的命中宽度等于文本 advance，而不是以被测错误框决定空白位置；测试指针进入真正 trailing blank、等待 tooltip、点击，再移回文字检查 tooltip/导航。覆盖标题、Feed actor/repo/PR title/Read more。缩放 1.0/1.5/2.0，宽/窄布局，检查实际行边界、紧凑留白、斜杠零布局间距和长 repo 的非零目标。

Windows 专项用 `TestAppContext::build_with_text_system` 注入平台原生字体系统，加载生产字体注册函数。按实际 shaped glyph、行盒和原生 glyph raster alpha 计算已显示短行的墨迹上下边界，避免只断言 style 常量。它仍是 GPUI 模拟事件，不代表 Windows 原生输入或负责人视觉验收。

`src/ui/work_item_row_tests.rs` 通过完整共享 renderer 覆盖 global Whole 模式、Issue/PR glyph、宽/窄、实际 56px 背景边界与留白。空白只触发行选择，文字只触发仓库导航，真实 tooltip 在空白不出现、文字出现。

最终检查、最新 main 组合与 PR head 证据在本轮完成后记录。保留负责人 `blocked`；准备验收时加 `pending`，不合并。

## 可复用教训候选（供协调者汇总）

文本链接修复不能止于“把 flex_1 移到 wrapper”。必须检查 wrapper 的 display 与子项 sizing：block 子项可能继续 stretch。回归应以独立 shaping 宽度定位文字末端，再验证真实 hover/tooltip/activation；用错误元素自身 right 边界选择点，会把误触区一起跳过去。行盒余量平衡也不能替代字体墨迹或原生画面证据。
