# 2026-11-24 PR Conversation

- 分支 `feat/pr-conversation` 从 `f888a31` 开始。最小复现是点击 Feed 已有的 PR Read more。基线的 `feed_read_more_opens_and_reuses_the_internal_pr_tab` 覆盖内部路由和标签复用，正式 `Shell` 对该路由返回空挂载，`tab-refresh` 随之禁用。这轮先实现 PR 独立切片。
- 查阅 GitHub 官方 GraphQL PR 字段以及 REST PR、普通评论接口。匿名 `GET /repos/nocpiun/githux/pulls/100` 用应用的 REST 版本 `2022-11-28` 核对响应形状。对同一公开 PR 执行只读 GraphQL 查询，仅输出字段类型，确认 `MERGED`、非空 `headRefName`、`headRef: null`、可见 `headRepository`。离线探针使用自造仓库、作者、分支、正文和凭据。 [GraphQL PR 字段](https://docs.github.com/en/graphql/reference/pulls#pullrequest)
- The first slice added the PR model, account-scoped GraphQL read and GPUI Markdown Flow content. It used a standalone probe before the shared comment controller was available. [Conversation contract](../pr-conversation.md)

## Independent validation on November 24

```sh
node scripts/verify.mjs -- cargo fmt --all
node scripts/verify.mjs -- cargo run --locked --example pr_conversation_probe
```

该轮格式化通过（`ecc78e39`）。独立离线探针通过（`7776379c`），共享构建槽等待 921.0 秒，执行 17.3 秒。覆盖四种状态、来源分支删除、空白和非空正文、GraphQL 权限错误、错误资源号、A → B → A 账户隔离及 GPUI 刷新状态。正式 Shell 回归接替该探针。

## 2026-11-26 负责人验收返工

按负责人授权，以 #106 的 `1ac899e22a4e7f1da42c71ecd85b87ff5705666a` 为初始 stacked base，随后同步到 `ab24dd949424d392400abc5b5cb23828b662039d`。在原分支挂载 `PullRequestPage`，通过 `Controller::begin_comments`、`CommentsRequest::execute_online` 和 `Controller::finish_comments` 复用普通评论读取、分页及错误处理。PR 的 GraphQL 正文走自身读取会话。页面使用 Shell 的标签滚动、顶部刷新和 Markdown Flow。空正文和空评论分别给出明确状态。

`node scripts/verify.mjs -- cargo test --locked --bin githux app::pages::` 通过全部 75 项（`744db403`）。共享构建槽等待 144.0 秒，命令执行 65.2 秒。回归从 Feed 的实际 Read more 检查 `alice/demo #5` 的 Markdown 正文和评论、同账户标签复用、第二页、503 重试和 403 撤下内容，也覆盖 `alice/demo #7` 的空状态及 #106 的既有 Issue 路径。真实 Windows/PAT 交由负责人验收。

## GitHub 审查修复

初审发现正文与评论未接图片 provider，仓库相对链接也缺少目的地。复用 #106 的账户作用域图片 provider，并从 PR base 仓库建立 Markdown 上下文。Shell fixture 验证正文和评论中的仓库 SVG 实际渲染，并在点击正文链接后核对 GitHub HEAD 地址与片段。串行页面回归通过全部 76 项（`f07061fc`），构建槽等待 84.3 秒，命令执行 87.4 秒。同步 #106 后较早的并行运行（`dfe28820`）中，#106 新增的冷启动 Issue 用例有一次停在 Home，独立执行及串行整组均通过。

## 合入 main 的基线

#106 squash 为 `951da323a114d3d2dd6e8cb2955779c12f2f1a13`，文件树与已审查的生产者快照 `ab24dd9` 一致。#109 保留原 PR，把 PR Conversation 差异重放到该提交并改以 `main` 为目标。按负责人决定，最终外观由 Issue/PR 阅读批次共同验收。
