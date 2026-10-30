# 第三方资产与依赖许可清单

更新：2026-10-29。任务 `dependency-licenses`，基线 `730d2292dcd5bb981928341259c971e42ff497b5`。

本文记录当前首版**实际分发**的字体、图标、复制的 Markdown 代码、Rust 依赖与 Windows App SDK 运行时的来源、固定版本、哈希、许可证文本与实际分发形态，并区分已验证与未覆盖。它是后续打包的依据，不是法律结论，也不代表整个应用已通过发布验收。

> **打包尚未完成。** 源码仓库里存在许可文件，不等于未来安装包/可执行文件会自动携带。本文末尾列出发布时必须随附的文件。当前没有任何安装包或发布产物，只有源码树与开发构建。

## 范围与方法

- 只核对本基线真正进入分发的对象：`Cargo.lock` 解析图、`assets/` 中的字体与 manifest、`src/platform/fonts.rs` 与 `ely-core` 的 `include_bytes!`、复制的 `src/ui/reader/markdown/` 源码、Windows App SDK 运行时。不扫描上游全部文件，也不把整库搬进仓库。
- 依赖清单来自 `cargo metadata --locked --offline --format-version 1`（Rust 1.98.1）。其 `license` 字段是 **SPDX 声明**，用于筛选与分类；**SPDX 字符串不等于已逐项核验全量许可合规**。
- 固定版本的许可文本从本机已获取的 cargo registry 源码与 Ely git checkout 按精确版本/修订读取；本机缺失的固定版本从官方精确 tag/revision 查证。
- 未升级依赖、未改 `Cargo.lock`、未改缓存源码、未改项目许可证、未复制或替换字体/图标二进制。

### 复现命令

```sh
cargo metadata --locked --offline --format-version 1
```

`--offline` 要求本机缓存已有 lock 中的固定版本；缺少时先让 cargo 获取一次（不改变 `Cargo.lock`），之后即可离线复现。下面“直接依赖”表的版本与 SPDX 即由该命令产出。

## 1. 字体

`src/platform/fonts.rs` 嵌入四个应用私有 UI 字面（Githux UI）与原 Google Sans Code。UI 衍生字面保留 Noto Sans v2.015 的全部 2,965 字符映射，补入 Noto Sans SC 2.004-H2 的 30,361 映射；400/500/600/700 均使用源字体对应权重，主字族改名以避免与上游/系统字体混淆。官方固定来源、每个文件体积/哈希与可复现生成过程统一维护于 [字体清单](../assets/fonts/README.md)，生成脚本为 [build-ui-fonts.py](../scripts/build-ui-fonts.py)。

- 四份 UI 字面共 43,707,220 B（41.68 MiB），替换旧 Latin-only 的 1,866,600 B。仅包含这两个 Noto 来源，没有引入其他语言字体集。
- Noto Sans 与 Noto Sans SC 均为 SIL OFL 1.1；随包携带 `assets/fonts/githux-ui/OFL-NotoSans.txt`、`OFL-NotoSansSC.txt`。SC 来源原声明含 Reserved Font Name `Source`，衍生字体使用 `Githux UI` 名称。两份来源的版权和许可文本保留，未改写。
- Google Sans Code v7.001 原文件 268,264 B，SHA-256 `74642762fae66d5883077794a763e1086394b8cb72f2623e456e06359dc5c64b`；字体与许可均未改。
- CI artifact 打包步骤同时携带两份 UI 来源 OFL 及 Google Sans Code OFL。许可文本不嵌入 binary，单独分发时仍必须随附。
- Windows GPUI 0.3.10 的显式 fallback 查询只使用系统字体集合。合成字面让中文直接命中应用内注册字体，不依赖系统安装；逐映射轮廓/advance 比对和原生 shaping 证据见[字体开发日志](devlog/2026-10-29-ui-fonts.md)。

### 上游 Ely 附带的字体（非本仓库文件）

