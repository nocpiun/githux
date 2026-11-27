# 补齐阅读图片链路的三个已锁定 crate 许可文本

日期：2026-11-27。任务 `current-dependency-notices`，分支 `fix/current-dependency-notices`，基线 `f9784ab76674fb39dd9b63b60779e87b4509e3d6`。执行者：基层 Agent。交付形式：PR（创建后补链接）。首次发布 milestone #1。

## 目标与状态

关闭已验证的 R6 打包缺口：当前应用实际使用 `chrono 0.4.45`（Reader 图片 HTTP `Date` 解析）、`sha1_smol 1.0.1`（Git blob 对象校验）与经自有 path crate 引入的 `fast_image_resize 5.1.4`（SIMD 图片重采样），但 notice 索引和 CI artifact 的 staging 清单都没有它们的许可文本。

当前状态：实现、本地离线验证与文档完成，待提交、CI/Codex 与最新 head 核验。不是完整法律审查，不改依赖、`Cargo.lock`、运行时行为、平台范围或 CI 矩阵。

## 改动与决定

- 复现：`chrono`、`sha1_smol`、`fast_image_resize` 出现在 `Cargo.lock` 与源码引用中（`src/network/reader_images.rs`、`src/github/blob/image.rs`、`src/ui/profile/view.rs`、`crates/image-resize`），但 `stage-third-party-notices.mjs` 的 `NOTICE_PATHS` 与 `THIRD_PARTY_NOTICES.md` 都没有这三个 crate。
- 从本机 registry 已缓存的精确版本归档解出许可原文，按归档字节复制为四份文件：
  - `assets/licenses/chrono-0.4.45-license.txt` ← `chrono-0.4.45/LICENSE.txt`（同一文件含 MIT 与 Apache-2.0 全文）
  - `assets/licenses/sha1_smol-1.0.1-license.txt` ← `sha1_smol-1.0.1/LICENSE`（BSD-3-Clause，另含 `src/simd.rs` 的 MIT 段落）
  - `assets/licenses/fast_image_resize-5.1.4-license-apache.txt` ← `LICENSE-APACHE`
  - `assets/licenses/fast_image_resize-5.1.4-license-mit.txt` ← `LICENSE-MIT`
- 归档 SHA-256 与 `Cargo.lock` 的 `checksum` 完全一致（chrono `1aa79e62…3327`、sha1_smol `bbfa15b3…705d`、fast_image_resize `d372ab32…89cd`）。没有下载新依赖、没有安装、没有改动 lockfile。
- `NOTICE_PATHS` 从 16 增至 20 个 canonical 路径；四份新文本随 artifact 以仓库相对路径落地，索引链接可直接解析。既有 18 个文件（16 canonical + 2 个旧字体名）与 Windows SDK 规则未改。
- 明确区分 `githux-image-resize`（我们自己的 `crates/image-resize`，`publish = false`）与上游 `fast_image_resize`；notice 只覆盖上游 crate。
- 索引与资产清单在 `THIRD_PARTY_NOTICES.md`、`docs/third-party-assets.md` 第 5/8/9 节更新，记录版本、SPDX、版权行、归档 checksum、归档内路径与文本 SHA-256，并保留“其余直接/传递依赖文本与双许可选择未解决”的事实表述。
- 新增两条离线回归：固定四份文本 SHA-256 并从 `Cargo.lock` 比对归档 checksum，防止文本被替换或与锁文件脱钩；要求两份索引里的 `assets/licenses/` 链接都存在、相对解析且被 staging。staging 测试同时断言 canonical 路径数仍为显式的 20。

## 验证

未提交工作区证据，最终以 PR 最新 head 的 CI 为准：

- `sha256sum` 三个 `.crate` 归档与其解出的四份文本：与 `Cargo.lock` checksum、上表记录一致。
- `node --test .github/scripts/stage-third-party-notices.test.mjs`：8/8 通过（原 6 项 + 2 项新增）；fixture 报告 `Staged 22` / `Verified 22` notice 文件。
- 行尾：回归按 LF 归一后比对文本摘要；在本工作区把四份新文件改为 CRLF 再跑一次仍 8/8 通过，避免 Windows `core.autocrlf` 检出把摘要校验变成环境噪声。
- 未运行 Cargo：本次是打包与文档变更，没有 Rust 源码或依赖改动。
- 未覆盖：没有下载四平台 artifact；其余依赖文本、双许可选择与图标实际链接集合仍未解决，本任务不声明 R6 整体验收完成。

## 后续

- 等最新 head CI（`CI` 汇总、`changes` 的 notice 测试与四平台 build）、Codex 审查与线程状态，然后按工作流 squash 合并。
- 本任务全程若未出现待裁决事项，基层 Agent 自行合并；出现即转负责人或主 Agent。
