# 第三方资产与依赖许可清单

字体章节更新：2026-10-30（ui-fonts）；其余章节为 2026-10-29 任务 `dependency-licenses`，基线 `730d2292dcd5bb981928341259c971e42ff497b5`。第 5、8、9 节另含 2026-11-27 任务 `current-dependency-notices` 的补充：chrono 0.4.45、sha1_smol 1.0.1 与 fast_image_resize 5.1.4 的四份原文。

本文记录当前首版**实际分发**的字体、图标、复制的 Markdown 代码、Rust 依赖与 Windows App SDK 运行时的来源、固定版本、哈希、许可证文本与实际分发形态，并区分已验证与未覆盖。它是后续打包的依据，不是法律结论，也不代表整个应用已通过发布验收。

> **CI Debug artifact 已携带下列已入库 notices；完整发布许可核验仍未完成。** 四平台 CI 在 executable 旁保留 `THIRD_PARTY_NOTICES.md`、本清单及第 8 节的实际文本，路径与源码树一致，因此主要 notice 链接可在 artifact 内解析。Windows App SDK 许可证只加入 Windows artifact。其余直接/传递 crate 文本与双许可选择仍未解决；没有安装包或正式发布，也不把构建产物等同于跨平台产品支持。

## 范围与方法

- 只核对本基线真正进入分发的对象：`Cargo.lock` 解析图、`assets/` 中的字体与 manifest、`src/platform/fonts.rs` 与 `ely-core` 的 `include_bytes!`、复制的 `src/ui/reader/markdown/` 源码、Windows App SDK 运行时。不扫描上游全部文件，也不把整库搬进仓库。
- 依赖清单来自 `cargo metadata --locked --offline --format-version 1`（Rust 1.98.1）。其 `license` 字段是 **SPDX 声明**，用于筛选与分类；**SPDX 字符串不等于已逐项核验全量许可合规**。
- 固定版本的许可文本从本机已获取的 cargo registry 源码与 Ely git checkout 按精确版本/修订读取；本机缺失的固定版本从官方精确 tag/revision 查证。
- 原清单不修改依赖或图标；2026-10-30 字体资源替换与衍生字面生成单独记录在第 1 节，未改 Cargo.lock。

### 复现命令

```sh
cargo metadata --locked --offline --format-version 1
```

`--offline` 要求本机缓存已有 lock 中的固定版本；缺少时先让 cargo 获取一次（不改变 `Cargo.lock`），之后即可离线复现。下面“直接依赖”表的版本与 SPDX 即由该命令产出。

## 1. 字体

`src/platform/fonts.rs` 嵌入四个应用私有 UI 字面（Githux UI）与原 Google Sans Code。UI 衍生字面全部来自 Noto Sans SC 2.004-H2 的 30,890 映射（覆盖中文汉字、全套拉丁字母、数字及 CJK 标点）；400/500/600/700 均使用源字体对应权重，主字族改名以避免与上游/系统字体混淆。官方固定来源、每个文件体积/哈希与可复现生成过程统一维护于 [字体清单](../assets/fonts/README.md)，生成脚本为 [build-ui-fonts.py](../scripts/build-ui-fonts.py)。

- 四份 UI 字面共 41,855,896 B（39.92 MiB）。仅包含该 Noto Sans SC 官方来源，没有引入其他语言字体集。
- Noto Sans SC 为 SIL OFL 1.1；随包携带 `assets/fonts/githux-ui/OFL-NotoSansSC.txt`。SC 来源原声明含 Reserved Font Name `Source`，衍生字体使用 `Githux UI` 名称。版权和许可文本保留，未改写。
- Google Sans Code v7.001 原文件 268,264 B，SHA-256 `74642762fae66d5883077794a763e1086394b8cb72f2623e456e06359dc5c64b`；字体与许可均未改。
- CI artifact 打包步骤同时携带 UI 来源 OFL 及 Google Sans Code OFL。许可文本不嵌入 binary，单独分发时仍必须随附。
- Windows GPUI 0.3.10 的显式 fallback 查询只使用系统字体集合。将 Noto Sans SC 作为全量 UI 字体让英文与中文直接命中应用内注册字体，不依赖系统安装；逐映射轮廓/advance 比对和原生 shaping 证据见[字体开发日志](devlog/2026-10-29-ui-fonts.md)。

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
- 2026-10-29 基线没有 vendor/patch；当前 resvg 的精确发布包补丁见下文 SVG 补充。`ely-documents` / `ely-editor` **不是**依赖（Markdown 组件已提取，见第 4 节）。
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

