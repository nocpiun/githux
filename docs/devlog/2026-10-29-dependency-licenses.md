# 依赖与第三方资产许可核验

日期：2026-10-29。任务 `dependency-licenses`，分支 `docs/dependency-licenses`，基线 `730d2292dcd5bb981928341259c971e42ff497b5`。交付形式：PR。

## 目标与范围

核验当前首版**实际分发**的字体、图标、提取的 Markdown 代码和依赖的来源与许可，补齐可追溯清单与已明确要求保留的 notices，为后续打包提供依据。不升级依赖、不改项目许可证、不把清单当法律结论或完整发布验收。

## 本轮结论

- 新增 [第三方资产与依赖许可清单](../third-party-assets.md)：给出字体/图标/Ely/GPUI/复制代码/直接依赖的来源、固定版本、哈希、许可文本、分发形态与保留文件，并区分已验证/未覆盖。
- 新增根 [THIRD_PARTY_NOTICES.md](../../THIRD_PARTY_NOTICES.md)：分发用 notices 索引与打包随附清单。
- 新增 `assets/licenses/`：按上游原文复制 `ely-mit.txt`、`lucide-icons-isc.txt`、`ibm-plex-sans-ofl.txt`、`gpui-pre-apache-2.0.txt`、`pulldown-cmark-mit.txt`、`sqlite-public-domain.txt`，并记录 SHA-256。
- 更正 [`assets/fonts/README.md`](../../assets/fonts/README.md)：删除已不存在的 Hello 原型描述，改为当前 UI=Noto Sans、代码=Google Sans Code；把“Ely additionally bundles its own upstream assets and fonts”改为本构建事实（`ely-core` 无条件附带 IBM Plex Sans 与 Lucide 图标，Inter / JetBrains Mono 因 `default-features = false` 不分发）；补充相邻 OFL 未编译进 binary、打包需单独随附。

## 关键事实与证据

| 项 | 结论 | 证据 |
| --- | --- | --- |
| 打包字体 | 只有 `NotoSans-Regular.ttf`、`GoogleSansCode.ttf` 由 `src/platform/fonts.rs` `include_bytes!` 编入 | 仓库内仅这两处 `include_bytes!`；SHA-256 与 `assets/fonts/README.md` 一致 |
| 字体许可 | 两者 SIL OFL 1.1，相邻 `OFL.txt` 各自匹配（仅版权行与 FAQ 链接不同）；**OFL 未编译进 binary** | `sha256sum` 重新计算；逐字节 diff |
| Ely 附带字体 | IBM Plex Sans（OFL）无条件编入；Inter / JetBrains Mono 因关闭 `fonts` feature 不编入 | `ely-core @ 30c66fe1` 的 `assets.rs`：`PLEX` 无 feature gate，`FONTS` 有 `#[cfg(feature = "fonts")]`；`Cargo.toml` 对 `ely-core` 使用 `default-features = false` |
| 图标 | 未复制进仓库；由 `ely-core` 的 `IconName` 宏按引用 `include_bytes!`，ISC + Feather MIT | `ely-core @ 30c66fe1` 的 `primitives/icon/names.rs` 与 `icons/LICENSE`；本仓库引用 30 个 `IconName::*` |
| Ely / GPUI | `ely-core 0.2.0` / `ely-settings 0.1.1` / `ely-canvas 0.1.1` @ `30c66fe1`，MIT；`gpui-pre*` 全部 `0.3.10`，Apache-2.0 | `Cargo.lock` 与 `cargo metadata`；上游 `LICENSE` / `LICENSE-APACHE` |
| 复制代码 | `src/ui/reader/markdown/` 派生自 `ely-documents`（MIT），映射见 `content-reader-markdown-patch.md` | 源文件头注释与主题文档 |
| 依赖清单 | `cargo metadata --locked --offline` 解析 748 个 package；直接依赖版本与 SPDX 见清单 | 命令输出；SPDX 为声明，非逐项合规核验 |
| Windows App SDK | 固定 NuGet 包；脚本已把每个包的 `license.txt` 复制到可执行文件旁 | `scripts/windows-runtime-lib.ps1` 及其测试 |

## 验证

- `cargo metadata --locked --offline --format-version 1`：成功（Rust 1.98.1），共 748 个 package。
- 字体与许可文件 `sha256sum`：`NotoSans-Regular.ttf` `478c558e…`、`GoogleSansCode.ttf` `74642762…`，与 `assets/fonts/README.md` 一致；两份 `OFL.txt` 各自匹配。
- GitHub API 核验 release tag：`NotoSans-v2.015`（commitish `c4a321e1…`）、`v7.001` 均存在。
- `git diff --check`（见交付前检查）。
- 纯文档改动，未重跑全量 Rust 编译；未改动任何 Rust 源码、`Cargo.lock`、CI、根 README 或字体/图标二进制。

## 未覆盖 / 后续

- 未反汇编最终 binary，图标实际链接集合与 `ely-canvas` 是否被链接未固定。
- 传递依赖只有 SPDX 声明分类，未逐项读取 748 份许可原文，也未把 build/dev/非目标平台 crate 从分发集合剔除。
- 未重新下载核验字体归档 zip 的 SHA-256。
- 未选择任何双许可项（如 `self_cell` 含 GPL 选项），未判断商业兼容。
- 未产出安装包；打包随附文件清单见清单第 8 节，**打包尚未完成**。
- 未新增 `scripts/licenses/` 脚本：`.github/scripts/ci-scope.mjs` 把任何非文档/非 skills 路径（包括 `scripts/**`）归为应用改动，会触发全量 Rust code-check 与四平台构建；本任务保持纯文档交付，清单用文档化命令复现。若后续要脚本化，需同时评估 CI 范围策略。

## 与其它任务的边界

只写 `docs/third-party-assets.md`、`THIRD_PARTY_NOTICES.md`、`assets/fonts/README.md`、`docs/devlog/2026-10-29-dependency-licenses.md` 与 `assets/licenses/`。未触碰 reader-images（reader）、shell-pages（app/页面接线）、release-plan（总览）的写入范围。
