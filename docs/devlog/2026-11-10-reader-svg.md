# README SVG 渲染与有限 CSS 动画 (`reader-svg`)

- 任务 ID `reader-svg`，同一分支 `fix/reader-svg`、同一 [PR #82](https://github.com/nocpiun/githux/pull/82)。初始 base `0c619070be103048f9aa033f2f57520372cbebb2`；2026-11-11 返工已整合 main `f9e31a07ad3c5352eebd803592d420d624dc2bf1` 的已验收字体、Profile、图片布局、tabs 与行链接；未引入未合并的 #84 viewport padding 方案，不修改页面宿主或缓存首屏任务的文件。不合并 PR，`merge_owner=coordinator`。
- 状态：**2026-11-11 负责人否决初次静态交付，返工中**。此前 clean Codex/CI 和有效 PNG 不能证明实际 Profile 内容正确，也不构成人工验收。以下初次静态实现与检查为历史记录；当前行为契约以 [README 图片](../reader-images.md) 为准。

## 2026-11-11 再次品质返工：同字节参考、设备密度与静默加载

负责人反馈 opanel-mc/opanel Star History 几何/标签失真、Profile 卡片仍模糊，并要求所有 pending 图片不占位。继续原会话、分支和 PR；表格图片另属 `reader-table-images`，本轮不修改其 AST、DocText、Table 分支/测试，也不修改页面宿主或 viewport geometry。最新旧 head 审查 `5859062688` 的两条 P2（显式 transform none 与 dashoffset px）合并处理；负责人尚未验收，合并归协调者。

同一响应比较：从 README 的实际 Star History URL 取得 65,171 B SVG，SHA-256 `2af50869a89d82f9858924efb1c621ba0d6eac9cf712718b619d099064ccd95f`，只保存在本地调查目录；用相同字节生成 Chromium 1×/1.5×/2×/3× 参考。查询含 token 的 URL 未进入提交/文档。原图确有手绘噪声，未拉直作者几何。具体偏差有二：内嵌 xkcd WOFF1 原本被忽略；resvg 0.46.0 调用方已经转换 displacement scale，底层又乘一次，使源 scale 5 实际成为 25，负值符号也丢失。密度探针随后复现 1.5× 下 SourceGraphic 与位移图因滤镜区域取整相差一像素、触发库的等尺寸断言；采样改为遍历真实输出、使用每个输入自己的 stride/bounds，越界透明且不增添工作面。原创小数几何与超过内部画布裁切的最小滤镜分别覆盖两条边界。首轮字体/滤镜修补后，同尺寸 RGB 平均绝对差由 8.2807 降至 3.9266；这是中间 1× 对照，不替代最终密度探针或人工验收。roast 内两张 PNG 的实际网格为 96×96、32×32，作者盒分别 32×32、13×13；SVG 文字/线条可重新光栅化，但超出这些内嵌位图原生像素的细节不能凭空恢复。

根 Cargo patch 使 GPUI 和 Reader 共用 `vendor/resvg` 精确发布包，仅修改 displacement 的比例/网格采样算术并带修改声明，保留原 metadata/示例/双许可全文及 PATCH.md。字体从受限 data URL 加入私有内存 fontdb，WOFF1 有界逐表解包/校验、保留 CSS family alias，单输入 1 MiB、根与内嵌共用解包 4 MiB。拒绝原始和 WOFF flavor 伪装的字体集合，SFNT 表/名称数量与名称字符串累计工作独立限制，防止小字节头描述巨大解析/分配工作；无系统扫描、文件或网络字体读取。提交的字体仅为现有 Google Sans Code 的改名 OFL 测试子集，公开原图/字体不提交。

新增兼容 `ImageProvider::start_with_options`，默认委托旧 start，ImageRequest 与宿主接口不变。Reader 初始化/绘制取设备比例，生产 provider 后台解码使用真实比例：入场帧目标为设备像素密度，最终图额外 2× 超采样，作者逻辑宽高不变。最终图、保留帧、滤镜工作面共同受预算钳制；不足时先降低时间采样数并保持稳定帧网格，极端情况下才最终静态 fallback。更高密度保留旧图至后台完成，不重播入场，准确替换保留成本；低密度重用清晰网格，PNG/GIF 不无效重载。真实失败（含 session 失效）保留诊断并释放旧成本。rem 只缩放文字；未来若有整体图片 zoom，须显式传入有效比例。高 DPI 大图/滤镜可能仍低于目标网格，这是实现预算边界。

合法图片未请求/排队/Loading 时零高度，无盒、alt 标签或 140px 面板；真实失败/拒绝仍显示原因。构建块先登记需求，再从 flex 图片行去掉 pending 子项，避免零宽子项仍制造 gap；全 pending 块归零。调度不依赖图片本身正面积，Ready 的可见交集只用于开始动画；完成后复用虚拟项重测/锚点或 Flow 高度通知，alt 继续存在于复制文本。原有保留队列去重已存在，密度升级复用它而非计成第二张图片。

本轮最终验证：完整图片组串行 `f4a448a7-087a-4fdd-8deb-080c6fd21e07` **832 通过**（1 个 opt-in 忽略）；Repository/overview `a1ac2807-6e04-4962-98c2-34a57d8b9537` **922 / 44 通过**。随后发现的滤镜网格边界/WOFF flavor 修正由 `fbcfab9f-dfb6-42e3-be0d-dfd5e00ef444` **16 项品质回归通过**；最终 all-target clippy `4842e13e-b224-4b51-bcb6-59431bd638c6`、正式 app/example build `8fcfd73b-78c1-4e18-a7e9-2ae5c0cc0663`、fmt check `5f90f4fc-3386-417e-9c0c-e8bea14bc504` 和公开探针 `0a05483a-2c26-4b31-b55e-e8746bf9a609` 全部通过。较早全套前六组 1280 / 103 / 626 / 330 / 295 / 830 通过；两次并行测试分别命中未修改的 loopback framing/chunked 用例 `error sending request`，隔离与后续串行组内复测通过，保留失败记录而未调整网络实现。新回归覆盖 data 字体实际选字/解包与解析工作上限、正负位移、transform none/translateX 与 px dashoffset、1–4× 物理网格/最终成本、DPI 旧 scope/保留替换/失败、PNG/GIF 不重载、pending 行间空隙，以及虚拟/Flow 延迟完成和纯图片零尺寸调度。

最终公开探针仍使用相同本地响应，Stats 的 467×195 逻辑盒不变：

| 设备比例 | 入场网格 | 保留最终网格 | 入场样本 | 保留 BGRA 总字节 |
| --- | --- | --- | --- | --- |
| 1× | 467×195 | 934×390 | 27 | 11,292,060 |
| 1.5× | 700×292 | 1401×585 | 27 | 25,353,540 |
| 2× | 934×390 | 1868×780 | 15 | 27,683,760 |

roast 最终网格依次 880×400 / 1320×600 / 1760×800，逻辑盒仍 440×200。Star History 的全画布滤镜受工作面预算限制，最终网格依次 1295×863 / 1275×850 / 1377×918；2× 下低于参考物理大小，这是记录在案的实现预算限制。完整浏览器截图尺寸上的 RGB MAE 约由 8.28–8.34 降到 5.13–5.33，上文首轮数值来自较早的 1× 调查；本轮明确按完整浏览器截图尺寸比较，两组数字不混用为验收门槛。参考卡片显式采用 dark color-scheme，匹配 Reader 当前采用的源基础配色；条件 CSS、浏览器字体合成和抗锯齿仍可能不同。三张 browser/旧/新对照与指标 JSON 留在本地质量目录，公开素材不提交。

当前原生 PrintWindow 录像仍是本次品质返工之前的历史证据。本轮 headless Chromium 对照、测试平台生命周期与解码探针均不构成真实账户 Profile/手动输入验收。当前实现与预算详见 [README 图片](../reader-images.md)；必须获得新 head CI/review，仍不合并。

### 品质审查：嵌套矢量目标画布预算

`9ec716c` 的审查 `5861251870` 提出 P2 `4539863427`：resvg `image::render_vector` 为每级内嵌 SVG 分配当前目标尺寸的 pixmap，旧估算只遍历内嵌 root 的固有 bbox，漏掉了该完整画布。现按每级实际目标（包括隔离组目标）计费，传递 SVG 树之间的祖先变换与 renderer 上限；同层顺序图片取峰值，嵌套路径累计父子共存表面。1000×1000 的四级 1×1 资源会降低物理网格以容纳全部表面，1400×1400 在原生网格仍超限时分配前拒绝，带 opacity 的子树也覆盖放大后的隔离表面。

同时核对到滤镜 primitive 使用完整转换后的 region，和被内部画布上限裁切的 group layer 不同。工作面计费不再把 layer 裁切错误用于 filter region；原创极小画布/巨大滤镜区域回归在危险分配前拒绝。未修改 renderer 算术、页面接口或表格范围。完整组还发现前一轮 SFNT 签名保护把未知、未使用的 data 字体扩大成整图拒绝；现先忽略未知格式以保留原 fallback，支持格式继续走预算校验，原始与 WOFF flavor 伪装的字体集合仍拒绝。

审查修正验证：`733abb1d-baf2-414b-a892-37b4dab86354` 完整图片组串行 **836 通过、0 失败、1 opt-in 忽略**；`c6722e4a-f0db-4716-a56c-f8f027ab64ce` **18 项品质回归通过**。all-target clippy `b77eb91a-df96-4411-9144-ba89caeab3a9`、app/example build `61707be8-a017-4e87-ba3f-61e8bd04660c`、公开密度探针 `72d3b1b2-b4dd-450b-9c1c-7e88266b74b7` 与 fmt check `dc322052-6c65-4b81-8528-5b69dc1b46b4` 全部通过。1×/1.5×/2× 共 21 张公开探针 PNG 与 `9ec716c` 品质实现字节相同，预算修正没有改变已观察样本的像素。随后发布的 head 仍须重新获得 CI/review，负责人验收未完成。

### 品质审查：动画组件级联与重复 offset

`6c1c1b3` 审查 `5861847433` 的 P2 `4540344965` / `4540344974` 均有效：旧实现把 animation shorthand 优先于最终 animation-name，也会让重复 100% 的第一段遮住后续覆盖。现将 shorthand 组件分别纳入声明顺序、specificity、important 级联，保留原始值直到节点变量环境计算后再提取组件，并恢复省略 longhand 的默认值；组件副本有分配前预算。相同 keyframe offset 先按源码顺序逐属性合并，负零与零规范为同一停点，保留同 offset 未被覆盖的其他属性。回归检查最终/中间可见像素、shorthand/longhand 顺序、规则优先级、important、变量、方向/次数覆盖和默认值重置。

本轮验证：完整图片组 `89fe63c0-3933-42db-b82b-72511112c566` **839 通过、0 失败、1 opt-in 忽略**；品质回归 `a1e6b38e-c992-4b3d-9ff4-6122d9736adc` **21 通过**。all-target clippy `7ec872a3-d8a1-4669-846c-34a6b2ae7589`、app/example build `b051492b-77d9-402a-85ea-24cd6a6630ca`、公开密度探针 `05e59165-0a8b-4d25-9c28-278cdf8b6b21` 与 fmt check `97d10aab-318e-414f-bc1e-52a1f8b930ac` 均通过。21 张公开 PNG 与前一轮逐字节相同；随后新 head 重审，不据旧 head CI/review 放行。

### 品质审查：根伪类 specificity

`8448eb1` 审查 `5862255535` 的 P2 `4540685483` 有效：把 `:root` 改成虚拟类型选择器降低了优先级，`:root{opacity:1} svg{opacity:0}` 会错误隐藏整图。现仅改写 selector header 中真正的 root 伪类 token，使用同等 specificity 的虚拟属性匹配；支持 compound `svg:root` 和大小写，不改 quoted attribute、escape 或 declaration payload。回归检查类型/class/id/important/source-order 竞争、根外不匹配、后代优先级及含 `:root` 的 gradient URL。

本轮根选择器验证：完整图片组 `dc65d5cb-0937-4840-a2a0-66e4ea22c73b` **841 通过、0 失败、1 opt-in 忽略**；品质回归 `bd65eb60-ddb2-4ae2-aa7c-1ff5e2b05d55` **23 通过**。all-target clippy `3bfbb443-c337-48a2-be28-9975d665742b`、app/example build `c4d5a231-bcd6-4a0e-a56a-105534a4e027`、公开密度探针 `e9b82fc5-0f70-4f4a-aac4-a728fcdcb0d0` 与 fmt check `2c199493-09e7-40fb-b340-a6af99503a15` 全部通过。21 张公开 PNG 与前一轮逐字节相同；发布新 head 后重新 CI/review，人工 Profile 验收仍待负责人。

## 2026-11-11 负责人反馈后的集成返工

负责人提供了真实 Profile README 与三份公开 SVG 响应，先读取本地证据再实现，没有把原始响应或用户 Profile 复制进 Git fixture：

- Stats 的 `.stagger` 基础 opacity 为零，依赖 `fadeInAnimation`、450–1050ms 行延迟、forwards fill 才显示；rank 还有 translate/scale 与 stroke-dashoffset 时间线。静态 resvg 会永久保留隐身基础状态。
- Shields badge 的 logo 是 `data:image/svg+xml`；初次 resolver 拒绝全部嵌套 SVG，所以文字/背景出现而图标缺失。
- 440×200 roast 是普通 SVG，使用 CSS 自定义属性与 fallback、CJK 字体和内嵌 PNG；不能归因成 foreignObject 或 HTTP 失败。
- 已验收 #81 解决比例钳制、成功图片背景和作者尺寸提示。残留的一徽章一行来自 AST 把同一段落的图片拆成独立块；本轮保留 #81 的几何语义，并在模型中保留纯图片段落和居中作用域。

实现采用受限声明式时间线，不引入浏览器、不执行脚本、不按主机名或内容打补丁：

1. `timeline.rs` 使用 simplecss 的 selector matcher 与有界 CSS declaration/keyframe 扫描；支持所供样本的 delay、easing、fill-mode、opacity、translate/scale 和 dashoffset。自定义属性、fallback、`!important` 和静态隐藏独立处理并移除已完成 cascade 的原样式表，避免 usvg 自己的 important 优先级再次覆盖采样/根可见性。simplecss 的 CSS 2.1 declaration parser 不接受 `--name`，由实测中 fallback 色与变量色不同的失败断言确认，因此只复用其 selector 能力，变量声明另作有界解析。
2. 后台准备最多五秒/100 帧，最终 PNG 仍在 `DecodedImage` 原字段中；新增可选 `animation` 元数据携带有限 PNG 帧与时间戳，四个 raster 格式变体不变。帧总额计入图片预算；取消/截止时间在各帧之间检查。超时长/超预算的受支持动画使用最终静态 fallback，并明确不是动画验收结果。
3. 内嵌 SVG 同样验证 XML、主动内容与外部资源边界；递归、节点、字节、累计解码成本都有上限。GIF 先受限解码到 PNG，防止 frame 网格与逻辑头不一致。绘制前检查 canvas/隔离组/滤镜 scratch surfaces，避免小画布叠大量 opacity 层绕过分配预算；超采样可退回固有网格。未能约束 tile 分配的 SVG patterns 暂不接受。
4. Reader 只选择单个、已解码 BGRA8 的 `RenderImage`，复用既有 pump；中间帧不重测尺寸。第一次 Windows 启动序列的原生捕获发现，逐帧切换 PNG `Image` 会让 GPUI 每次异步解码，产生整张卡片短暂消失的闪烁；因此像素在 provider 中一并准备，UI 只封装现成 buffer，包括最终帧，不在每拍重新解码。已保留帧与当前 scratch 工作面共同扣减解码预算。实际可见绘制开始时间线，完成、reduced motion、暂停/离屏后转为最终 PNG 并释放其余帧。替换文档/账户不保留旧帧。
5. `ImageRow` 保留纯图片段落的软换行；空行另起段落。`Centered` 在绘制前仍按段落拆成独立虚拟项，标题和文档 unit 的顶层索引随之重建；复用普通标题/正文 renderer，Ready 不再强制 alt caption。原生外层滚动容器复核暴露了 flex 块宽度随内容收缩、导致居中失效的问题，显式占满列宽后修复，并把回归改成真实外层 scroll host 结构。#81 的透明、native/requested sizing 与缩小适配保留。
6. Star History 的真实响应声明根 `background:#fff`；resvg 接受 `background-color` 而不接受这条 shorthand，原来黑色标签会落在透明背景上。颜色 shorthand 规范化后使用库本身的背景绘制；普通透明图不补背景。根背景与前景由库分别处理；需先记录声明的根 opacity/display，在库输入中中和前景根 opacity，再对包含背景的最终表面应用一次整体声明值，避免前景透明度重复相乘；根 visibility 隐藏时不绘制根背景。回归覆盖黑色文字对比、静态隐藏子节点、根 opacity/display/visibility、半透明白纸与重要背景声明，未强行把 opacity=0 改成 1。
7. 接手并补齐 resvg/usvg/tiny-skia 许可文本、哈希与 notices；保留 #78 的 Noto Sans SC 字体 notices。simplecss/svgtypes 复用 usvg 的既有锁定版本。

已完成的本地调查（不等于原生验收）：公开三份响应经过当前解码器的 probe，Stats 为 467×195、27 个采样帧 + 最终帧、10,199,280 decoded bytes；600ms 只有首个延迟行部分出现，最终全部行/值与 rank 可见；roast 为 440×200，CJK 文案、雷达、头像均可见；badge 为 83×20，JS logo 可见。另将 README 的其余徽章无凭据读取为本地响应集合，供 21 个真实图片 URL 的离线原生组合检查，未提交用户素材。

本轮回归断言真实内容的像素和布局：中间/最终行、数值文本区域、rank 缩放、隐藏区域、变量色、logo 空心透明细节、节点/资源/隔离层预算、取消、离屏/暂停/文档切换、缓存重开后的动画，以及行内徽章的 x/y/尺寸/段落间距。回归通过原生窗口捕获发现的空白闪烁、Flow 宿主下的居中、根背景与重要声明再 cascade 问题后，最终 focused 检查基于 main `f9e31a0`，不沿用初次静态交付的通过状态。

| 检查 | 结果 | runner run |
| --- | --- | --- |
| `cargo test --locked --test reader_images` | 818 passed，0 failed，1 个公开样本 probe 默认 ignored | `c6caa267-e73b-4bae-82e1-3c5d8f86b0b3` |
| `cargo clippy --locked --all-targets -- -D warnings` | 全目标通过，零警告 | `3da38593-fe1c-44cd-a8f3-29ff6c93bd15` |
| `cargo build --locked --bin githux --example reader_images` | 正式应用与预览通过 | `198b5215-b24a-464e-801b-509f7f57f2cb` |
| opt-in `public_svg_probe -- --ignored --nocapture` | 所供 Stats/badge/roast 与新取 Star History 响应均成功，七张 PNG 与接续前逐字节相同 | `b2599582-e77a-44a3-a14d-8cfbaf26d068` |
| `cargo fmt --all -- --check` | 通过 | `bfb26496-7cd3-4c92-b8bc-fe97577e06ae` |

中途在 main `54c8afbc` 集成点运行的全套 `cargo test --locked` 也通过：1255 / 98 / 607 / 310 / 275 / 806 / 901 / 44（run `7d23115a-30c6-4d16-8b11-9ea012c5a5a7`）。它早于最后的 Flow 宿主宽度/根样式修复与 #83 同步，不冒充最终提交的全套检查；最终提交仍须获得新的 CI 与 Codex 审查。

### 原生组合证据（2026-11-11）

构建 `githux.exe` 与 `reader_images.exe` 后，在 Kero project `m5d2w59` 的唯一任务 helper 中运行 `reader_images "SVG profile"`；临时 fixture 账户使用合成 token，21 个公开 URL 全部从本地响应 manifest 应答，无实时网络、无真实 PAT。所供 README 原文与公开素材未纳入 Git。

- Windows 150% DPI，原生窗口为 1792×1286 设备像素。普通 screen-copy 捕获到了 Windows 锁屏，未作产品证据；改用仅对本任务窗口的 `PrintWindow(PW_RENDERFULLCONTENT)`，并将捕获线程置为 per-monitor DPI-aware，避免把虚拟化坐标下的半幅截图误判为布局右移。不解锁桌面、不操作其他用户窗口。
- 第一次启动序列明确暴露逐帧 PNG 异步解码闪烁；改成预解码 BGRA8 后重建并重新采集 60 帧。Stats 首个可绘制帧到最终帧为 **1,334ms**（CSS 时长 1,350ms，含采样/捕获误差），随后 **4,157ms** 的所有 Stats 区域逐像素一致；从首次出现到最后，卡片背景始终存在，没有先前的整图空白闪烁。捕获中可辨认标题渐入、rank 缩放/arc 进度和延迟统计行依次显现。
- 同一真实窗口中 roast 的中文、雷达、头像与页脚细节可读；Stats 全部行/数值与 rank 保留；19 枚徽章按所供 README 的四个段落成组居中，logo 可见，成功图片没有 alt 图注或 140px 段间占位。后面的 Links 回到普通左对齐。
- 本地复核产物：`.agents/task/native-svg/ready-profile-final.png`、`ready-profile-animation.gif`（一次播放，按捕获文件时间间隔导出）、`evidence.json`；原始 60 张 `opacity-review-native-*.png` 捕获留在同目录（main `f9e31a0`、build run `064040d8`，helper session `zirwkhm`）。它们是本机调查证据，不是被 Git 跟踪的测试前置，也不是人工验收通过记录。

验证边界：原生渲染捕获成功；桌面保持锁定，未宣称真实鼠标/键盘手动验收、真实 PAT 的正式 Profile 页面 dogfooding 或跨平台原生验收。正式应用另做构建与测试；最终接受/解除 blocked/合并仍由负责人或协调者执行。

## 返工后的审查修复（2026-11-11）

首轮 head `00a107aa` 的 Codex review `5856080149` 提出一条 P2（`4535769796`），核实成立：基础 CSS 属性已解析变量，keyframe stop 还保留原始 `var()`，导致 `to { opacity:var(--visible,1) }` 插值失败、最终仍隐藏。现在按每个动画节点的继承变量环境解析每个 stop，并在插入属性前累计展开字节，沿用两 MiB 上限。回归同时断言默认 fallback 与祖先定义 `.5` 的两个节点，在开始/中间/最终时刻分别为 alpha `(0,0)` / `(128,64)` / `(255,128)`。重新跑 808 个 Reader 测试、全目标 clippy、构建与公开 probe 全通过；七张公开 probe PNG 与修复前逐字节一致，并以修复后构建重新采集上述 60 帧原生序列。没有裁决事项，合并仍由协调者负责。

第二轮 head `394a4283` 的 review `5856283143` 提出 P1 `4535932686` 和 P2 `4535932697`，均核实成立并修复：变量串递归拼接可能先分配巨型中间值，现向同一输出缓冲直接展开，按当前剩余限额在申请/追加前检查，保留深度上限；811 项 Reader 回归包含 1 MiB inline seed 的三倍递归链和低限额下输出长度/容量不越界的断言。普通 `transform` presentation 属性现在保留为基础值，延迟前/none fill 结束后恢复；像素回归同时验证基础位置、动画替换位置与最终恢复位置。重新通过上表的测试、全目标 clippy、构建、probe 与格式检查，七张公开 probe PNG 未改变，并再次从新构建采集 60 帧原生证据。

第三轮 head `84ae25bd` 的 review `5856517624` 提出 P1 `4536113431` 与 P2 `4536113441`，同样核实成立：空变量十倍递归并不产生输出，字节限额不能约束 CPU；现对整次时间线构建共享 100,000 步工作限额，空值计费、每 128 步检查取消/截止时间，节点遍历也检查。回归证明零输出展开耗尽工作限额，并能在限额耗尽前取消。仅有 `to` / `from` 的 keyframes 则从元素基础 CSS/presentation 值补齐缺失端点，继承属性按祖先基础值查找，并按各属性自己的停点插值；回归验证开始/中间/最终 alpha `(0,255)` / `(128,153)` / `(255,51)`。本轮 813 项 Reader 测试通过；随后按 clippy 建议把取模判定改成等价 `is_multiple_of` 并通过最终全目标 clippy、构建、probe 与格式检查。公开七张 PNG 仍逐字节相同，原生序列再次从最终构建采集。

第四轮 head `611f04c8` 的 review `5856997279` 提出两条 P2（`4536496340`、`4536496346`），核实并修复：opacity/fill-opacity/stroke-opacity 百分比端点先归一化为 0–1 再插值；局部自定义属性在声明元素的完整变量环境中先计算，子元素继承计算值，不能用自己的变量覆盖反向重绑定祖先表达式。回归覆盖三种百分比透明度的初始/中间/最终 alpha，以及祖先红色派生变量、子级蓝色覆盖/本地重新声明、动画继承 `.5` 端点的区别。最终 815 项 Reader 测试及上表全部检查通过；七张公开 PNG 仍相同，并以当前构建刷新 60 帧原生序列。

第五轮 head `3bb24f51` 的 review `5857437716` 提出 P2 `4536852898`（shorthand 方向）与 `4536852909`（内嵌根半透明），均核实成立。按 2026-11-11 clean-continuation 检查点保留原 Pi 会话/分支/worktree，未因重复协调消息重做实现。方向 tokens 中 reverse/alternate/alternate-reverse 使用与 longhand 一致的静态回退；内嵌分数透明度通过仅服务已验证 Tree 的一次性内部 resolver、矢量 image 包装整体合成，保留分数固有尺寸，新增节点/隔离层仍计入预算。像素回归发现库前景已有根 opacity、根背景却在不同路径处理，因此先记录声明并中和库输入前景的根 opacity，再对整体应用一次；同时保留直接子元素显式 `opacity:inherit` 的原计算值，避免二次相乘或继承丢失。测试覆盖重叠路径、两层透明度相乘、顶层/内嵌一致性、分数尺寸及显式继承。本轮不新增桌面验收；原生捕获的构建/边界仍按上文原始记录，公开样本另作回归比对。最终 818 项 Reader 测试与上表检查通过。首次测试曾出现无关的 `length_is_only_a_hint_and_malformed_framing_cannot_bypass_bound` loopback `error sending request`；隔离重跑 `e63e48b0`、随后整组 `aca3d0d4` 和最终 `c6caa267` 均通过，按 L002 保留环境抖动记录，未修改网络模块。

## 初次静态交付的改动与决定（历史）

### 1. 光栅化库选型与依赖

- 选用 `resvg = "0.46.0"`（`default-features = false`，仅开启 `text`、`raster-images`），与 `Cargo.lock` 中 `gpui-pre 0.3.10` 已锁定的 `resvg 0.46.0` / `usvg 0.46.0` / `tiny-skia 0.11.4` 完全一致，未引入版本分叉。
- 关闭 `system-fonts` 与 `memmap-fonts`，把应用已打包的 `Githux UI` Regular/Bold（Noto Sans + Noto Sans SC）与 `Google Sans Code` 注入进程内懒加载的 `usvg::fontdb::Database`，并将通用字体族映射到应用内置字体；500/600 字重由 fontdb 选最接近的常规/粗体面。SVG 解码不读取磁盘字体目录、不使用外部资源目录（`resources_dir = None`）。

### 2. 公开样本形态核查与确定性测试边界

2026-11-10 实地核查公开 SVG 响应特征（仅用于确认真实响应形状，不作为自动化解码通过的依赖）：
- Shields.io 徽章（`https://img.shields.io/badge/build-passing-brightgreen`、`.../license-MIT-blue?style=for-the-badge`）：返回 `Content-Type: image/svg+xml;charset=utf-8`，带 `width`/`height`、`linearGradient`、`clipPath`、`filter`/`feGaussianBlur`、`shape-rendering="crispEdges"`、`<text transform="scale(.1)" textLength=...>` 与 `Verdana,Geneva,DejaVu Sans,sans-serif`。
- Star History 图表（`https://api.star-history.com/svg?repos=torvalds/linux&type=Date`）：返回 `width="800" height="533.333"`、`<style>` `@font-face`（`data:application/font-woff;base64,...`）、`feTurbulence` + `feDisplacementMap` 滤镜、路径/文本及内嵌 `data:image/jpeg;base64,...` `<image>`。
- `github-readme-stats.vercel.app` 公开示例当时返回 `DEPLOYMENT_PAUSED` 纯文本，因此统计卡片形态按经典 SVG `<?xml version="1.0"?>` + `viewBox` + `<style>` CSS 类 + `<text>` + `<circle stroke-dasharray>` 构造本地确定性 fixture。

### 3. 安全、预算与缓存兼容性

- **保持 `DecodedImage` 契约不变**：`src/content/images/decode.rs` 对 SVG 负载在 `ImageLimits` 预算检查后以最大 2x（受 `max_dimension` / `max_pixels` / `max_decoded_bytes` 钳制）光栅化为 PNG，并保留 SVG 固有 / `viewBox` 宽高（`width`、`height`）。外部 `DecodedImage`、`ImageFormat` 与所有 UI 渲染路径无需新增 `Svg` 分支。
- **有界性与拒绝语义**：
  - 响应字节上限检查先于一切 XML/SVG 解析；
  - 禁用 DTD/实体展开（`allow_dtd: false`）、节点上限 `50_000`（超限报 `TooLarge`）、嵌套深度上限 `256`（超限报 `Malformed`）；
  - 显式拒绝 `<script>`、`on*` 事件属性、`<foreignObject>`、`@import` 样式与 SMIL 动画元素（`<animate>`/`<animateTransform>`/`<animateMotion>`/`<set>`），报 `ImageFailure::Unsupported`；
  - 零尺寸、无固有尺寸/`viewBox`、非 UTF-8 或畸形 XML 报 `ImageFailure::Malformed`；
  - 逻辑尺寸与像素数先于 pixmap 分配校验（超限报 `TooManyPixels` / `TooLarge`）；编码后 PNG 字节受 `max_response_bytes` 约束。
- **无外部 I/O**：`options.resources_dir = None`、`resolve_string` 恒返回 `None`、`resolve_data` 仅允许有界（≤2 MiB）内嵌栅格 data URI（不允许嵌套 SVG data URI）。测试中用本地未连接文件与本地未 accept 的 `TcpListener` 验证解码过程不读本地文件、不发起网络连接。
- **网络与缓存路径**：`src/network/reader_images.rs` 将外部请求的 `Accept` 头补充 `image/svg+xml`；`RepositoryImageService` 沿用原有固定 commit 解析、授权复核、`no-store` 失效、私有仓库隔离与 `CredentialRejected` 隔离。

## 文件改动

- `Cargo.toml`、`Cargo.lock`：添加直接依赖 `resvg = { version = "0.46.0", default-features = false, features = ["text", "raster-images"] }`。
- `src/content/images/decode.rs`、`src/content/images/mod.rs`：实现有界静态 SVG 光栅化与 XML/特性安全校验，更新文档注释。
- `src/content/images/tests.rs`：保留原有栅格、预算与失败回归，更新 SVG 测试覆盖徽章、统计卡片、Star History 图表、XML 头/`viewBox`/BOM、畸形/DTD、超限、禁用特性及本地文件/网络不可达证明。
- `src/network/reader_images.rs`：外部请求 `Accept` 头加入 `image/svg+xml`。
- `src/data/reader_images_tests.rs`：新增端到端 provider + 持久化缓存 + 离线重开 + `CredentialRejected` 撤权隔离 SVG 回归。
- `docs/reader-images.md`、`docs/devlog/2026-11-10-reader-svg.md`：更新当前支持能力与边界文档。

## 初次静态交付的验证（历史）

全部通过 `node scripts/verify.mjs` 串行执行（共享 Windows Cargo mutex），过程记录在 `.agents/task/runs/`：

| 命令 | 结果 | 运行记录 |
| --- | --- | --- |
| `cargo test --locked --test reader_images` | 通过，787 passed（最终代码） | `50ca666a-9a31-435d-88d7-c6aa9171561f` 内同一二进制 |
| `cargo test --locked`（全部测试目标，最终代码） | 通过：1235 / 98 / 599 / 293 / 259 / 787 / 885 / 44，0 failed | `50ca666a-9a31-435d-88d7-c6aa9171561f` |
| `cargo fmt --all -- --check` | 通过 | `201b9629-5aeb-4290-8984-86185de02de5` |
| `cargo clippy --locked --all-targets -- -D warnings` | 通过，零警告 | `e7736998-19ce-4748-b49b-36f56b07a26e` |
| `cargo check --locked --all-targets` | 通过 | `86595c65-1aac-4c75-aea2-67da6e8a8e46` |
| `cargo build --locked` | 通过，产物 `target/debug/githux.exe` | `d06b9928-5928-4b5f-90f1-4ca088294376` |

新增/更新的回归测试：

- `content::images::tests::formats_are_detected_from_content_not_extension`：SVG/BOM/XML 声明/DOCTYPE 识别与 RSS 负例。
- `content::images::tests::static_svg_badge_stats_card_and_chart_rasterize_to_png_with_dimensions`：shields.io 形态徽章（gradient/clipPath/text/scale/textLength）、`<?xml?>` + `viewBox` 统计卡片（CSS 类/文本/环）、Star History 形态图表（filter/turbulence/displacement/内嵌 PNG data URI/路径/文本）均产出有效 PNG 并保留固有宽高，并断言实际绘制像素颜色。
- `content::images::tests::svg_unsupported_active_and_foreign_features_are_rejected_honestly`：`<script>`、`<foreignObject>`、SMIL `<animate>`、`on*`、`@import` 均返回 `Unsupported`。
- `content::images::tests::svg_malformed_and_dtd_payloads_are_rejected`：截断标签、零尺寸、无固有尺寸、DTD/实体、非 UTF-8 均返回 `Malformed`。
- `content::images::tests::svg_oversized_dimensions_pixels_and_node_counts_are_bounded`：超单边/像素/解码字节预算与深度超限均在分配前拒绝（深度超限在 XML 解析前由有界扫描先行拒绝，避免深度树导致栈溢出）。
- `content::images::tests::svg_external_file_and_network_references_cannot_read_local_files_or_fetch_network`：`<image href="file:///…">`、本地路径、`http://127.0.0.1…`、`https://127.0.0.1…` 均不被读取/抓取（渲染结果只有背景色，本地 `TcpListener` 无连接）。
- `data::reader_images::tests::svg_badge_stats_card_and_chart_load_through_provider_and_cache`：真实 `RepositoryImageService` + 持久化 `CacheStore` 路径下，外部徽章/统计卡与仓库内图表 SVG 经 provider 解码为 PNG；缓存重开离线读取仍成功；`CredentialRejected` 后私有仓库 SVG 读取返回 `StaleContext`。
- 既有 782 项 `reader_images` 回归（授权、取消、超时、预算、缓存新鲜度/304/no-store、私有隔离）继续通过。

同时把 `fixed_revision_images_persist_across_reopened_store_and_service` 与新增缓存测试的重开逻辑改为等待后台写入释放缓存锁（`Fixture::reopen_cache_store`），修复全量并行测试下 `CacheStore` 重开偶发 `InUse` 的既有竞态。

两次全量运行中出现与本改动无关的抖动：`app::pages::home::tests::authentication_failure_cancellation_and_uncached_pages_are_not_historical_success`（`e742c5f5` 同一套件通过，单独重跑通过，随后全量再次全绿）与 `network::transport::bounded_tests::chunked_and_no_length_stop_before_eof_on_success_and_errors`（`926780ed`，本机同时出现 GitHub TLS 握手失败与 loopback 请求 `error sending request`；单独重跑通过，最终全量 `9b0b60fa` 全绿）。按 L002 记录为负载/网络环境相关的既有测试抖动，不声称已修复也不归因于本任务。

## 审查修复轮次

第一轮 Codex 审查（head `35ed2fa4`，review 5838968518）提出 2 条 P2，均核实成立：

1. **超采样 PNG 未按实际像素网格计费**：`DecodedImage::decoded_bytes()` 原来只用逻辑 `width`/`height`，而 2x 光栅化后的 PNG 像素是 4 倍，reader 的保留预算/淘汰会低估内存。现在 `decoded_bytes()` 对 `ImageFormat::Png` 读取 PNG IHDR 的实际宽高再计算 `w*h*4`（普通 PNG 的两者相等），并新增断言「徽章 SVG 的 decoded_bytes 等于实际 PNG 网格大小且大于逻辑尺寸」。`DecodedImage` 字段未变，UI 仍用 `width`/`height` 做比例布局。
2. **非 ASCII 属性名切片 panic**：事件属性检查原来用 `name.len() >= 2 && name[..2].eq_ignore_ascii_case("on")`，遇到合法多字节属性名（如 `中`）会在非字符边界 panic，生产路径会被转成 `Network`。现在改用 `name.as_bytes()` 检查前两个字节，并新增回归 `svg_non_ascii_attribute_names_do_not_panic_the_decoder`。

第二轮 Codex 审查（head `91d24b8e`，review 5839212482）提出 2 条 P1，均核实成立：

1. **深度预扫描把图形元素当成隐式 void**：原实现把 `rect`/`path`/`image` 等当作自闭合跳过计数，但 XML 没有隐式 void 元素。`<rect>` 连续嵌套 300 层时预扫描只报 1 层，进入 roxmltree 的递归 tokenizer 后仍可能栈溢出。现在只豁免真正的 `/>` 自闭合写法；新增 `svg_xml_nesting_counts_every_non_self_closing_element`（300 层 `<rect>` 在解析前返回 `Malformed`）。
2. **内嵌栅格只限制压缩字节、未限制解码像素**：`<image href="data:image/png;base64,...">` 的 2 MiB 编码上限拦不住高压缩的超大画布（如 8192×8192 解码后 256 MiB），而 `usvg` 只包装字节、由 `resvg` 在渲染时才解码。现在 `svg_usvg_options(limits)` 在 `resolve_data` 中用 `image::ImageReader::into_dimensions()` 只读文件头，按 `max_dimension` / `max_pixels` / `max_decoded_bytes` 校验后才交给 `usvg`；超限的内嵌图被跳过。新增 `svg_embedded_raster_pixels_are_bounded_before_resvg_decodes_them`（9000×9000 声明画布 + 极小 payload 只渲染背景色）。

第一轮修复的校验（PNG 实际像素网格计费、非 ASCII 属性名不切片 panic）在本轮继续通过。

第三轮 Codex 审查（head `827a2e4`，review 5839445655）提出 1 条 P1，核实成立：

1. **深度预扫描不识别引号内的 `>`**：`<g a="/>">` 的 `/` 与 `>` 都在属性值里，但简单扫描把它当自闭合，20,000 层嵌套仍然只报 1 层并进入递归 XML 解析器。现在 `count_svg_nesting_depth` 是一个小型非递归扫描器：跳过 `<!-- -->`、`<![CDATA[ ]]>`、`<? ?>` 和 `<!DOCTYPE ...>`（含内部子集括号与引号），在开始标签内跟踪 `'`/`"` 引号状态，只在引号外把 `>` 当标签结束、并要求结束前最后一个非空白字节是 `/` 才判自闭合。新增 `svg_depth_preflight_ignores_tag_like_text_in_quotes_comments_and_cdata`：300 层 `<g a="/>">` 在解析前 `Malformed`，而注释/CDATA/引号中包含 `<g>`、`>`、`/>` 的合法小文档正常渲染。

第四轮 Codex 审查（head `b217590e`，review 5839774180）提出 1 条 P2，核实成立：

1. **超采样取整可能越过像素预算**：`2001×2000` 的合法 SVG 本身低于 8,000,000 像素，但按 `sqrt(cap/(w*h))` 缩放后四舍五入得到 `2829×2828 = 8,000,412`，被随后的检查判为 `TooLarge`，等于可选的超采样拒绝了本可支持的图片。现在先向下取整，若乘积仍超限则按比例缩小并做有界递减，保证 `render_w*render_h <= min(max_pixels, max_decoded_bytes/4)`。新增 `svg_dimensions_just_below_the_pixel_budget_still_render`（2001×2000 正常解码，实际 PNG 网格不超过像素/字节预算，`decoded_bytes` 与网格一致）。

## 初次静态交付的运行边界（历史，已由返工取代）

- 构建产物：`target/debug/githux.exe`（`cargo build --locked`）。预览入口沿用 `cargo run --example reader_images`，但本任务未修改预览 fixture（归 `reader-image-layout`），因此预览仍只展示原有 PNG 场景；SVG 的生产路径证据来自数据层/provider 回归，不是外部网络可达性证明。
- 不声称原生视觉验收：本任务无 UI 改动，照片/徽章的实际排版、去灰底与拉伸由 `reader-image-layout` 负责并经负责人原生窗口验收。
