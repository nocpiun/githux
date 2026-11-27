# Home/Feed 描述测试的 CI 稳定性

## 失败与复现

main `f9784ab76674fb39dd9b63b60779e87b4509e3d6` 的 Ubuntu Code check 在 `src/app/pages/home/tests.rs:974` 等待首个仓库描述进入 transport 时失败。其余 1,651 项测试通过，8 项被忽略，四个平台的 Debug 构建均通过。[失败 job](https://github.com/nocpiun/githux/actions/runs/45978883307/job/140437542244) · [完整运行](https://github.com/nocpiun/githux/actions/runs/45978883307)

本机在该基线上仅暂停 `/search/issues` 并记录原 FIFO 的回包。`/user/repos` 消耗的是为 `/user` 排入的用户对象，接着 `/users/octo/received_events?page=1` 得到 Home 搜索对象。Feed 接收方期待事件数组，立即进入 `invalid type: map, expected a sequence`，没有星标事件触发 `/repos/octo/repo`。GPUI 测试已在显式 drain 中收到这一失败，门闩因此始终未进入。原 CI 日志只记录门闩超时，精确回包顺序由受控复现提供。

复现命令 `node scripts/verify.mjs -- cargo test --locked --bin githux append_keeps_a_delayed_first_page_description_and_refresh_rejects_old_batches -- --nocapture`，run `7949c5a6-c56e-4228-a00d-aed472bac167`，1 failed，退出 101。构建槽等待 58.3 秒，命令运行 72.6 秒。

## 修复覆盖

- 按 HTTP 方法、主机、路径和 Feed 页码分派 mock 回复。账户检查、侧栏仓库列表、Home 搜索、关注关系和具体仓库的描述各有对应数据。
- 追加用例先让 Feed 在 Home 搜索暂停期间发布首张卡片，确认 `/repos/octo/repo` 已进入门闩。放行真实第二页，确认两张卡片和第二页描述绘制，首个描述批次仍在途。再放行首个描述，确认两条描述都绘制。
- 刷新用例先让真实旧批次完成并留在 `enrich_inbox`，通过视图触发刷新后才 drain。确认旧批次已被消费，且刷新后的同一仓库仍无描述。放行刷新后的描述，确认新描述绘制。

验证经过账户服务、Feed 过滤、HomePage 回包队列和 GPUI 测试窗口。改动范围为测试夹具与本日志。

## 修复后验证

- `node scripts/verify.mjs -- cargo fmt --all` 成功，run `6e7b0183-a886-4ea5-97c0-4d2ec4084360`。
- `node scripts/verify.mjs -- cargo test --locked --bin githux app::pages::home::tests::` 全部 31 项通过，含两条描述回归，run `5b137643-e1b4-4e4c-9b7c-b25720f5d092`，退出 0。构建槽等待 102.6 秒，命令运行 79.6 秒。
- 完整测试套件及各平台构建由 PR CI 验证。