### 4.1 GitHub Linguist 语言颜色表

仓库页面底边语言条不再自行用哈希分配调色板色，而是使用 GitHub 自己的语言颜色。数据来自 GitHub Linguist 的 `lib/linguist/languages.yml`，只提取“语言名 + 显式 `color:`”两列，生成 [`src/ui/repository_overview/language_colors.rs`](../src/ui/repository_overview/language_colors.rs)，运行时不上网、不解析 YAML。

| 项 | 内容 |
| --- | --- |
| 上游仓库 | `https://github.com/github-linguist/linguist` |
| 固定修订 | `d456527fe93355baa28c3c29d542c97282b61bc7`（2026-11-04，`main` 当时最新） |
| 固定文件 | `lib/linguist/languages.yml`，167,711 B，SHA-256 `09c13db185b6f9b8f64b932dcd79815fef4a67624ab2dade9c194388d1dbe29d` |
| 提取规则 | 只取显式声明 `color:` 的 701 个语言；无 `color` 或未知名字使用中性回退色，不自行推算色值 |
| 再生成 | [`scripts/generate-linguist-language-colors.mjs`](../scripts/generate-linguist-language-colors.mjs)（脚本注释内含下载与调用命令） |
| 许可证 | MIT，`Copyright (c) 2017 GitHub, Inc.`，原文已复制到 [`assets/licenses/linguist-mit.txt`](../assets/licenses/linguist-mit.txt)（SHA-256 `7717070c5a8c85440ff2312930bb30dd19002ab5bc235c9800042767f6f5242f`） |
| 分发形态 | 颜色表以静态 `&[(&str, u32)]` 编译进 binary，只含名字与色值，不携带语法或仓库内容 |

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
| `resvg` | 0.46.0 | Apache-2.0 OR MIT | 编译进 binary；原先由 GPUI 传递引入，现在 Reader 直接使用 | [`MIT`](../assets/licenses/resvg-0.46.0-license-mit.txt)、[`Apache-2.0`](../assets/licenses/resvg-0.46.0-license-apache.txt) |
| `simplecss` / `svgtypes` | 0.2.2 / 0.16.1 | Apache-2.0 OR MIT | 已由 usvg 传递引入；Reader timeline 直接使用 | registry `LICENSE-MIT` / `LICENSE-APACHE` |
| `pulldown-cmark` | 0.13.5 | MIT | 编译进 binary | `assets/licenses/pulldown-cmark-mit.txt` |
| `chrono` | 0.4.45 | MIT OR Apache-2.0 | 编译进 binary（Reader 图片 HTTP `Date` 解析） | [`assets/licenses/chrono-0.4.45-license.txt`](../assets/licenses/chrono-0.4.45-license.txt)（同一个 crate 文件含 MIT 与 Apache-2.0 全文） |
| `sha1_smol` | 1.0.1 | BSD-3-Clause | 编译进 binary（Git blob 对象校验） | [`assets/licenses/sha1_smol-1.0.1-license.txt`](../assets/licenses/sha1_smol-1.0.1-license.txt)（同一文件另含 `src/simd.rs` 的 MIT 段落） |
| `fast_image_resize` | 5.1.4 | MIT OR Apache-2.0 | 经 `githux-image-resize` path crate 编译进 binary | [`MIT`](../assets/licenses/fast_image_resize-5.1.4-license-mit.txt)、[`Apache-2.0`](../assets/licenses/fast_image_resize-5.1.4-license-apache.txt) |
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

### SVG 渲染依赖补充（2026-11-11，reader-svg）

`resvg 0.46.0`（直接依赖）、`usvg 0.46.0`（通过 resvg）与 `tiny-skia 0.11.4`（通过 resvg）原本就在 GPUI 的解析图中；品质返工为修正 displacement scale 重复相乘及小数/裁切输入网格不等造成的断言，现将精确 resvg 发布包源码 vendoring 到 `vendor/resvg`，通过根 Cargo patch 让 GPUI 与 Reader 统一使用该修补版本，原包 metadata、源码/示例及 MIT/Apache 文本保留，修改边界见 `vendor/resvg/PATCH.md`，没有升级版本。Reader 启用 resvg 的 `text` 与 `raster-images`，禁用默认 feature，不启用系统字体扫描。`simplecss 0.2.2` / `svgtypes 0.16.1` / `flate2 1.1.10` 复用已锁定版本；新增 flate2 直接边用于有界 WOFF1 解包（MIT OR Apache-2.0，原 registry 许可文件）。

