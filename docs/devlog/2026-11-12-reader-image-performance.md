# README original-image performance（2026-11-12）

任务：`reader-image-performance`；性能基线 `2c34b455d0f394b293b789f467e774901ef130f8`。已在本任务 worktree 合入获准的 table/silent checkpoint `945698ddfe271979d3d95c2f2b3c7d6f16628983`（本地整合提交 `5774c6b`），未复制其它 worktree 的未提交文件。PR #86 仍 OPEN/pending，既未合并也未获负责人验收；性能 PR 须 stacked 在其分支，依赖合入后再 retarget/rebase main。性能交付 PR 为 [#88](https://github.com/nocpiun/githux/pull/88)，milestone #1、`pending`。本文不是负责人验收收据。

## 行为与资源边界

网络准入仅包围实际远程读取，本地 metadata/cache I/O 与 decode 各有独立配额（后两者各 2）。从排队开始计算总截止时间，blocking worker 持有许可直到实际结束。commit/tree/目标 bytes/blob 的在途合并活在捕获 session 的 service 内，最后一个订阅取消才取消共享读取，不建立跨账户 SHA 热缓存。

生产 raster 一次解码，保留 EXIF、straight BGRA、GIF/WebP 多帧与 delay。静态 raster 的实际显示网格与原始 header/作者逻辑尺寸分离；缓存继续保存完整原始编码字节。SVG 不再编码中间 PNG：先发布正确初帧与最终备用图，再按原时间戳分批加入后续帧；空间网格先核验，预算优先减少时间采样，不取消有限动画。

事件携带 scope/request/连续 phase/terminal，首 Ready 不释放在途计数。排队事件保留 decode 准入，取消时移除事件并拒绝迟到发布。HTTP fresh/SWR/304/no-store、Date/Age/响应时间、旧 age 元数据迁移、授权/clear 失效令牌均有回归；同字节验证不重新解码或起播。

普通行和 table cell 上报实际 visible/near-visible 几何，初次静默图片通过不增加 flex 子项/gap 的探针上报插入点。Flow 使用外层真实 mask 和自然高度根盒；Reader 使用自身 viewport。Released 不绘制图片 UI 或保留布局空间，仅留原图尺寸元数据供恢复网格计算；新的需求 epoch 才恢复，scope/DPI/恢复切换允许一次新尝试。Ready 尺寸、表格列对齐、选择/链接和 alt/copy 模型保持原路径。

虚拟列表会跳过锚点前的零高度行，因此“本帧未测量”不能直接判为离屏。已准入、几何仍未知的 Loading 请求保留有界需求；实际测得 outside 或 scope 失效仍取消。此规则不把 Ready/Released 的旧位置长期当作当前可见性。

窗口稳定后，静态 raster 若比实际目标宽至少 25% 则从原始数据重采样；小幅抖动不反复解码。SVG 与 GIF/WebP 时间线不进入这条缩小路径。透明边缘用 16-bit premultiplied 中间通道滤波再恢复 straight alpha；opaque screenshots 保持直接路径。

GPUI `RenderImage` 绕过 asset cache，Arc 释放不自动删除 atlas tiles。动画收尾、替换、淘汰、隐藏/释放和文档切换均显式安排 `window.drop_image`。64 MiB 是每个 reader 的 retained pixels 起点，**不是进程 RSS 或独立显存的硬上限**；原始响应、两个 decode 作业、旋转/重采样临时面、事件队列和前台回收间隔中的旧资源另计。

## 同 profile 的阶段样本

所有 Cargo 操作走 `node scripts/verify.mjs -- cargo …`，保留 runner 默认和共享 heavy-build mutex。`TEMP`/`TMP` 指向本任务 D: 目录，`CARGO_PROFILE_DEV_DEBUG=0`、`CARGO_PROFILE_TEST_DEBUG=0`、`CARGO_INCREMENTAL=0`。不为耗时设置通过阈值。

阶段探针使用同一确定性 2559×1520 PNG（146,373 encoded bytes）与原创 `entry-stats.svg`。基线 run `bdf9b3fc-4022-42b8-a6e5-61dedabd8016` 等槽 228ms、运行 479,308ms（包含首次依赖构建）。最终内核阶段 run `093d2989-4ec7-41f3-952e-93be7db96cb6` 等槽 177ms、运行 169,835ms。应用和原始 decoder 维持 unoptimized 配置；新增 `fast_image_resize 5.1.4` 与私有 `crates/image-resize` 在 dev/test 中明确按 opt-level 3 编译，属于交付配置。非泛型内核边界使泛型卷积也在该包内优化，未改变 runner 参数。