链接 `ely-core` 时，`ely-core` 的 `Assets` 无条件嵌入 IBM Plex Sans（GPUI 的 SVG 渲染器使用），本构建同样进入 binary：

| 资产 | 来源 | 文件 | 文件 SHA-256 | 许可证 | 备注 |
| --- | --- | --- | --- | --- | --- |
| IBM Plex Sans Regular | `ely-core` @ `30c66fe1`（`crates/ely-core/assets/plex/`） | `IBMPlexSans-Regular.ttf` | `975dcda37d80f038dcd143c22e33ca2d97a0cc5a929aace1c749153b0fe1afa5` | SIL OFL 1.1 | 上游把 `IBMPlexSans-LICENSE.txt` 也作为 asset 一起 `include_bytes!`，即许可文本本身进入 binary；仍建议在 notices 中重复保留 |
| Inter / JetBrains Mono | `ely-core` @ `30c66fe1`（`crates/ely-core/assets/fonts/`） | — | — | SIL OFL 1.1 | **本构建不分发**：Githux 对 `ely-core` 使用 `default-features = false`，`fonts` feature 关闭，这两个字体及其许可文件都不被编译 |

## 2. 图标

应用通过 `ely_core::primitives::IconName` 使用图标，**没有**把任何图标复制进本仓库。

| 项 | 内容 |
| --- | --- |
| 来源 | `ely-core` @ `30c66fe1d5d23db769a6c834b771d054d7f288cf`，`crates/ely-core/icons/`，307 个 Lucide SVG + 1 个 `LICENSE` |
| 许可证 | ISC（Lucide 图标与贡献者）+ MIT（其中派生自 Feather 的子集，名单见 `LICENSE`） |
| 分发形态 | `names.rs` 的宏为每个图标生成 `include_bytes!` 静态量；被引用的图标才会链接进 binary |
| 上游许可文本 | 已按原文复制到 [`assets/licenses/lucide-icons-isc.txt`](../assets/licenses/lucide-icons-isc.txt)（SHA-256 `b495047bd93a9b06913511076f504daba17d5bbeb3e0650f3bb53a4220329c57`） |
| 已验证 | 本仓库源码引用的 `IconName::*` 共 30 个（见下）；`ely-settings` 非测试代码另引用 7 个；`ely-canvas`（`ely-settings` 的依赖）引用 33 个 |
| 未覆盖 | 未反汇编最终 binary，无法逐项确认链接器实际保留的图标集合；`ely-core` 自身 `forms` 代码还引用约 110 个，但 `IconName::ALL` 只被未使用的 `IconPicker` 与测试引用，**按引用链推断不应进入 binary**。因此上界是 307，实际分发子集未在本轮以二进制证据固定 |

本仓库引用的 30 个图标：

`Bell, BookOpen, Check, ChevronDown, ChevronLeft, ChevronRight, CircleAlert, CircleDot, Code, Ellipsis, File, FileText, Folder, GitBranch, GitPullRequest, House, Link, MapPin, Minus, Package, Plus, RotateCw, Search, Settings, Star, Tag, TriangleAlert, User, Users, X`

（`ely-settings` 侧：`Globe, Monitor, Moon, Palette, RotateCcw, Search, Sun`。）

## 3. Ely 与 GPUI

