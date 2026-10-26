# 固定版本 blob 正文服务

更新：2026-10-25。R3 的底层正文切片，位于 `src/domain/blob.rs`、`src/github/blob.rs` 和 `src/content/mod.rs`。它消费[仓库快照服务](repository-snapshot-service.md)的对象标识，还没有接入 data 或 UI。

## 身份与调用契约

`BlobIdentity::from_file(&DirectoryTree, index)` 只接受普通文件（包括 executable），拒绝目录、symlink、submodule 和未知类型；路径从父目录与条目拼接。`BlobIdentity::from_readme(&ReadmeDescriptor)` 保留 README 的仓库、commit、path、blob 和 size。字段私有，通过 getter 读取。句柄中没有 URL 或 PAT。

`fetch_blob(client, pat, &identity, &limits, cancel)` 只请求 `GET /repos/{owner}/{repo}/git/blobs/{完整blob SHA}`，强制 `Accept: application/vnd.github+json`，沿用 API 版本与 User-Agent。不会请求 branch、响应中的 url、download_url 或外部资源；分支移动不改变已派生句柄的请求。返回 `BlobRead` 同时携带原 identity 与 `BlobBody::Bytes` 或 `TooLarge { size, limit }`。

低层仍由调用方传入 `Pat`，服务不读系统凭据。身份中的仓库 ID/固定 SHA 不能代替授权检查；后续 data 接线仍需捕获账户、授权及选择代次，在成功和失败后复查；UI 回写还需核对页面会话与资源版本。本任务没有修改 `src/data/**`。

```rust
let identity = BlobIdentity::from_file(&directory, file_index)?;
// README: BlobIdentity::from_readme(&descriptor)
let limits = BlobLimits::default();
let blob = fetch_blob(client, pat, &identity, &limits, cancel).await?;
let prepared = prepare_content(blob, limits.decoded_bytes);
```

## 有界传输与校验

| 阶段 | 默认限制 | 行为 |
| --- | --- | --- |
| 已知 descriptor size | 2 MiB 解码大小 | 超限直接返回 TooLarge，不请求网络；已取消操作优先返回 Cancelled |
| HTTP JSON | 8 MiB | 真正 reqwest 在每个 chunk 加入缓冲前检查累计字节；Content-Length 仅作提前拒绝提示 |
| base64 | 2 MiB 实际解码大小 | 先按非换行 base64 长度和 padding 算出输出大小，再分配有界缓冲；实际输出必须匹配响应 size |
| 内容准备/结果 | 调用方传入 max_bytes | 再检查字节数；接受的 UTF-8 消费并复用原 Vec 分配，不保留 base64 或 JSON |

`BlobLimits { decoded_bytes, response_bytes }` 是可配置工程限制。2 MiB 的普通 base64 约 2.67 MiB；8 MiB 也给换行、JSON 转义和元数据留下空间。极端转义或元数据可先触发 JSON 限制，即使实际文件较小。HTTP 超限是 `RepositoryError::LimitExceeded`，不是正常空文件；读取阶段无法得知真实 decoded size 时，不伪造 TooLarge 的文件大小。

响应必须为 200 且无 SSO partial，JSON 必须包含 sha/encoding/size/content。SHA 必须与请求相同（大小写经 ObjectSha 规范化），encoding 必须为 base64；响应 size 必须与已知 descriptor size 相同，解码实际长度必须与响应 size 相同。移除 CR/LF 后使用严格 base64 解码，拒绝其他空白、非法字符、padding 和非零尾部位。未知/虚假 size 不能触发超预算解码或把超限正文作为 Bytes 返回。此处核对服务端 SHA 字段，不另行计算 Git object hash。

401、403、SSO、404、主/次限流、网络、超时、取消沿用类型化错误。404 表示不存在或当前不可见，不表示空正文；错误信息不插入不可信 JSON 字段、正文或授权头。已知 size 超限的本地结果不是本次授权成功证据。

`Transport::execute_bounded` 作为默认失败的新方法保留已有 implementor 的编译兼容性：自定义 Transport 未实现时返回 `BoundedReadUnsupported`，绝不退回完整读取后检查。生产实现必须用 `ReqwestTransport::new`；旧的 `with_client` 仍保留原 execute 行为，但由于无法检查注入 client 的自动重定向策略，其有界路径明确拒绝执行。MockTransport 检查 fixture 字节数；接收阶段的硬限制只由真实 socket 测试证明。

生产有界路径沿用手动同源（scheme+host+port）跳转、最多 5 跳与每跳配置超时。重定向 body 被丢弃不累计，最终响应（成功或错误）逐块受限；不跟随的跨源/降级跳转返回无 body 的 3xx。每次 chunk 等待参与取消竞争，body 超时仍为 Timeout；缺失、低报、错误 Content-Length 和 chunked 不会让累计缓冲超预算。低报 Content-Length 时 HTTP framing 只把声明部分暴露为 body，尾随数据不会进入 JSON；声明截断 JSON 仍校验失败。底层 HTTP 栈/当前单个 chunk 与解析元数据有额外开销，此限制不是进程 RSS 总上限。

## 内容分类

`prepare_content` 返回保留身份的 `PreparedContent`：

- `Empty`：仅零字节成功文件；BOM-only 仍是非空 UTF-8 文本。
- `Utf8 { text, has_bom }`：严格 UTF-8，原样保留 BOM、中文、换行和现有字符；不做 replacement 解码、换行归一化或按扩展名推断。
- `Unsupported(NonUtf8)`：非法 UTF-8，以及当前不支持的 UTF-16/其他编码。
- `Unsupported(BinaryControlCharacters)`：有效 UTF-8 但含 NUL 或除 TAB/CR/LF 之外的控制字符（含 C1）。这是保守的文本策略，可能拒绝含控制序列的文本，不声称识别所有二进制格式。
- `TooLarge`：来自读取限制或准备阶段预算，保留 size/limit，不保留正文。

路径和 commit 供以后解析相对资源；本任务不解析 Markdown AST、不下载图片、不渲染、不高亮、不编辑，也没有业务正文磁盘缓存、离线或自动重试。

## 官方依据与验证边界

实现前无认证读取 [GitHub REST Git blobs / Get a blob](https://docs.github.com/en/rest/git/blobs#get-a-blob)，核对固定 `file_sha` 路径、JSON sha/size/encoding/content、base64 内容、JSON 与 raw 媒体类型；官方支持最大 100 MB，本服务采用更小的 2 MiB 默认工程限制。

fixture 与真实 localhost 测试使用合成凭据；不读取用户 PAT、gh token 或 Cookie，不做真实 GitHub 私有仓库/SSO/TLS 联调。测试覆盖固定 SHA、错误元数据/编码、分类、分阶段限额、HTTP chunked/无 Length、低报/声明超限、取消/超时、同源重定向与跨源凭据保护。运行证据见[开发日志](devlog/2026-10-25-blob-content-service.md)。

新增直接依赖 `base64 =0.22.1`，许可证 MIT OR Apache-2.0；该版本原已存在于锁文件传递依赖中，仅新增本 package 的直接依赖关系，未升级 GPUI、Ely 或 Rust。