最终收尾 run `cc09b676-4d83-4f47-81fc-85d5d92b9db5` 复测完整原图→显示网格为 238,865µs、内核约 10.7ms；同次 SVG 首发布 45,472µs、完整准备 289,444µs / 27 帧。阶段样本间有机器负载差异，不将单次最好值当保证。

| 阶段 | 基线样本 | 联合性能样本 |
| --- | ---: | ---: |
| 三个网络槽占满时 fresh cache | 100ms 内无 Ready，需释放网络槽 | 槽仍占用时 13,513µs 完成（`e40d4835`） |
| PNG native-grid provider decode/conversion | 154,896µs | 258,915µs |
| PNG 1200×713 display-grid provider | 未实现 | 203,477µs，retained 3,422,400B |
| renderer 额外静态解码 | 154,926µs | 0 |
| stats SVG 首 usable publication | 等待整组 | 43,394µs |
| stats SVG 完整准备 | 463,302µs / 27 帧 | 243,204µs / 27 帧 |
| stats SVG retained pixels | 2,976,000B | 2,976,000B |

这些是阶段样本，不是网络、实际首屏绘制或应用端到端速度承诺。基线额外解码为按 GPUI 路径执行的确定性探针；转换 BGRA 本身也有成本。另有更强的结构性回归：三个**真实 gated download** 持续占用网络槽时，fresh cached image 必须先 Ready，不依赖脆弱的毫秒阈值。

## 九原图、滚动和内存

九张 native pixels 合计 140,028,480B。独立最终 probe（`cc09b676` 的首例）在 600 logical px、2× DPI 下保留 30,801,600B（29.37 MiB），Windows working/peak 从 `(7,208,960, 23,146,496)B` 到 `(47,034,368, 62,537,728)B`。此前隔离 run `58080ec3` peak 为 83,808,256B（79.93 MiB）；它们都是确定性 decode 进程，均不含原生 UI/GPU。

联合 UI fixture 使用另一份生成的 2559×1520 PNG（480,659B），九个独立资源身份，真实 AccountService/cache/provider/decoder，包含普通行/表格和 Repository/Flow 四组合。缓存逐项比对完整原始 bytes。保留数量上限设 4 强制回收，字节预算仍 64 MiB；验证所有九图随滚动 Ready、scroll-back 不再次读取外部源、稳定 15 frames 不新增 decode/eviction/reload，及 alt 文本保留。首次 reads 允许启动 resize/DPI 取消后的重试，不将它伪称成每图恰好一个网络读取。

| 大图场景 | settle 后 retained B | incoming pixel peak B | evictions / reloads |
| --- | ---: | ---: | ---: |
| Rows / Repository | 13,336,576 | 16,670,720 | 6 / 1 |
| Rows / Flow | 13,689,600 | 17,112,000 | 6 / 1 |
| Table / Repository | 2,792,384 | 3,490,480 | 7 / 2 |
| Table / Flow | 2,877,600 | 3,597,000 | 7 / 2 |

联合表取自最终 `cc09b676`，测试从第一次绘制就使用 640×400 宿主，避免先用默认最大化窗口发请求再缩小。九图访问完共读取九个外部资源。该测试进程累计 working-set peak 为 121,257,984B，包含此前字体/探针/多用例的峰值；GPUI 测试使用测试图形平台，**不测真实 GPU**，不能按表格 retained 值冒充总内存。native-size core pressure 另测 retained 62,234,880B、replacement incoming peak 77,793,600B，8 evictions / 4 reloads，稳定不 churn。

列宽/DPI 联合探针实际网格 542 → 802 → 1203px（640→900px 窗口，2×→3×），逻辑尺寸不变，重新读取原始 cache，外部源首图读取次数不增加；settle 后不持续解码。

## 真实 raw blob 契约

