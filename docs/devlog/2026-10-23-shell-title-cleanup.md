# 2026-10-23：简化外壳标题区域

按负责人要求，移除窗口顶栏下方的分隔线，以及 island 顶部重复显示页面名称的占位标题条和对应高度。页面内的大标题保留；后续应用级多标签需求不变，见 [产品约定](../product.md#外壳与导航)。

同时移除只供占位标题条使用的 Route::title，更新现有布局测试以匹配新的页面结构。未修改负责人正在调整的 geometry.rs 圆角和间距。

首次调整只移除了 shell-body 的上边框，遗漏 chrome::titlebar 自身的下边框，负责人截图确认分隔线仍在。随后移除 src/ui/chrome.rs 中顶栏容器的 border_b_1 和对应 border_color；搜索框自身的边框保留。

随后按负责人反馈调整最大化/还原图标：Ely 当前的 WindowControls 使用通用 Square/Copy 且未提供单个图标替换入口，因此在应用层组合窗口控件，继续使用 Ely 的尺寸/颜色和 GPUI 的 Min/Max/Close 命中区域及原有操作。最大化为 10 逻辑像素直角单方框，还原为两只错位 2 逻辑像素的 8 像素方框，线宽 1 逻辑像素；只绘制后层露出的边，不用填充遮盖悬停背景，也不依赖额外字体或修改 Ely 依赖。控件随 window.is_maximized() 选图标，保留既有 debug selectors，补充按钮角色与名称。多 DPI、原生最大化/还原和 Snap 交互仍需真实窗口确认。

验证：`cargo fmt --all -- --check`、`cargo test --locked`（28 项通过）、`cargo clippy --locked --all-targets -- -D warnings` 和 `git diff --check` 均通过。自动化布局检查不替代真实窗口的视觉验收；重新运行 `cargo run` 可查看调整后的界面。
