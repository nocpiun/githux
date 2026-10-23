# 2026-10-23：Windows 原生标题按钮验证

状态：实现与本机验收完成，负责人已要求提交 PR 并进入审查循环。以下各轮是诊断过程；CI、审查与合并状态以对应 PR 为准。

负责人要求保留 Githux / 搜索框所在的自定义顶栏，同时将三个按钮交给 Windows 真正绘制和处理。当前 GPUI Windows 后端没有该混合模式配置，候选方案在应用的 src/platform/caption.rs 中取得本窗口 HWND，通过 SetWindowSubclass 接入 DWM，恢复 WS_CAPTION 并扩展顶栏区域。普通消息仍回到 GPUI，原生按钮的非客户区消息交给 DWM / DefWindowProc。

界面按 DWM 返回的按钮范围预留右侧宽度，不另画一套按钮；首次尚无范围时保守预留。DPI/样式改变时更新 frame，销毁时移除 subclass 和属性。初始化失败恢复自绘控件；测试平台没有 HWND，继续验证自绘回退路径。Windows 依赖仅在 cfg(windows) 下启用，没有修改 GPUI/Ely 的缓存源码。

此实现首先需要验证：DirectComposition 是否遮住系统按钮、顶栏是否出现系统重复文字/边框、三个按钮及拖动/双击/贴靠是否正常、最大化还原和多 DPI 后预留区域是否正确。电脑上的 computer-use 工具初始化成功，但 list_windows 连续失败（native pipe 不存在），重置后重试仍失败，因此真实窗口需要负责人确认。模拟测试不代替上述结果。

本任务从主工作区 HEAD 创建独立 worktree，并复制当时已暂存的外壳视觉调整作为候选基线；主工作区及其 index 未被本任务修改。