匿名、有界读取公开 `opanel-mc/opanel` 固定 commit `66e9985afca1f43abf3eff5ddeaf6416212b5cde` 的 Dashboard blob `a4c3cd1ae89b951f7beb217a678b43bc6d6e5b17`，Accept=`application/vnd.github.raw+json`、version=`2022-11-28`：HTTP 200、`application/octet-stream`、423,588B、PNG 签名 `89504e470d0a1a0a`，`SHA1("blob " + length + NUL + body)` 与请求 SHA 一致。未提交第三方响应；未用 raw.githubusercontent.com 的计时替代此 API 验证。

图片专用 raw adapter 保留 RepositorySession、同源重定向、字节上限、错误分类与响应缓存元数据；注入 transport 不支持二进制，或 406/415 时才有界回退 JSON。通用 text/blob 路径保持原契约。SHA1 验证 Git 对象身份，不替代授权。

## PR #88 首轮反馈与修复

审查在 `06108b77` 指出：SWR 旧像素到期并被撤回后，成功的 304/同字节 200 只发布 `Validated`，原来的 UI no-op 会永久空白。`ImageBoard` 现在仅在仍持有 Ready 像素时直接确认；否则结束旧请求一次、取消其 sink，并按实际需求从已验证缓存重新得到 drawable pixels，迟到的旧 terminal 不占新请求的槽。仍存活的同字节图不额外 decode，也不重启时间线。

新增真实 service/cache/Reader 回归分别 gate 304 与同字节 200：先观察 Ready，再等明确 SWR 到期、像素成本归零，之后放行响应；最终恢复 Ready，只有一次外部读取与两次 decode（provisional + 已丢像素后的恢复）。另有“Validated 已排队、UI drain 前到期”的计数/旧 terminal 回归。聚焦 `e60f15e1` 五例通过；将恢复分支暂时设回 no-op 的负对照 `ba2f5843` 两例均在“成功验证后应 Ready”断言失败，随后恢复正式实现。

第二轮审查在 `1dc06963` 指出 provisional decode 自身排队/执行越过 SWR 时，`StaleContext` 会提前退出而不重验证。现在只在原授权仍 current、失效的是旧像素许可时跳过这次可选 decode，继续同一总截止内的网络/缓存路径；撤权不被吞掉。受控 semaphore 回归先占满两个解码许可，等请求确认排队，再使 SWR 到期，分别检查 304/同字节 200 到达条件请求并恢复 Ready；第三分支在排队时换账户，必须保留 StaleContext 且零外部读取。`50062564` 六个 SWR 聚焦用例通过。去掉该 catch 的负对照 `83f52fcb` 在“重验证应进入”处等待失败，随后恢复正式 catch。集中 fmt check `7f57f6b6`、all-target Clippy `7aa0f70c` 通过；完整 `cargo test --locked` 的 `39d21fbb-05ed-49d0-a938-0db47c134e6e` 九个目标合计 6263 passed、0 failed、22 ignored。

同一旧 head 的四平台 CI build 全通过，Ubuntu Code check 暴露两个测试前提：Home 已暖缓存含 continuation，检查时可已恢复第二页；Profile 的历史/partial 标记或 retry 可能在自然高度正文下方。Home 撤权测试现在接受该已知缓存集合的首屏/已追加状态，继续要求撤权后全清空；Profile 使用模型等待不同 provenance 状态，并在明确 880×560 宿主中走真实滚轮将 retry 揭示后点击，保持真实恢复断言。没有对应的产品逻辑变更。修复后集中验证：fmt check `0e794b28`、all-target Clippy（warnings denied）`4840c220` 通过；`9fee1975-1268-4956-a8ac-077572a24ada` 在基于 `06108b77` 的修复工作区运行正式应用 + image target，1396 + 906 = 2302 passed、0 failed、11 ignored，包含上述小视口 Profile 和 Home 撤权用例。

第三轮审查在 `bf5b3154` 指出坏的可选旧内容也不能阻断重验证。旧响应在 decode 前已持久化，因此 malformed/unsupported 或超过旧图预算时应放弃 provisional pixels；仅在原授权仍 current、取消未发生且总截止未到时继续。新增一条真实 service/cache 用例分别先存 unsupported 文本、截断 PNG、encoded 超限、pixel 超限四种坏旧内容，再要求条件请求得到有效 1×1 原始 PNG，最终 Ready、两次 decode 且缓存精确换成新原字节。`dd34551e` 七个 SWR 聚焦用例通过；去掉内容错误分支的负对照 `88dff9fb` 在 HTTP 进入断言等待失败，随后恢复正式分支。集中 fmt check `99751a6d`、all-target Clippy `cc483f5f` 及 `9ff1d245-c020-488a-b091-eab6986bc2c0` 的应用/image 两目标通过（2308 passed、0 failed、11 ignored）。

