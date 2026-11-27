# Windows 阅读路径基线测量（R6）

更新：2026-11-27。本文记录当前集成桌面客户端在 Windows 下的首份可复现阅读路径测量结果，对应 [开发路线图 R6](roadmap.md#r6windows-首版综合验收) 对启动、交互/滚动、内存、图片/网络与磁盘占用的基线要求。本文只记录实测方法、样本数据、变异性与已知边界，**不代表真实账户人工验收已通过，也不构成硬性性能预算或 CI 门禁**。

## 测试环境与基线范围

- **基线提交**：`322b3cb974c6d30746aded90c48858e46860e7a6`（含 PR #115 PR Conversation 离线缓存与 PR #116 正式 Shell 挂载 `SettingsView`），在此之上仅新增隔离测量探针。
- **构建配置**：`debug` profile（`unoptimized + debuginfo`，其中 `fast_image_resize` 与 `crates/image-resize` 按 `Cargo.toml` 固定配置为 `opt-level = 3`），Rust `1.98.1`，`--locked`。未与未来 `release` 构建混比。
- **Windows 环境**：Microsoft Windows 11 Pro（`10.0.26300`, 64-bit），16 核 / 32 线程 x86_64 CPU，31.8 GiB RAM。
- **隔离与合成数据**：全部运行使用临时目录（`tempfile::tempdir()`）、内存凭据存储（`InMemoryCredentialStore`）与确定性本地 `Transport`；不访问真实 GitHub，不写入真实账户目录或用户配置，不窃取桌面焦点。
- **合成 Fixture 规模**：
  - 账户与首页：12 个仓库；Home `involves:` 列表 25 条；Feed 25 条事件。
  - 仓库 `alice/demo`：60 条根目录树；12,498 B Markdown `README.md`（12 个章节、3 个表格、12 个代码块）+ 4 张确定性 1200×800 RGBA PNG（共 199,152 B 编码字节）；仓库 Issues 50 条（2 页×25）；Issue #42（约 3.1 KB 正文 + 25 条评论）；仓库 PRs 40 条（2 页×20）；PR #5（约 3.1 KB 正文 + 20 条评论 + 15 个改动文件及 unified diff）。
  - 全局入口：Notifications 20 条，全局 Issues 25 条，全局 PRs 25 条。

## 可复现命令

1. **正式装配链路（无头 `gpui_test_platform`，`native_window=false`，N=5 轮冷/热完整阅读流）**：

```sh
node scripts/verify.mjs -- cargo test --locked --bin githux app::pages::tests::reading_baseline_tests -- --ignored --nocapture --test-threads=1
```

2. **Windows 原生进程启动与内存探针（`gpui_platform::application()`，`show: false, focus: false`，N=5 次独立进程）**：

```sh
node scripts/verify.mjs -- cargo build --locked --example reading_baseline_native
./target/debug/examples/reading_baseline_native.exe
```

注：Cargo 编译耗时与构建槽等待时间均与应用程序启动时间分开记录，不将编译或进程拉起等同于首屏可用内容。

## 实测结果（N=5，Debug 构建）

### 1. 正式装配链路：冷缓存 vs 磁盘暖缓存重开（`gpui_test_platform_headless`）

冷启动阶段使用 `0 ms` 本地 mock 延迟；暖重开阶段在真实关闭并释放冷阶段 `CacheStore` 锁（验证 `CacheStore::open` 非共享克隆）后重新打开同一临时目录，并对非 `/user` 请求注入 `150 ms` 人工网络延迟，以区分**磁盘缓存首屏可见内容**与**后台重验证完成后的首个网络空闲**。由于冷（0 ms 延迟）与暖（150 ms 延迟）属于两个不同延迟场景、并非同延迟对照实验，下表分别报告各自实测耗时，不直接计算缓存加速倍数。

| 指标（ms） | Min | Median | Max | 说明与请求数 |
| --- | ---: | ---: | ---: | --- |
| 冷启动 Home 首个可用内容（`home-item-row-0`） | 158.3 | 175.4 | 207.2 | 计时起点在 `add_window_view` 之前 |
| 冷启动 Home 首个网络空闲（`home_is_ready`） | 226.1 | 251.1 | 295.8 | 5 次 mock HTTP 请求 |
| 暖重开 Home 缓存首屏（150ms 延迟下） | 168.6 | 187.4 | 219.8 | 远早于后台重验证完成 |
| 暖重开 Home 后台重验证空闲 | 522.6 | 550.1 | 573.3 | 4 次 mock HTTP 请求 |
| 冷打开 Repository（`markdown-heading-0`） | 238.9 | 281.7 | 357.6 | 12.5 KB README + 4 PNG + 60 树项 |
| 冷打开 Repository 首个网络空闲 | 264.0 | 310.5 | 395.4 | 17 次 mock HTTP 请求 |
| 暖重开 Repository 缓存首屏（150ms 延迟下） | 105.7 | 113.8 | 151.3 | 磁盘缓存首屏（早于 150ms 重验证） |
| 暖重开 Repository 后台重验证空闲 | 202.3 | 284.2 | 309.9 | **5 次 mock HTTP 请求**（减少 12 次） |

### 2. 长文档滚动、分页、详情与多标签切换（`gpui_test_platform_headless`）

| 场景 | Min (ms) | Median (ms) | Max (ms) | 观测条件 |
| --- | ---: | ---: | ---: | --- |
| 长 README 滚动（20 次向下 + 20 次向上） | 891.3 | 1198.6 | 1509.4 | 约 30.0 ms/步（Debug 无优化 UI） |
| 仓库 Issues 第 2 页加载并滚至 `#50` | 151.5 | 198.1 | 253.8 | 验证 `repository-issue-title-50` 可见 |
| Issue #42 详情加载（3.1 KB 正文 + 评论） | 187.4 | 228.6 | 304.6 | 验证 `issue-detail-body` 可见 |
| PR #5 Files changed 懒加载（15 文件 diff） | 72.9 | 104.0 | 106.7 | 验证 `pr-file-0` 可见 |
| 5 次暖标签切换（Home↔Repo↔Issue↔PR↔Home） | 230.4 | 255.7 | 273.0 | 约 51.1 ms/次，**0 次**新增 HTTP 请求 |

### 3. 进程内存生命周期（Run 0，`K32GetProcessMemoryInfo`）

| 阶段 | Working Set (MiB) | Peak WS (MiB) | Private Commit (MiB) |
| --- | ---: | ---: | ---: |
| 挂载 `Shell` 前 | 22.09 | 22.09 | 7.14 |
| Home 就绪（1 个标签） | 42.43 | 43.79 | 14.86 |
| 打开全部 9 个阅读标签后（Home + 8 页） | 72.93 | 75.25 | 36.59 |
| 关闭 8 个标签仅留 Home 后 | 63.31 | 75.25 | 26.66 |

关闭 8 个标签后，Private Commit 立即回落 9.93 MiB（占 9 标签阶段增量的约 45.7%）；相对初始 Home 仍高出 11.80 MiB，反映分配器/字体/共享缓存保留，且无头平台不含原生 GPU 显存。

### 4. 隔离磁盘缓存占用（冷+暖完整阅读后）

- `cache/` 目录：2 个文件（`cache.sqlite3` 与 `cache.lock`），总计 **421,888 B**（412.0 KiB）。
- `accounts.sqlite3*`：总计 **32,768 B**（32.0 KiB）。

### 5. Windows 原生进程启动与内存基线（`reading_baseline_native.exe`，N=5）

使用真实 `gpui_platform::application()`、Win32/Direct2D/DirectWrite、字体注册与隐藏不聚焦窗口（`show: false, focus: false`），独立进程执行 5 次：

| 阶段 | Min (ms) | Median (ms) | Max (ms) |
| --- | ---: | ---: | ---: |
| `CacheStore::open` + `AccountService` + `login` | 17.2 | 19.0 | 22.4 |
| `gpui_platform::application()` 就绪 | 260.4 | 266.6 | 320.0 |
| `ely_core::init` + 字体注册 + 主题 + 快捷键 | 283.8 | 286.8 | 340.3 |
| 原生窗口创建与 `Shell::new` 挂载 | 500.0 | 531.5 | 576.9 |

- **原生进程内存**：入口 Working Set `10.54 MiB` / Private Commit `2.83 MiB`；窗口打开并等待 250ms 后 Working Set `162.8–163.1 MiB`（`170,688,512–170,971,136 B`）、Private Commit `114.1–115.1 MiB`（`119,676,928–120,713,216 B`）。
- 已有大图原生专项证据另见 [README 原图性能日志](devlog/2026-11-12-reader-image-performance.md)（144 DPI 原生窗口 9 张 2559×1520 原图：Working Set 188.5 MB / Peak 282.6 MB / Private Commit 230.7 MB）。

## 未覆盖项与观测到的异常

- **明确未覆盖**：独立 GPU 显存（VRAM）、真实公网 GitHub 延迟与限流、可见原生窗口下的实时滚动帧率/观感、中文 IME 组合态与多 DPI/跨屏切换，以及 `release` profile 性能。
- **异常 1（测试底座析构边界）**：在手动创建的 `gpui::TestAppContext` 中调用 `add_window_view` 后，若未显式调用 `cx.quit()` 就直接 `drop(cx)`，`VisualTestContext::into_mut` 注册的 `on_quit` 不会执行，导致 `AccountRuntime` 引用计数保持为 1 且 `CacheStore::open` 返回 `CacheError::InUse`。显式调用 `cx.quit()` 后 `weak_runtime.strong_count()` 稳定归零，真实关闭/重开通过。
- **异常 2（合成 Profile `/users/alice` 响应提示）**：测量期间观察到 `Profile` 打印 `apply_basic None`；因本任务不修改业务页面或扩大范围，仅如实记录该合成 fixture 现象。
