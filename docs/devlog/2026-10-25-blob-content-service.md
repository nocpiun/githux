# R3 固定版本 blob 正文与有界传输

任务：`blob-content-service`；基线 `262a013a03ef79b6d0b072097df57f019a4a7a1a`；分支 `feature/blob-content-service`。

## 实现

新增 `domain::blob`：从目录普通文件条目或 README descriptor 派生 `BlobIdentity`，保留 repository/commit/path/blob/size，拒绝 symlink/submodule/目录/未知类型。新增 `github::blob::fetch_blob`，仅固定 SHA Git blobs JSON 请求，校验 SHA、encoding、descriptor size、严格 base64 与实际解码大小，返回字节或明确 TooLarge；401/404/限流/取消/错误不会变成空文件。

`content::prepare_content` 消费正文并分类 Empty、严格 UTF-8（原样保留中文/BOM/换行）、非 UTF-8/控制字符二进制和 TooLarge。文本复用字节分配，不保留 encoded JSON 或 base64 副本。默认正文预算 2 MiB、JSON 8 MiB，分别可配置。

`Transport::execute_bounded` 默认失败以兼容旧 implementor 且不退回无界读取；`ReqwestTransport::new` 使用真实 chunk 累计检查，适用于成功和错误响应。Content-Length 只用于提前拒绝，最终仍检查实际 chunk；保留取消、typed timeout、同源重定向、固定头与请求 Debug 凭据脱敏。`with_client` 无法核实注入 client 的自动跳转配置，因此其有界路径明确拒绝执行，原 execute 保持可用。

官方依据：实现前无认证下载并核对 [Git blobs](https://docs.github.com/en/rest/git/blobs#get-a-blob) 的 file_sha 路径、base64 JSON 字段、raw/JSON 媒体类型与 100 MB 服务端上限。新增直接依赖 base64 0.22.1（MIT OR Apache-2.0），原已存在于锁文件传递依赖中，锁文件仅增加本 package 的直接依赖关系。没有升级工具链、GPUI、Ely，也没有改 data/auth/UI/workspace/app。

完整接口与工程限制见 [blob 内容服务](../blob-content-service.md)，仓库快照文档同步区分原快照 execute 与新正文 bounded 路径。

## 验证

fixture 覆盖 README/普通文件固定 SHA（模拟分支已移动）、身份与路径保留、symlink/submodule 拒绝、空文件、中文/BOM/BOM-only、换行 base64、非法字符/padding/尾部位、SHA/encoding/size 错误、二进制 `.md`、非 UTF-8、不同阶段限额与取消。真实 localhost 串联 fetch_blob、ReqwestTransport、内容准备，并核对请求路径/头与截断 JSON 失败。

真实原始 socket 测试发送未结束的 chunked/无 Length 响应，客户端必须在 EOF 前报告超限，成功和错误状态均覆盖；另测声明超限、低报 Length、TE+CL、非法 Length、精确预算、body 取消/超时、同源跳转目标仍受预算约束、跨源目标从未接收请求、注入自动跳转 client 的有界路径拒绝。已有授权脱敏、畸形 Location、origin 解析等回归随全量测试执行。

首次完整测试中的跨源测试清理阻塞已定位：未被访问目标的 LocalHttpServer 被 Drop 唤醒后仍等待请求头；清空其未消费脚本后再清理。已停止旧测试进程，修正版重新验证。clippy 的 is_multiple_of 提示已修正。

最终 Windows 本地检查全部通过：`cargo fmt --all -- --check`、`cargo check --locked --all-targets`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo test --locked`（182 passed，0 failed）、`cargo build --locked` 和 `git diff --check`。PR 最新 head 的四平台 CI、Codex 审查、合并信息保存在 PR 与本任务 delivery-state，避免为记录最终状态新增未经检查提交。本地构建不替代云端跨平台检查或真实阅读器验收。

## 剩余集成

正文 data 接线及账户/授权/选择代次封装留给后续小任务；未修改并行 repository-data-service 的文件。未实现 Markdown AST/渲染、源码高亮、图片/相对资源下载、UI、业务正文缓存或离线，也未迁移既有快照接口到 bounded transport。测试仅合成凭据和 localhost，无用户 PAT/gh token/Cookie 或真实 GitHub 私有/SSO/TLS 联调。首版 milestone #1；当前无待裁决项，后续出现裁决按永久记录规则处理。
