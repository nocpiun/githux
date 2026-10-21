# PR 线性提交历史约定

- 日期：2026-10-21（Asia/Shanghai）；任务 ID：linear-history-policy；执行者：主 Agent。
- 状态：文档更新；本任务尚未创建 PR。

## 改动与决定

按负责人要求，后续 PR 必须 squash merge，并保留 GitHub 默认生成的提交标题与正文。规则维护在 [Agent 工作流](../agent-workflow.md#提交历史与合并方式)，同步根 AGENTS 和 PR 审查 skill。保留最新 head 的 CI/review、裁决历史与合并权限门槛；本次不修改仓库设置或重写已合并历史。

## 验证

已核对 GitHub 官方文档和 gh 2.101.0 源码：显式 `--squash` 且省略提交信息参数时，CLI 不传自定义标题/正文。未提交工作区的 `git diff --check` 通过，已复核共享入口、流程和 skill 的规则一致性。通过 `uv run --no-project --with pyyaml` 在隔离环境运行 `quick_validate.py`，skill 格式校验通过；本次未改 frontmatter 或等待脚本。未执行实际合并，本项文档变更未单独运行 Rust 测试；同批 CI 变更的验证见 [CI 开发日志](2026-10-21-ci-matrix.md)。