精确 registry 发布包的许可原文已保存在 `assets/licenses/`：

| 文件 | SPDX / 版权 | SHA-256 |
| --- | --- | --- |
| `resvg-0.46.0-license-mit.txt`、`usvg-0.46.0-license-mit.txt` | MIT；Copyright 2017 the Resvg Authors | `f5d934dc281b44e0003ee461ac740b18b6629a454decd872c774d34e4ee0b21d`（两份相同） |
| `resvg-0.46.0-license-apache.txt`、`usvg-0.46.0-license-apache.txt` | Apache-2.0 | `a6cba85bc92e0cff7a450b1d873c0eaa2e9fc96bf472df0247a26bec77bf3ff9`（两份相同） |
| `tiny-skia-0.11.4-license.txt` | BSD-3-Clause；Copyright (c) 2011 Google Inc.; Copyright (c) 2020 Yevhenii Reizner | `6d41e05f1d54fe726997cb0334ec539f1dff50600b1d5d90462ec2b392770ced` |

随发布携带上述文本；resvg/usvg 保留双许可选项，此处不替项目选择其一。测试专用 `src/content/images/fixtures/embedded-font.woff` 为既有 Google Sans Code v7.001 的 OFL 子集，改名为 Githux SVG Fixture（5,312 B，SHA-256 `20de0d77199053950d57640b4ce7a4b2063a0532a5f7f57d7ce7369958c6b7aa`），保留版权/许可 name 记录；只在 cfg(test) 中使用，不进入应用字体集，源码再分发仍随附既有 Google Sans Code OFL，来源说明见相邻 README。远程 README 返回的用户 SVG 仅为本地调查证据，不因获取公开响应而被纳入可分发素材；提交的动画回归使用原创最小几何与示例文案，并注明观测的 CSS 形状来源。

### 图片阅读依赖许可文本补充（2026-11-27，current-dependency-notices）

`chrono 0.4.45`（Reader 图片 HTTP `Date` 解析）与 `sha1_smol 1.0.1`（Git blob 对象校验）是根 crate 的直接依赖；`fast_image_resize 5.1.4` 由我们的 path crate `githux-image-resize`（[`crates/image-resize`](../crates/image-resize/Cargo.toml)，`publish = false`）以 `=5.1.4` 固定引入。**`githux-image-resize` 是 Githux 自己的 crate，不是上游 `fast_image_resize`**；下面的文本只覆盖上游 crate。三者都进入当前分发的 binary，但在此之前既不在 notice 索引也不在 staging 列表中。本轮只补原文与索引、打包路径，不改依赖、`Cargo.lock`、运行时行为或平台范围。

| 文件 | 上游归档（`Cargo.lock` checksum） | 归档内路径 | 文本 SHA-256 |
| --- | --- | --- | --- |
| `assets/licenses/chrono-0.4.45-license.txt` | `chrono-0.4.45.crate`，`1aa79e62e7697b8e29b513a68abacf485adcd1fe8284a4316c5ae868e6633327` | `LICENSE.txt` | `946c9835d8034d24404f8cfec5f4654cee5dad17e944afc3d06d742cf2882831` |
| `assets/licenses/sha1_smol-1.0.1-license.txt` | `sha1_smol-1.0.1.crate`，`bbfa15b3dddfee50a0fff136974b3e1bde555604ba463834a7eb7deb6417705d` | `LICENSE` | `282a21b0710441c364025461a447362daf18f5798a7b72ae2f801a2aa536da51` |
| `assets/licenses/fast_image_resize-5.1.4-license-apache.txt` | `fast_image_resize-5.1.4.crate`，`d372ab3252d8f162d858d675a3d88a8c33ba24a6238837c50c8851911c7e89cd` | `LICENSE-APACHE` | `5caa63b85f1c4683ed495f09991128fd777fda36066e2fcdf0f8533212776ed1` |
| `assets/licenses/fast_image_resize-5.1.4-license-mit.txt` | 同上 | `LICENSE-MIT` | `59c7bec3a007a291de2369c504545cb61b5f4d7ac9893e50d523f2aff4fcc7c6` |