| 组件 | 固定版本 / 修订 | 许可证 | 实际用法与分发形态 |
| --- | --- | --- | --- |
| `ely-core` | git `30c66fe1d5d23db769a6c834b771d054d7f288cf`（2026-10-26） | MIT（`Copyright (c) 2026 Ely GPUI Component contributors`） | 直接依赖，`default-features = false` + `runtime_shaders`；代码编译进 binary，另附带 IBM Plex Sans 与 Lucide 图标 |
| `ely-settings` | 同上修订 | MIT | 直接依赖（`src/ui/accounts/form.rs` 使用 `SettingsRow` / `SettingsSection`）；代码编译进 binary |
| `ely-canvas` | 同上修订 | MIT | `ely-settings` 的传递依赖；`Cargo.lock` 中已解析。本应用只引用 `ely-settings` 的 section/row，不引用 canvas 类型；其代码是否被链接取决于构建期的死代码消除，**未以二进制证据核验** |
| `gpui-pre` / `gpui-pre-platform` 及 `gpui-pre-*` 子 crate | `=0.3.10`（crates.io） | Apache-2.0（`Copyright 2022 - 2025 Zed Industries, Inc.`，`gpui-pre snapshot of zed@08cbb88`） | 直接依赖，`font-kit`（Windows/Linux）；所有 `gpui-pre-*` 子 crate 同为 0.3.10 / Apache-2.0；代码编译进 binary |

- 上游 Ely 工作区 `LICENSE`（MIT）原文已复制到 [`assets/licenses/ely-mit.txt`](../assets/licenses/ely-mit.txt)（SHA-256 `c69028b5957f4ded637dc55a4a412b209c014414ece49efefa60fa811a4c0312`）。
- `gpui-pre` 的 `LICENSE-APACHE`（含 Zed 版权行 + Apache 2.0 全文）原文已复制到 [`assets/licenses/gpui-pre-apache-2.0.txt`](../assets/licenses/gpui-pre-apache-2.0.txt)（SHA-256 `752daf2fb234ca4a1fa372c073fe127f44b7b90fd2529ae44273a64f9d53da7a`）。
- 没有整库/单 crate vendor、Cargo patch、fork 或缓存修改；`ely-documents` / `ely-editor` **不是**依赖（Markdown 组件已提取，见第 4 节）。
- 观察到的文档不一致：根 `README.md` 仍写“当前外壳直接依赖 `ely-core 0.2.0`”，但本基线同时直接依赖 `ely-settings`（账户表单使用）。本任务不修改根 README，仅在此记录。

## 4. 复制的 Markdown 源码

`src/ui/reader/markdown/` 由 Githux 维护，提取自 Ely 的 `ely-documents` reader @ `30c66fe1`（MIT）：

| 本地文件 | 上游来源 | 处理 |
| --- | --- | --- |
| `document.rs` | `ely-documents/src/lib.rs`、`document/mod.rs`（Preview 路径） | 裁剪为只读 |
| `inline.rs` | `render.rs`、`render/style.rs` | 重写 inline 排版 |
| `ast.rs` | `tree/*`、`markdown/*`（mdast） | 用 `pulldown-cmark` 事件流重写 |
| `doctext.rs`、`mod.rs` | — | Githux 自身新增 |

- 上游 MIT 适用于派生文件；映射、改动与维护规则见 [`content-reader-markdown-patch.md`](content-reader-markdown-patch.md)。源文件头部均保留来源与许可证注释。
- 派生自 Ely 的部分与 `ely-core` 同受 [`assets/licenses/ely-mit.txt`](../assets/licenses/ely-mit.txt) 覆盖。
- 新增直接依赖 `pulldown-cmark 0.13.5`（MIT，`Copyright 2015 Google Inc.`，原文 [`assets/licenses/pulldown-cmark-mit.txt`](../assets/licenses/pulldown-cmark-mit.txt)，SHA-256 `c4f10f55904bdb9f27d3fbf94c354926d6cfe8b982276e556238c258941b243b`）与 `unicode-segmentation 1.13.3`（MIT OR Apache-2.0）。

## 5. 直接依赖

版本与 SPDX 来自 `cargo metadata --locked --offline`。**“许可证文本”列指向本机精确版本的原文；本轮只对明确标注的做了核对，其余是上游声明。**

