fn main() {
    let _ = windows_bindgen::bindgen([
        "--in",
        "default",
        "target/windows-appsdk/microsoft.windowsappsdk.interactiveexperiences.1.8.260708001/metadata/10.0.18362.0",
        "--out",
        "src/platform/wasdk_bindings.rs",
        "--filter",
        "Microsoft.UI.Windowing.AppWindow",
        "Microsoft.UI.Windowing.AppWindowTitleBar",
        "Microsoft.UI.Input.InputNonClientPointerSource",
        "Microsoft.UI.Dispatching.DispatcherQueueController",
        "Microsoft.UI.Dispatching.DispatcherQueue",
        "Microsoft.UI.WindowId",
        "Microsoft.UI.Windowing.TitleBarHeightOption",
        "Microsoft.UI.Windowing.TitleBarTheme",
        "Microsoft.UI.Windowing.IconShowOptions",
        "Microsoft.UI.Input.NonClientRegionKind",
        "--reference",
        "windows,skip-root,Windows",
    ]);
}