四份文件均按归档内字节完整复制，未改写版权行或删减段落：`chrono/LICENSE.txt` 本身同时含 MIT 与 Apache-2.0；`sha1_smol/LICENSE` 除 BSD-3-Clause 外还带有 `src/simd.rs` 的 MIT 段落及其版权行，也一并保留。fast_image_resize 的 MIT/Apache-2.0 两个选项都保留，**未替项目选择其一**。Windows 检出可能按 `core.autocrlf` 把行尾转成 CRLF；这是行尾转换而非内容改动，下表文本 SHA-256 与离线回归都按 LF 归一后的文本计算，artifact 的 verify 仍逐字节比较源文件与暂存副本。可复现命令（使用本机已获取的精确版本，不需重新下载或安装）：

```sh
cache=~/.cargo/registry/cache/index.crates.io-1949cf8c6b5b557f
sha256sum "$cache/chrono-0.4.45.crate"        # 必须等于 Cargo.lock 的 checksum
tar -xzOf "$cache/chrono-0.4.45.crate" chrono-0.4.45/LICENSE.txt | sha256sum
tar -xzOf "$cache/sha1_smol-1.0.1.crate" sha1_smol-1.0.1/LICENSE | sha256sum
tar -xzOf "$cache/fast_image_resize-5.1.4.crate" fast_image_resize-5.1.4/LICENSE-APACHE | sha256sum
tar -xzOf "$cache/fast_image_resize-5.1.4.crate" fast_image_resize-5.1.4/LICENSE-MIT | sha256sum
```

离线回归 [`stage-third-party-notices.test.mjs`](../.github/scripts/stage-third-party-notices.test.mjs) 固定了上表四个文本 SHA-256（按 LF 归一），并从 `Cargo.lock` 读取同名 crate 的 checksum 比对归档摘要，因此本地文本被替换或与锁文件脱钩会直接失败；它不重建也不下载归档。四份文本同时加入 staging 列表，索引内的 `assets/licenses/` 链接必须指向存在且被 staging 的文件。

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

## 8. CI Debug artifact 随附的文件

四平台 Debug build 的 staging helper 是
`.github/scripts/stage-third-party-notices.mjs`；CI 对下列每项与仓库实际文本逐字节比较，非 Windows 在验证后保持原目录层级生成 `.tar.gz`，Windows 直接上传目录。旧 artifact 根目录的 `OFL-NotoSansSC.txt` 与 `OFL-GoogleSansCode.txt` 继续作为相同内容的兼容副本保留，不作为另一份来源。

1. UI 来源 OFL（`assets/fonts/githux-ui/OFL-NotoSansSC.txt`）及 `assets/fonts/google-sans-code/OFL.txt`。
2. IBM Plex Sans 的 OFL：`assets/licenses/ibm-plex-sans-ofl.txt`（binary 内也有一份 asset，但 artifact 仍显式随附）。
3. Ely MIT：`assets/licenses/ely-mit.txt`（覆盖 `ely-core`/`ely-settings`/`ely-canvas` 与提取的 Markdown 派生代码）。
4. Lucide 图标 ISC + Feather MIT：`assets/licenses/lucide-icons-isc.txt`。
5. GPUI Apache-2.0：`assets/licenses/gpui-pre-apache-2.0.txt`；pulldown-cmark MIT：`assets/licenses/pulldown-cmark-mit.txt`。
6. GitHub Linguist MIT：`assets/licenses/linguist-mit.txt`（语言颜色表为 MIT 数据的派生）。
7. SQLite 公有领域说明：`assets/licenses/sqlite-public-domain.txt`（信息性）。
8. resvg / usvg 的 MIT 与 Apache-2.0 全文，以及 tiny-skia BSD-3-Clause 全文：均保留 `assets/licenses/` 下的现有文件；不替项目选择双许可项。
9. chrono 的 MIT+Apache-2.0 合一原文 `assets/licenses/chrono-0.4.45-license.txt`、sha1_smol BSD-3-Clause（含 `src/simd.rs` MIT 段落）的 `assets/licenses/sha1_smol-1.0.1-license.txt`，以及 fast_image_resize 的 `assets/licenses/fast_image_resize-5.1.4-license-apache.txt` / `-mit.txt`；双许可项不替项目选择。
10. 合并入口：[`THIRD_PARTY_NOTICES.md`](../THIRD_PARTY_NOTICES.md)，以及本清单。
11. 仅 Windows：App SDK 各包 `license.txt` 由 `windows-runtime-lib.ps1` 复制为 `microsoft.windowsappsdk.foundation.LICENSE.txt` 与 `microsoft.windowsappsdk.interactiveexperiences.LICENSE.txt`；验证同时要求 `githux-wasdk.manifest` 非空。