| crate | 解析版本 | SPDX（metadata 声明） | 分发形态 | 许可证文本位置 / 备注 |
| --- | --- | --- | --- | --- |
| `base64` | 0.22.1 | MIT OR Apache-2.0 | 编译进 binary | registry `LICENSE-MIT` / `LICENSE-APACHE` |
| `dirs` | 7.0.0 | MIT OR Apache-2.0 | 编译进 binary | registry |
| `ely-core` | 0.2.0（git） | MIT | 编译进 binary | `assets/licenses/ely-mit.txt` |
| `ely-settings` | 0.1.1（git） | MIT | 编译进 binary | 同上 |
| `gpui-pre` | 0.3.10 | Apache-2.0 | 编译进 binary | `assets/licenses/gpui-pre-apache-2.0.txt` |
| `gpui-pre-platform` | 0.3.10 | Apache-2.0 | 编译进 binary | 同上 |
| `image` | 0.25.10 | MIT OR Apache-2.0 | 编译进 binary | registry |
| `pulldown-cmark` | 0.13.5 | MIT | 编译进 binary | `assets/licenses/pulldown-cmark-mit.txt` |
| `reqwest` | 0.13.5 | MIT OR Apache-2.0 | 编译进 binary | registry（`native-tls`/`http2`/`system-proxy`，关闭默认 feature） |
| `rusqlite` | 0.37.0 | MIT | 编译进 binary | registry `LICENSE`；`bundled` 另含公有领域 SQLite，见 [`assets/licenses/sqlite-public-domain.txt`](../assets/licenses/sqlite-public-domain.txt) |
| `serde` | 1.0.229 | MIT OR Apache-2.0 | 编译进 binary | registry |
| `serde_json` | 1.0.154 | MIT OR Apache-2.0 | 编译进 binary | registry |
| `thiserror` | 2.0.21 | MIT OR Apache-2.0 | 编译进 binary | registry |
| `tokio` | 1.54.1 | MIT | 编译进 binary | registry `LICENSE`（`Copyright (c) Tokio Contributors`） |
| `unicode-segmentation` | 1.13.3 | MIT OR Apache-2.0 | 编译进 binary | registry |
| `uuid` | 1.28.0 | Apache-2.0 OR MIT | 编译进 binary | registry |
| `raw-window-handle` | 0.6.2 | MIT OR Apache-2.0 OR Zlib | Windows 编译进 binary | registry（三选一） |
| `windows` | 0.62.2 | MIT OR Apache-2.0 | Windows 编译进 binary | registry（Microsoft） |
| `windows-core` | 0.62.2 | MIT OR Apache-2.0 | Windows 编译进 binary | registry（Microsoft） |
| `windows-future` | 0.3.2 | MIT OR Apache-2.0 | Windows 编译进 binary | registry（Microsoft） |
| `tempfile` | 3.27.0 | MIT OR Apache-2.0 | **仅 dev-dependency** | 不进入发布 binary |

- 传递依赖 `libsqlite3-sys 0.35.0`（MIT）随 `rusqlite` 的 `bundled` feature 把 SQLite amalgamation 编译进 binary；SQLite 为公有领域，`libsqlite3-sys` 包装层为 MIT。
- 多项是 **MIT OR Apache-2.0 双许可**：本清单同时记录两个选项，**未替项目选择其一**。发布时需决定并随附对应文本（见第 9 节）。

## 6. 传递依赖清单

`Cargo.lock` 共解析 748 个 package（含根 `githux`）。可复现命令：

```sh
cargo metadata --locked --offline --format-version 1
# 或直接统计 lock：
grep -c '^\[\[package\]\]' Cargo.lock
```

本轮按 `cargo metadata` 的 SPDX 声明统计（**声明分类，未逐项核验**）：