## Released 零空间修正

移除 `released_geometry`：Ready 以外的普通图片行和 table cell 一律不生成布局盒；Released 同样用零高度插入点收集需求。保留的只有原图尺寸元数据，不是已显示图片的宽高占位。新增 `performance_released_table_picture_removes_its_measured_space` 用两个独立图片行施加 retain-one 淘汰，要求被回收行回到作者图注/padding 高度，scope/DPI 回归也明确断言 Released 没有布局几何。

`02c2b820-3234-41c0-8d41-ec80e2537083` 的 45 个 performance 用例（显式含 ignored originals，单线程）全部通过，覆盖四宿主九原图、回滚恢复、静止无 decode churn、SWR 边界和列宽/DPI。大图 Rows/Reader、Rows/Flow、Table/Reader、Table/Flow retained 分别 13,336,576 / 13,689,600 / 2,792,384 / 2,877,600B，incoming pixel peak 16,670,720 / 17,112,000 / 3,490,480 / 3,597,000B。所有宿主都走到九个原始身份；这是 test-platform 证据，原生几何交互待同 head 重跑。

旧 head `1dc06963` 的 CI 仍有两个 Profile 等待失败；新增 `track_caller`、provenance 与 unavailable 诊断保留真实条件。进一步将 Profile fixture 开关移到页面创建前：此前 SlowPicture 在初始加载开始后才换 README 内容，却沿用同一 commit/blob；固定资源缓存可以合法命中旧模式。partial/calendar-fail 也必须先配置，保证 NotCached 用例没有先成功写入日历。两处慢图用例均先装 gate 再建页面，不修改产品缓存规则。