参考：[DWM 自定义窗口框架](https://learn.microsoft.com/en-us/windows/win32/dwm/customframe)、[DwmDefWindowProc](https://learn.microsoft.com/en-us/windows/win32/api/dwmapi/nf-dwmapi-dwmdefwindowproc)。

## 预览反馈与第二轮

第一轮成功建立真实 Githux 窗口，但负责人明确反馈“按钮不显示或被遮挡”，不算原生按钮验证通过。第二轮将窗口合成背景改为 Transparent，主背景由 shell-body 绘制，顶栏只在原生按钮预留区左侧绘制不透明底色，避免整窗/整条顶栏覆盖 DWM 区域。初始化失败仍恢复 Opaque。

第二轮 cargo build --locked --offline、cargo test --locked --offline（28 项）、cargo clippy --locked --all-targets --offline -- -D warnings、git diff --check 通过。第二版真实窗口已打开，等待负责人确认按钮是否出现及背景/交互情况。28 项测试仍只证明 GPUI 测试平台的布局和自绘回退，不证明 DWM 绘制。

## 第三轮（最大化与背景）

第二轮负责人反馈：按钮已出现，但背景与交互异常；窗口化可交互，最大化后不可正常交互，因此仍未验收。第三轮增加 Windows 11 的 DWM caption/text 颜色设置，使用 Ely 启动时的背景/前景色（旧系统不支持颜色属性时保留系统配色），并将 DWM 的 window-relative 矩形转换为 client-relative 矩形。透明范围仅覆盖实际按钮区域，顶部/底部/右侧其余部分绘制应用背景。

WM_ACTIVATE/WM_SIZE 也更新扩展框架。WM_NCHITTEST 优先 DWM，未接管时再检查 DefWindowProc 的真实按钮结果，其余命中仍交给 GPUI。新增 GITHUX_TRACE_CAPTION 开关记录本窗口状态和按钮命中结果，用于真实预览诊断，不默认开启。初始化 subclass 失败不再预先切换窗口透明背景。

第三轮本地 build、31 项 test（含窗口化/最大化 150% DPI 坐标换算及无效矩形）、clippy -D warnings、diff --check 均通过。已打开第三版预览，等待真实背景、最大化后还原/最小化的反馈；这些测试仍不证明原生交互已修复。配色当前在窗口创建时配置，未来支持运行时主题切换时须同步更新。

## 第四轮（原生屏幕坐标）

负责人确认第三轮背景正常，但最大化交互仍失败，且按钮与品牌/搜索框不对齐。应用内部只读诊断记录：144 DPI 最大化窗口矩形 (-11,-11,3851,2099)，client origin (0,0)；DWM bounds (3630,0,3850,44)，TITLEBARINFOEX 返回按钮屏幕矩形 min=(3627,0,3698,43)、max=(3698,0,3768,43)、close=(3768,0,3840,43)。在 max 中心 (3733,21)，DwmDefWindowProc 未接管、DefWindowProc 返回 HTCLIENT。因此第三轮的默认过程回退不足，且按隐藏边框偏移换算 DWM bounds 导致透明区域高度误缩到22逻辑像素。

第四轮将布局透明孔与补充命中统一使用 WM_GETTITLEBARINFOEX 的屏幕矩形。优先保留 DWM/default 原生按钮结果；二者未识别时，读取三个按钮矩形并检查 unavailable/invisible/offscreen 状态，返回对应 HTMINBUTTON/HTMAXBUTTON/HTCLOSE，操作仍交 Windows 默认非客户区处理。品牌/搜索框按原生按钮行高对齐，去掉原生模式下原有的8像素向下偏移；不通过拉伸或自绘替代原生按钮。

回归测试包含真实故障坐标、隐藏边框不重复扣减、负屏幕原点、按钮边界和禁用状态。第四轮 build、31 tests、clippy -D warnings、diff --check 通过。以临时 GITHUX_CAPTION_START_MAXIMIZED=1 启动第四版预览；实际点击/悬停和对齐仍待负责人确认。诊断开关和探针需在定稿时清理或限制为调试使用。

参考：[TITLEBARINFOEX](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-titlebarinfoex)。

## 第五轮（恢复 DWM 悬停处理）

第四轮负责人确认背景与交互正常，但最大化状态下没有 hover 背景，按下 active 有反馈。第四轮补充 HT 命中只恢复了默认点击，DWM 仍未接管。进一步参考 [Chromium 56 的 HWNDMessageHandler::OnNCCalcSize](https://chromium.googlesource.com/chromium/src.git/+/56.0.2924.87/ui/views/win/hwnd_message_handler.cc)：其原生框架分支记录过顶部非客户区导致 DwmDefWindowProc 的 hover/click 失效。

第五轮仅在启用原生 caption 且最大化时调整 WM_NCCALCSIZE：仍由 GPUI 计算左右/底边，但将 client top 恢复至窗口 top，不保留顶部非客户区。隐藏的 resize frame 高度改由根布局 padding 补偿，按 DPI 系统度量获取；透明区域换算扣除同一 padding，frame extension 同步增加该高度。窗口化与此前按钮点击路径保持。

本机真实只读探针及鼠标日志已确认：相同最大化按钮中心，DWM 从 false/0 变为 true/HTMAXBUTTON；三个按钮实际命中日志均 dwm=true。说明 DWM 接管恢复，但是否显示正确 hover 及离开清除仍等待负责人视觉确认。第五轮 build、31 tests、clippy -D warnings、diff --check 通过；测试平台仍不能覆盖非客户区绘制。


## 本机验收与收尾

负责人于第五轮确认“一切正常”：最大化时三个按钮的 hover 背景与移开清除正常，窗口化后的 hover 和点击正常；此前第四轮已确认背景与点击正常。临时只读探针、命中日志开关和启动最大化开关已清理，默认恢复窗口化启动。没有修改依赖缓存源码，也没有用自绘 hover 替代原生 DWM 状态。

验收范围是当前机器的真实 Windows 窗口（日志 DPI=144）；跨屏 DPI、其他 Windows 版本/系统主题、高对比度、辅助技术与 Snap flyout 未在本轮逐项验收，不据此宣称通过。原生颜色在启动时配置，未来全局主题切换须同步更新 DWM。主工作区已暂存的原视觉调整保持不动，本任务实现保存在独立工作区。

收尾清理后再次执行 cargo build --locked --offline、cargo test --locked --offline（31 项）、cargo clippy --locked --all-targets --offline -- -D warnings、git diff --check，均通过；重新打开默认窗口化的最终本地预览。


## 验收后的间距调整

负责人要求标题栏上下间距均为 spacing 2（8px / 0.5rem）。品牌与搜索框改为固定 28px 内容行，上下各 8px，总高 44px；移除 island 的额外 top inset，搜索框下缘到 island 顶边为 8px。几何常量集中在 src/ui/geometry.rs。原生 caption 区不添加 padding；DWM 按钮尺寸仍由系统决定，没有通过自绘或全局系统设置拉伸至 island。此调整不改原生命中、最大化修正、hover 和点击路径。实际新间距视觉待确认。

间距调整后 build、31 tests、clippy -D warnings、diff --check 均通过，已启动新的本地预览。


## 本次提交范围

按负责人要求合并此前外壳视觉调整一并提交：island 圆角 6px、右/下边缘间距 7px、侧栏间距 8px，移除顶栏分隔线及 island 重复小标题，原生标题按钮和最终上下 8px 间距。此前主工作区暂存内容与创建任务时的基线核对一致，没有遗漏额外修改。Windows App SDK Tall 标题栏仅讨论可行性，未纳入本次实现；原生按钮保留当前系统高度。