| SPDX（声明） | 数量 |
| --- | --- |
| MIT OR Apache-2.0 系（含 `MIT/Apache-2.0`、`Apache-2.0 OR MIT` 等写法） | 447 |
| MIT | 166 |
| Apache-2.0 | 32 |
| Zlib 系（`Zlib`、`… OR Zlib`） | 27 |
| Unicode-3.0（含 2 个 AND 组合） | 20 |
| BSD 系（2/3-Clause 及组合，含 1 个 AND 组合） | 19 |
| Unlicense 系（`Unlicense OR MIT`、`Unlicense/MIT`） | 13 |
| Apache-2.0 WITH LLVM-exception 系 | 6 |
| MPL-2.0 | 3 |
| CC0-1.0 系 | 3 |
| ISC（纯） | 3 |
| 含 GPL/LGPL 选项（`Apache-2.0 OR GPL-2.0-only`、`… OR LGPL-2.1-or-later`） | 3 |
| 其他（`Apache-2.0 OR ISC OR MIT` 2、`Apache-2.0 AND ISC` 1、`… AND NCSA` 1、`… OR BSL-1.0` 1） | 5 |

需要发布前明确处理的多许可/非典型项：

- `self_cell 1.3.0`：`Apache-2.0 OR GPL-2.0-only` — 含 **GPL 选项**，必须确认实际选用 Apache-2.0 或确认其未进入分发。
- `r-efi 5.3.0`、`r-efi 6.0.0`：`MIT OR Apache-2.0 OR LGPL-2.1-or-later`。
- MPL-2.0：`cbindgen 0.28.0`（**build 依赖**，不进入 binary）、`dwrote 0.11.5`、`option-ext 0.2.0`。
- Unicode-3.0：20 个（18 个纯 `Unicode-3.0` + 2 个与 MIT/Apache-2.0 的 `AND` 组合）。
- `encoding_rs 0.8.42`：`(Apache-2.0 OR MIT) AND BSD-3-Clause`。

**未覆盖：** 本表是 lock 解析图（含全部目标平台、build 与 dev 依赖），不是某一个 binary 实际链接的集合；未把 build-only / dev-only / 其他平台 crate 从“随发布分发”中单独剔除，也未逐项读取每个 crate 的许可证原文。发布前应基于实际目标构建再收敛一次。

## 7. Windows App SDK 运行时

| 项 | 内容 |
| --- | --- |
| 固定包 | `microsoft.windowsappsdk.foundation 1.8.260803002`（SHA-256 `B9232041AFD605B606C6F78F442D92EAD0076453F1F2A3260D2B7F8089BCAB0E`）、`microsoft.windowsappsdk.interactiveexperiences 1.8.260708001`（SHA-256 `496EEA92D353B5D3601B67353F06DCADD6D2D9B635575ACEBE6E42587DBFAD76`） |
| 来源 | NuGet `https://api.nuget.org/v3-flatcontainer`，见 [`scripts/windows-runtime-lib.ps1`](../scripts/windows-runtime-lib.ps1) |
| 分发形态 | app-local：DLL/PRI 集、`githux-wasdk.manifest` 与每个包的 `license.txt`（落地为 `<package-id>.LICENSE.txt`）放在可执行文件旁；由脚本准备，不系统级安装 |
| manifest | [`assets/windows/app.manifest`](../assets/windows/app.manifest)（SHA-256 `0d9d20203d3fef11d6f814849a4f354e4f57f0d9490f2be30532ee27b6c81bf2`）由 [`scripts/generate-windows-manifest.ps1`](../scripts/generate-windows-manifest.ps1) 从上述固定包生成；`caption_tall.rs` 在运行时按 `githux-wasdk.manifest` 名称激活 |
| 许可证 | **已由现有脚本处理**：`windows-runtime-lib.ps1` 校验并复制每个固定包的 `license.txt` 到可执行文件旁。这是仓库里少数“分发时已自动携带许可”的路径 |

## 8. 打包时必须随附的文件

当前没有任何安装包/发布产物。未来打包/分发时，至少需要随附：

