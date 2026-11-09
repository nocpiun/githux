# Native Shell 迁移：路线与第一阶段

日期：2026-11-09。

负责人确认完整迁移到 Native Shell 原生 Windows 后端，继续使用纯 Rust，以 main 现有 UI 为基线。迁移结果保存在 migration/native-shell，暂不合并 main；十个 UI 赛马候选保留，未纳入迁移。

协调者核对 main 11f63e8、NSH v0.4.0 tag 对应 e626a8dd8f8d8b277b9507cfd5ac74d0e67020f3，并建立原生基础/集成、运行时桥接、Reader 三个独立任务的所有权边界。公共契约与最小可运行证据就绪后，再派发页面迁移。

详见 [迁移计划](../native-shell-migration.md)。本提交仅记录已确认方向和实施计划，没有声明依赖已编译、原生行为已验收或应用迁移已完成。后续实现者分别补充自己的代码和验证证据。