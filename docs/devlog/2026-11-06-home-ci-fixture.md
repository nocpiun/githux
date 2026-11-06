# Home 并发夹具的 CI 修复

## 问题与根因

main `78ce277` 的 [CI run 42173936188](https://github.com/nocpiun/githux/actions/runs/42173936188) 中，`app::pages::home::tests::no_cache_binding_keeps_the_original_online_path` 失败；四个平台构建成功，Rust 测试报告 1216 通过、1 失败。Home 预期取得 `online only`，实际进入解析错误，提示 `invalid type: map, expected u64`。

该组合测试创建真实 HomePage；Home 搜索与 Feed 读取各自提交到后台 executor，并不保证先后顺序。旧夹具把 `/user`、搜索结果、事件、关注列表、仓库简介排进同一个 `MockTransport` FIFO，因此 Feed 先发请求时会拿走搜索响应，后续响应也会错位。PR 上相同代码曾通过，只表示那次调度顺序碰巧符合队列，不能证明夹具稳定。

## 修复与验证方法

在线无缓存场景按 HTTP 方法、host 与路径选择响应，支持正常重复读取，未建模请求报告测试配置错误。其他缓存/错误注入测试继续使用既有脚本与 gate。产品请求逻辑、并发度、缓存路径与 CI 门槛不变。

保留原组合断言：Home 显示目标 issue，Feed 显示目标活动，页面没有缓存绑定。增加 transport gate，分别强制 Home 先完成、Feed 先完成，另一列必须仍处于 Loading 后才放行，消除对偶然调度顺序的依赖。回归不是给产品请求加串行锁。

所有 Cargo 操作通过 `scripts/verify.mjs`，以实际 runner 退出码和日志为准：

- 修复前：仅补强制 Feed 先完成的回归，运行 `cargo test --locked --bin githux no_cache_binding_reads_feed_before_home`，稳定重现与 CI 相同的 `invalid type: map, expected u64`；run `6c98a56d-0559-494d-a761-9add1fc63b42`，exit 101，1 failed。
- 修复后：`cargo fmt --all` 成功，run `2ecd8cf3-f304-4044-bf71-88e91016ee33`；`cargo test --locked --bin githux app::pages::home::tests` 全部 15 passed / 0 failed，run `23770f54-52ac-41df-ae9e-b4de781fe4fa`，exit 0，其中两种强制顺序分别通过。
- PR 最新 head 的完整 CI / Codex 审查和合并后 main CI 以 GitHub 实际运行记录为准，不以本地专项通过代替。

本任务只修改测试夹具，无新增原生 UI 或实网验收声明。

## LESSONS

应用并补充 L010：异步组合 fixture 除了支持反复操作，还必须按资源身份应答，并用受控顺序验证并发，不依赖全局 FIFO 或重复运行到绿。