1. UI 两份来源 OFL（`assets/fonts/githux-ui/OFL-NotoSans.txt`、`OFL-NotoSansSC.txt`）及 `assets/fonts/google-sans-code/OFL.txt`。
2. IBM Plex Sans 的 OFL：`assets/licenses/ibm-plex-sans-ofl.txt`（binary 内也有一份 asset，但打包仍应显式随附）。
3. Ely MIT：`assets/licenses/ely-mit.txt`（覆盖 `ely-core`/`ely-settings`/`ely-canvas` 与提取的 Markdown 派生代码）。
4. Lucide 图标 ISC + Feather MIT：`assets/licenses/lucide-icons-isc.txt`。
5. GPUI（Apache-2.0）与其余 Rust crate 的许可文本：至少 `assets/licenses/gpui-pre-apache-2.0.txt`、`assets/licenses/pulldown-cmark-mit.txt`，以及第 5 节各直接依赖的 registry 许可文本；双许可项需先定选。
6. SQLite 公有领域说明：`assets/licenses/sqlite-public-domain.txt`（信息性）。
7. Windows App SDK 各包 `license.txt`：由 `windows-runtime-lib.ps1` 复制到可执行文件旁。
8. 合并入口：[`THIRD_PARTY_NOTICES.md`](../THIRD_PARTY_NOTICES.md)。

## 9. 已验证与未覆盖

**本轮已验证：**

- 五份打包 ttf 的 SHA-256 与 [`assets/fonts/README.md`](../assets/fonts/README.md) 一致；两份 `OFL.txt` 各自与其字体匹配（仅版权行与 FAQ 链接不同）。Noto 归档的 SHA-256 本轮已按官方 release 重新下载核验为 `0c34df07…`。
- Noto v2.015 与 Google Sans Code v7.001 的官方 release tag 存在（GitHub API 核验）。
- `include_bytes!` 路径与 `assets/fonts/README.md` 的 include 描述一致；仓库内只有 `src/platform/fonts.rs` 两处 `include_bytes!`。
- Ely 修订 `30c66fe1` 的 MIT 文本、`gpui-pre 0.3.10` 的 Apache-2.0 文本、Lucide 的 ISC+Feather MIT、IBM Plex OFL、pulldown-cmark MIT 均已按原文复制到 `assets/licenses/` 并计算 SHA-256。
- `cargo metadata --locked --offline` 成功，得到 748 个 package 的版本与 SPDX 声明。
- Windows App SDK 的固定包哈希与“脚本已复制 license.txt”在 [`scripts/windows-runtime-lib.ps1`](../scripts/windows-runtime-lib.ps1) 与测试中已有覆盖。

**本轮未覆盖：**

- 字体归档已重新下载并核对 SHA-256；仍不核验归档内未打包的其他字面。
- 未反汇编最终 binary，图标实际链接集合、`ely-canvas` 是否被链接均未以二进制证据固定。
- 未逐项读取 748 个 package 的许可证原文；传递依赖只有 SPDX 声明分类。
- 未把 build-only / dev-only / 非目标平台 crate 从“分发集合”中剔除。
- 未选择任何双许可项，未判断商业兼容，未改写任何版权声明。
- 未产出安装包，未做完整发布验收。

## 10. 待裁决与不确定项

- **多许可证选择**：`self_cell`（Apache-2.0 OR GPL-2.0-only）、`raw-window-handle`（MIT OR Apache-2.0 OR Zlib）、`r-efi`（含 LGPL 选项）等需要项目决定实际采用项；本任务不自行拍板。
- **图标实际集合**：是否需要在发布前对目标 binary 做一次字符串级核对以固定图标许可的适用范围。
- **`ely-canvas` 链接**：是否需要为 `ely-settings` 的传递依赖单独保留许可。
- **根项目自身许可证**：本仓库当前**没有**根 `LICENSE` 文件；这不影响第三方 notices，但发布前应确认项目自身许可策略。

出现上述任一选择争议时，按 [工作流](agent-workflow.md) 登记待裁决并交负责人/主 Agent，不自行决定。
