# Feed description 并发 fixture 修复

日期：2026-11-02

任务：`feed-description-fixture`

## 根因与修复

`repository_descriptions_deduplicates_requests_and_handles_failures` 原先向全局 FIFO 依次加入 `acme/demo` 的 200 和 `acme/missing` 的 404，再断言两个详情请求也按该顺序到达。生产实现有两个 worker，各自经过异步凭据读取后才发出请求，因此响应 FIFO 的出队顺序并不等于仓库身份。PR #64 的 [CI run 41400442646 / job 125130967154](https://github.com/nocpiun/githux/actions/runs/41400442646/job/125130967154) 已复现该竞态：demo 得到 `Some(None)`，说明它消费了 missing 的 404。

本任务只修改测试 fixture，不改生产读取行为。详情响应改用现有 `Controlled::block_url` 按完整 URL 路由；两个请求都进入 gate 后才允许任一响应完成，再按 missing → demo 的顺序释放，覆盖与输入相反的完成顺序并确认读取没有串行化。保留 demo description、missing `None`、总数和去重断言，详情请求按 URL 与次数核验，不再依赖 `requests[1]` / `requests[2]` 的到达顺序。

应用 LESSONS L008：两个 gate 都在任何释放前进入，串行实现会在等待第二个请求时超时。应用 L010：响应绑定资源身份；未建模请求继续由 `Controlled` 明确 panic，不会任意回退为 404。无需修改共享 helper；现有有序 FIFO 和其他 gate 测试的语义不变，main 的 page-cache 模块注册保持原样。

## 验证

所有 Cargo 命令均经 `node scripts/verify.mjs -- ...`，共享 Windows mutex。

- 修复前：上述 CI 日志提供了真实反向顺序失败证据，1036 passed / 1 failed。当前基线本地定向运行 `ea346f4f-f0f4-4f22-88a6-07b7421c779a` 恰好通过，说明未施加调度时该竞态并非每次本地都出现，不把它计为稳定复现。
- 修复后定向用例：`bffb04da-a993-48b8-9e7d-7ea145cce641`，1 passed。
- Feed/page refresh 相关用例：`455851b9-8777-4d6e-8e31-9b59a43f4677`，全部通过。
- 完整 page-read：`a7ecfc65-f5a2-4aa6-afa9-f522d91e3525`，30 passed。
- 最终检查：fmt `89097e2e-23c6-4395-bf6d-f1950009cfc5`、check `3fcc3728-2989-45c3-97ca-534e55ff86ae`、clippy `888d9c9b-7edd-4090-aea6-44eb053625f4` 全部通过。首次 fmt `2167eed2-3f47-4d06-b489-17bb79e1cd70` 仅发现一处换行格式，按日志修正后重验。
- 首次全测试 `e6a4cfe0-48e1-4edb-b27e-73d8bb101ebb` 中主二进制 1063 项通过，但无关的 reader 图片缓存重开用例报 `InUse`（`tests/repository.rs` 791 passed / 1 failed）；该用例定向重验 `65f03df3-968d-4b19-b821-7380160d5e5c` 通过。因存在实际失败，完整测试再运行一次，`177d8bef-c31c-455c-b1cf-bc32e0632efe` 全部通过。未修改图片 fixture 或其生产代码。

验证使用合成凭据与可控 transport，不访问真实 GitHub；没有 GUI 或生产行为变更，因此未做 GUI 验证。未修改 PR #64 或其 worktree。此问题已归入现有 L008/L010，无需修改共享 LESSONS，也无额外候选条目。
