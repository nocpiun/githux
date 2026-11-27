# CI Debug artifacts 随附第三方 notices

日期：2026-11-27。任务 `debug-artifact-notices`，分支 `fix/debug-artifact-notices`，基线 `322b3cb974c6d30746aded90c48858e46860e7a6`。交付形式：PR；创建后补链接。

## 目标与状态

把现有第三方 notice 索引和仓库已核验的必要许可文本真正放入四平台 CI Debug artifact，并修正两处已经落后于当前字体资产的约定。不是完整法律审查，不新增依赖、安装器、发布、平台支持声明或许可选择。

当前状态：实现与本地验证完成，待提交、CI/Codex 与真实 artifact 下载核验。

## 改动与决定

- 基线 `.github/workflows/ci.yml` 只复制 executable 与两份根目录字体 OFL。最小临时目录复现显示 `THIRD_PARTY_NOTICES.md`、资产清单及 14 份其余 canonical notice 路径全部缺失。
- 新增 `.github/scripts/stage-third-party-notices.mjs`：把 `THIRD_PARTY_NOTICES.md`、`docs/third-party-assets.md`、两份字体 OFL、IBM Plex、Ely、Lucide、GPUI、pulldown-cmark、Linguist、SQLite、resvg/usvg 与 tiny-skia 的现有文本按仓库相对路径复制；保留两个旧根目录字体文件名作为相同内容的副本。verify 逐字节比较 source/artifact，并在 Windows 额外要求 manifest 和两个固定 SDK package license 非空。
- 新增 6 个离线测试，覆盖 canonical/旧路径、缺失与篡改、平台 runtime 条件、CLI 参数和符号链接。CI 的 `changes` job 运行这些测试；build 在上传前 verify。非 Windows 在 verify 后才 tar，保持 artifact 顶层目录、binary 名与执行命令；Windows 仍先运行现有 runtime preparation。
- 不改动四平台 Debug target、artifact 名称、缓存或发布路径。未下载 registry 许可文本、未替任何双许可项做决定，未把 748 个 lock package 的 SPDX 清单冒充完整随附文本；未覆盖项继续保留在[第三方资产清单](../third-party-assets.md)。
- 负责人在 worktree 复核指出 `THIRD_PARTY_NOTICES.md` 与 `docs/third-party-assets.md` 已成为 staging 输入，却仍会单独命中文档快路径。分类器现在仅对这两个精确路径设置 `app: true/scripts: false`，使后续单文件修改也产出包含新版声明的包；新增单文件回归先在旧分类器失败，再随修复通过，普通文档仍保持跳过。
- 按负责人补充，仅同步 [AGENTS.md](../../AGENTS.md) 与 [产品约定](../product.md)：Githux UI 的拉丁、数字、标点与中文均直接衍生自 Noto Sans SC 的真实 400/500/600/700 字重，不是 Noto Sans 加 SC 回退；代码仍为 Google Sans Code。两处链接到[当前字体清单](../../assets/fonts/README.md)，不改字体实现或其他产品范围。

## 验证

未提交工作区 / 后续提交沿用以下证据，最终以 PR 最新 head 的 CI 为准：

- `node --test .github/scripts/stage-third-party-notices.test.mjs .github/scripts/ci-scope.test.mjs .github/scripts/pr-policy.test.mjs`：21/21 通过（含后续两个单文件分类回归；前一轮为 20/20）。
- `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/tests/windows-runtime.test.ps1 -SkipCargo`：11/11 通过；本地固定包 fixture 验证两个 package license 与 manifest 的准备，未联网下载真实 SDK。完整 Cargo runner fixture 由既有 Windows CI 执行。
- 本地 Linux artifact 格式探针：stage + verify + tar 后逐项检查 16 个 canonical notice 路径、两个旧字体路径及实际解出的字节，全部与仓库一致；`THIRD_PARTY_NOTICES.md`、资产清单和两处字体文档的本地链接均存在。使用文本 executable fixture，不是 GUI 或原生启动验证。Git Bash 对 fixture 的 `chmod +x` 未反映为 tar executable mode，不能作为 Linux/macOS mode 证据；CI 使用原生 runner 上未改变的 `chmod +x`，最终下载再核。
- `python -c "import yaml; ..."`：CI YAML 解析通过。Ruby 不存在，先前 Ruby 解析命令未运行，未安装任何工具弥补。
- `node scripts/verify.mjs -- cargo fmt --all -- --check`：run `94227070-b91d-400a-99b5-35784c99cf9e` 通过；build slot wait 0 ms，命令运行 32.264 s。
- `git diff --check`：交付前检查通过。
- `.github/` 经 `classifyChanges` 为 `{ app: true, scripts: true }`，现有 CI 路由会执行 code-check 与四平台 build；不为本次 staging/文档改动额外启动本地完整编译。

## 后续

- 等最新 head CI 与 Codex，并从该次 run 下载 Windows、Linux、两个 macOS artifact：检查 artifact 身份、binary/launchability 的现有构建证据、上述 notice 路径与实际字节；Windows 检查 SDK 文件，非 Windows 检查 tar 顶层和 executable mode。只读 GitHub artifact，不发布 release。
- 其余直接/传递 crate 文本、双许可选择、图标实际链接集合等仍未解决；本 PR 不声明负责人 R6 整体验收完成。