本清单中的源码、脚本及未列入上述路径的设计文档链接仍以源码仓库为阅读入口；随 artifact 保留的是 notices、许可文本与清单，不是完整源码镜像。第 5 节其余直接依赖及第 6 节传递依赖的 registry 文本、双许可选择和目标分发集合仍需另行收敛，**不声称已全部随附**。

## 9. 已验证与未覆盖

**CI artifact 打包契约与本地确定性验证（2026-11-27，`debug-artifact-notices`）：**

- `.github/workflows/ci.yml` 的 `changes` job 运行 staging 离线测试；四平台 build 都在 runtime 准备（仅 Windows）之后、上传之前执行同一 verify。non-Windows 在 verify 成功后才生成 tarball；矩阵 target、Debug 模式、artifact 名称、可执行文件名、原有执行位与 Cargo/rust-cache 均不变。
- 本地 fixture 覆盖 20 个 canonical notice 路径和两个旧字体文件名、缺失/篡改文本、缺失 Windows manifest/单个 SDK license、Linux/macOS 不要求 Windows runtime，以及符号链接不作为普通文件接受。它不替代 CI 对真实构建产物的验证。
- 实际四平台 artifact 的内容与链接以本 PR 最新 head 的 CI 下载核验为准；源码实现与离线测试本身不冒充已下载产物证据。未新增 installer、release、目标平台或许可选择。

**阅读图片依赖原文与归档来源核验（2026-11-27，`current-dependency-notices`）：**

- 三个 crate 归档的 SHA-256 与 `Cargo.lock` 的 `checksum` 完全一致（`chrono 0.4.45`、`sha1_smol 1.0.1`、`fast_image_resize 5.1.4`）；归档取自本机 registry 缓存，未下载或安装新依赖。
- 四份文本从归档直接解出后与仓库副本字节一致，SHA-256 已记入第 5 节补充表；`sha1_smol/LICENSE` 与 `chrono/LICENSE.txt` 的内部多段许可与版权行原样保留。
- staging 列表从 16 增至 20 个 canonical 路径；新增离线回归固定四份文本摘要并从 `Cargo.lock` 比对归档摘要，同时要求两份索引里的 `assets/licenses/` 链接都存在且被 staging。这是打包与文档变更，因此本轮未运行 Cargo。

**依赖与字体清单原始核验：**

- 五份打包 ttf 的 SHA-256 与 [`assets/fonts/README.md`](../assets/fonts/README.md) 一致；三个来源的 OFL 与其字体匹配。Noto 归档的 SHA-256 本轮已按官方 release 重新下载核验为 `0c34df07…`。
- Noto v2.015 与 Google Sans Code v7.001 的官方 release tag 存在（GitHub API 核验）。
- `include_bytes!` 路径与 `assets/fonts/README.md` 的 include 描述一致；仓库内`src/platform/fonts.rs` 五处 `include_bytes!`。
- Ely 修订 `30c66fe1` 的 MIT 文本、`gpui-pre 0.3.10` 的 Apache-2.0 文本、Lucide 的 ISC+Feather MIT、IBM Plex OFL、pulldown-cmark MIT 均已按原文复制到 `assets/licenses/` 并计算 SHA-256。
- `cargo metadata --locked --offline` 成功，得到 748 个 package 的版本与 SPDX 声明。
- Windows App SDK 的固定包哈希与“脚本已复制 license.txt”在 [`scripts/windows-runtime-lib.ps1`](../scripts/windows-runtime-lib.ps1) 与测试中已有覆盖。

**本轮未覆盖：**

- 字体归档已重新下载并核对 SHA-256；仍不核验归档内未打包的其他字面。
- 2026-11-27 的补充只覆盖 `chrono`、`sha1_smol`、`fast_image_resize` 三个 crate 的原文；其余直接依赖的 registry 文本与 748 个 package 中的传递依赖仍未逐项读取或随附，双许可项仍未选择。本次不声称已完成发布许可核验。
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