零空间实现的 fmt `bac7eff7`、all-target Clippy `04292a1f` 及 `b1f6b2f7-f3e3-4d2b-8bc4-dcc0c9479812` 的应用/reader/image 三个目标通过（2654 passed、0 failed、12 ignored）；fixture 初始化修正的 formatter `1e6c4454`、all-target Clippy `8abf6733` 和正式应用测试 `7e79e69f-6388-4115-a1f2-e3b844557e37` 通过（1398 passed、0 failed、3 ignored）。`bf5b3154` 的云端 CI [43384730053](https://github.com/nocpiun/githux/actions/runs/43384730053) 也已全部通过（含 Linux 完整 Code check、四平台 build）；先前两处 Profile 等待失败已在该组合下通过。后续坏缓存修复仍需其新 head 的独立 CI/review。

零空间实现另在 `bf5b3154abb63303a82716e292f0deb7b568e296` 的 clean build `c6c60285` 原生验证：144 DPI，16 张 PrintWindow 记录均 foreground=true；四模式向下/向上、retain-four 回收恢复和 900px resize 均完成，正常关闭 exit0，九个唯一原始身份共获取九次。进程关闭时 working set 183,422,976B、累计 peak 280,985,600B、private commit 228,057,088B，专用 GPU 内存未测。该原生全矩阵对应此布局 head；后续可选坏缓存修复只改 service 错误路径，仍需重建最终 artifact/source-head 冒烟核验。

## 验证与原生预览交付

此前独立 backend 完整目标 run `f7ee0b0e-ebf2-4a34-8869-3f5b15029522`：872 passed / 3 ignored；bin + image Clippy run `aabf03f8-4ad3-448f-a1e2-9765a4cd3bcd` 通过。这些不代替本次接线后的完整收尾检查。

失败记录保留：`ca32773f` 复现 Flow 正文 3507px、reader 400px、scroll reach 0；`71ff58c8` 37 passed / 1 failed，复现初始 1822px 未缩到实际 542px。修复后的 `5719a239` 39/39 通过。早期编译错误与被工具超时中止的 `f74f5264` 均不算通过；该遗留 running 记录没有被伪装成成功。

2026-11-13 完整包检查先发现旧“延迟”测试使用同步 provider，`before` 已 Ready（`6d2bb296`）；改为受控放行而不弱化增高/锚点断言。随后 `81fa61bb` 的正式应用 1395/1395 通过，image target 902 passed / 1 failed，暴露零高度行在虚拟锚点前缺测而被取消的真实边界。修正后聚焦 run `d94d1ef3-1a93-4f43-8a06-1acb0af26af8` 的 virtual/Flow 两例通过。

2026-11-13 原生检查先以 owned HWND + Win32 消息/PrintWindow 在 144 DPI 打开生成原图窗口，完成 Table/Reader、Table/Flow、Rows/Reader 三种模式的滚动与返回；第四模式的自动坐标误点 retention 按钮，由窗口标题校验识别，未伪称通过。原生计数显示九个独立源、九次 fetch，旧标量重采样的 decode 平均约 2.3s，进程 peak working set 348,205,056B；这个发现促使重采样改为 SIMD。中间阶段 1200px 网格仍耗 1.11s，取消边界探针把 915ms 定位到泛型重采样调用；隔离优化内核后该调用约 9.8ms，完整 display-grid provider 为 203ms。前述原生数字是被替换实现的记录。

最终原生检查在 144 DPI 对 owned HWND 使用 Win32 消息与 PrintWindow，完成 Rows/Table × Reader/Flow 四模式的中段/文末/返回、retain-four 压力回收与 900px resize，退出码 0。九个独立原图源仅 fetch 九次，73 次 decode 累计 40,060,662µs（不同尺寸/模式累计均值约 0.55s，不是严格同场景速度比），59 次 first-ready 累计 65,296,250µs。进程 working set 188,547,072B、peak 282,632,192B、private commit 230,711,296B；这些含 UI/字体/驱动等，独立显存未测量。第一张核验截图已显示原始彩色图片，表格对齐与作者图注保留。

独立 `reader_images` 的 `SVG profile` 原生场景在 175/557/1055/2548/4040/5570ms 取样，观察到 Stats 从标题/入场图形变为 Stars/Commits 内容，4s 与 5.5s 全窗口 PNG SHA-256 均为 `09f45d38c9dc37d06940c729dfd5a1c94593223d4616ed032ede0eebe8519067`，收尾后稳定且退出码 0。该生成 SVG 场景不代替负责人公开样本的视觉验收。

最终未提交工作区（基于 `5774c6b`，含最终内核）检查全部通过，仍使用前述 runner/env/mutex：

| 检查 | run | running / slot wait |
| --- | --- | ---: |
| fmt check | `6f0d75b3` | 21,984 / 0ms |
| check all targets | `0d27eae7` | 163,398 / 183ms |
| Clippy all targets，warnings denied | `072bc9ba` | 167,199 / 187ms |
| 私有内核 Clippy all targets，warnings denied | `8e255579` | 890 / 171ms |
| 应用 build | `0e465974` | 31,758 / 187ms |
| 两个 native examples build | `6fd8e816` | 31,911 / 168ms |
| 完整 `cargo test --locked` | `1a616185` | 605,806 / 183ms |
| 7 个显式 ignored 原图/阶段探针 | `cc09b676` | 41,152 / 188ms |

完整包九个 test binaries 合计 6250 passed、0 failed、22 ignored（跨 harness 有共享回归）；image target 为 903 passed / 8 ignored。显式七例均通过，未运行需负责人提供响应目录的 `public_svg_probe`。最终两个原生进程均正常退出，截图/匿名计数留在本任务 ignored 目录；早期失败尝试仅清理本任务启动的进程，未操作其它窗口。Cargo 曾报告全局 C: cache 的 last-use DB 空间不足；任务 temp/target 位于 D:，未清理其它 worktree 或用户缓存。

独立预览 `examples/reader_image_performance.rs` 与上述联合测试共用生成数据，支持 Rows/Table × Reader/Flow、retain-four 开关、滚动返回与 resize/DPI，不替换 table 任务原有 preview。可执行文件按完成构建的 source head 复制到本任务 ignored preview 目录并附 SHA-256/构建回执。关闭窗口在旁边保存 `.metrics.json`：source head、匿名累计阶段计数、实际 fetched 原图数、Windows working set/peak/private commit；明确 `dedicated_gpu_memory_measured=false`。真实字体、滚动、窗口输入和跨 DPI 的负责人验收仍独立于测试平台结果，PR 保留 `pending`。
