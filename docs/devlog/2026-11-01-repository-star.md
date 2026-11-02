# 2026-11-01 · repository-star

## 交付

按 G3 顺序实现仓库页首个真实写动作 Star/Unstar，Fork 保持禁用。交付包含受控 PUT/DELETE transport、Starring REST adapter、RepositorySession 上的 action service、正式 RepositoryPage 状态与 Shell 组合、可重复离线 fixture/原生 preview，以及服务链/整页/正式 Shell 回归。

## 设计与 API 核对

开工只读抓取 GitHub 官方 Starring REST 文档（apiVersion=2022-11-28）：GET 状态 204/404，PUT/DELETE 成功 204；写响应不提供新 count；fine-grained 权限与 404 不可见歧义按官方语义保留。没有调用业务 GitHub API，没有读取或使用真实 PAT。

写后以权威 repository summary 刷新 count；超时/断连只 GET 对账，失败明确 UnknownOutcome，任何路径都不自动重放写。初始和写处理中禁用重复动作；旧 repository summary 用 Star 写代次拒绝回盖。账户/仓库/页面身份和既有 read_authenticated 门禁贯穿成功与错误发布。

应用的相关 LESSONS：

- L002：先核对官方响应形状；离线 fixture 与真实 GitHub 验收边界分开记录。
- L008：重复点击在真实异步忙碌阶段驱动，服务并发/取消沿用已验证的 session 门禁。
- L010：preview/test 共用按 method/path/account 应答的可重复 fixture，支持重试与往返。
- L012：验收在唯一 RepositoryPage 与正式 Shell 组合上进行，不以孤立按钮替代。
- L014：失败只用紧凑 warning + tooltip；可重试时才提供动作，成功不靠 toast。
- L016：没有改动 Read More 路由、未支持页面或新增详情目标。

候选可复用教训（交协调者评估，不修改共享 LESSONS）：写操作旁的旧读取需要“写意图代次”而不只是资源/账户代次；否则较早的 repository summary 会回盖写后计数。本任务以 `star_write_generation` 和专门回归处理。

## 实现中发现并修复

- `StarTimeoutUnknown` fixture 初版把首次状态 GET 也设为超时，测试无法建立初始状态；改为只在写后对账超时，没有放宽断言。
- 正式 Shell fixture 的 repository summary 条件曾提前吞掉 `/branches`；改为精确 summary path，并让测试显式排出真实 RepositoryPage 队列后再点击。
- Star binding 初版复用 overview binding，使无关的 PageReadSession 创建失败也能禁用 Star；改为直接由已验证 RepositorySession + canonical repository 构造，同时保留服务实例/账户门禁。
- 自查补上旧 repository summary 的写代次，防止旧读取回盖写后状态/计数；摘要失败保留已确认 starred state 并显示可重试警告。

## 验证记录

所有 Cargo 调用通过 `node scripts/verify.mjs -- cargo ...`，运行 JSON/日志在本地 `.agents/task/runs/`。早期 `cargo check --lib` 因项目无 library target 退出，改用实际 package targets；不是源码失败。

聚焦结果：

- `3de5756a`：`cargo test --locked --bin githux repository_action` 通过（10 项，含 github adapter 匹配项）。
- `4ce1069b` / `81f3b523`：`cargo test --locked --test repository star_` 通过；后者包含旧摘要回盖回归。
- `67e689cd`：`cargo test --locked --bin githux production_shell_composes_real_star` 通过，真实 Shell 按钮产生本地 PUT 和摘要 GET，Fork 无请求。

## 最终本地验证

- `0d192914` / `6fb5514f`：`cargo fmt --all` 通过。
- `4f9e1cee`：`cargo check --locked --all-targets` 通过。
- `cdd6f442`：`cargo clippy --locked --all-targets -- -D warnings` 通过。
- `07df4645`：`cargo test --locked` 全部通过：主 binary 972；home 95；profile 279；reader 250；reader_images 633；repository 715；repository_overview 44。
- `b178fd99`：`cargo build --locked` 通过。
- `2b9b3516`：`cargo build --locked --example repository` 通过；直接启动生成的原生 `repository.exe`，12 秒后仍正常运行、无 stderr，随后终止。仅证明离线原生窗口可启动；未声称已完成鼠标/tooltip 人工验收。

完整测试初轮发现新增 localhost 重定向测试未收敛及 adapter 测试对 mock 记录的错误假设，均修复后重跑；不以旧成功结果替代最终 `07df4645`。

未执行任何真实 GitHub 写操作；最终视觉、tooltip 与 Windows 原生按钮验收由负责人进行，PR 保持 `pending`。

## PR #63 第一轮审查修复

Codex 在 `02b8e4d` 指出：写成功但 summary 失败后，Retry 只读 starred 就清除 warning，会留下看似恢复的旧 count。意见成立，无需裁决。新增 `RepositoryActionService::refresh_starred`，初始读取与 Retry 都执行 starred GET + authoritative summary GET；不再复用只读状态成功来掩盖 summary 失败。新增 `StarSummaryFailure` 场景与整页回归，确认 warning 保留到 count 恢复、重试没有第二个 PUT。

修复后验证：`a484d8cf` 专项通过；`dde477b8` 完整 test 通过；`2c2af8d0` clippy 通过；`ad5e8180` build 通过。Clippy 中间发现重构后的未使用 import，已删除并重跑成功。

## PR #63 第二轮审查修复

Codex 在 `ee3e54f` 指出 `drain_star` 接受结果后没有 `cx.notify()`，独立完成的写可能不触发重绘，而 tests 的显式 refresh 掩盖了问题。意见成立，无需裁决。`drain_star` 现携带页面 Context，在至少一个经完整门禁接受的 reply 后通知；过期回复不通知。新增不主动 refresh/draw 的 observer 回归，证明接受结果会使页面失效。

验证：`6e9e2f08` 通知专项通过；`6e6db7ff` 完整 test 通过；`981d0b31` clippy 通过；`0ced48bc` build 通过。`70c29500` 全套中既有 bounded-read 用例出现一次 `error sending request`，该用例由 `0a01ae05` 单独通过且随后全套通过；未改测试掩盖波动。
