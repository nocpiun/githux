# 2026-11-12：减少 Agent 调查空转与验证排队

[滚动几何任务 #84](https://github.com/nocpiun/githux/pull/84) 的耗时复核发现：连续重复读取没有带来代码修改，最小复现没有覆盖用户实际看到的滚动中段，删除 padding 又改变了应保留的设计；格式检查也排在重编译后，相关用例与文档分批收尾导致重复验证和审查周期。构建等待、执行、CI/review 有重叠，不能把各项时间直接相加当作总交付耗时。

## 改动

- [共享工作流](../agent-workflow.md#先验证问题再扩大检查) 规定先记录实际失败、保留行为和最小复现；连续三轮调查无新事实后换方法做一次聚焦探针，仍无证据则保存检查点并报告阻塞。要求复用现有验证设施、确认 Cargo 目标、合并相关检查，并在相关修改和文档收齐后统一交付。
- 派发 skill 与每次 start/resume 的独立 Kero 上下文携带简短检查点，旧 worktree 的恢复也能收到；不覆盖原任务/历史上下文，不新增运行期监控、自动换模型或杀进程。
- verify runner 自动识别直接 `cargo [ +toolchain ] fmt`，跳过共享构建槽，保留日志、退出码与时序，并记录 `build_slot_policy`。其他命令和包装器默认串行；格式化仍遵守同一 worktree 的单一写入者和先格式化后构建顺序。既有显式 `--no-lock` 行为未改，不应拿它绕过重检查。
- runner 测试使用独立 mutex，避免验证工具自身排在真实 Agent 构建后；增加真实 Cargo fmt 在锁被占用时的失败、格式化、成功检查链路。派发生命周期测试验证新检查点随启动/原会话恢复传入，保留原任务及裁决历史。

## 验证与边界

Windows 本地 `node --test scripts/verify.test.mjs` 15/15 通过（约 12 秒），真实 fmt 三次调用在独立锁持有期间全部完成且构建槽等待为 0；`node --test .agents/skills/githux-agent-dispatch/scripts/dispatch.test.mjs` 11/11 通过（约 48 秒），覆盖新上下文注入及原会话恢复。skill-creator 的 `quick_validate.py` 与 `git diff --check` 通过。

没有修改产品源码、模型配置、CI 路径策略、审查或合并门槛；本次不需要编译整个桌面应用。提示约束的长期节时效果需要后续真实任务观察，不能由脚本测试推断 Pi 一定不会空转。
