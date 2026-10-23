//! Windows App SDK integration on GPUI's existing HWND (UI thread only).
#[path = "caption_color.rs"]
mod color;

use super::CaptionArea;
use crate::platform::wasdk_bindings::Microsoft::UI::{
    Dispatching::{DispatcherQueue, DispatcherQueueController},
    Input::{InputNonClientPointerSource, NonClientRegionKind},
    WindowId,
    Windowing::{AppWindow, AppWindowTitleBar, TitleBarHeightOption, TitleBarTheme},
};
use gpui::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
};
use windows::{
    Graphics::RectInt32,
    Win32::{
        Foundation::{HANDLE, HMODULE, HWND, RECT},
        System::{
            ApplicationInstallationAndServicing::{
                ACTCTXW, ActivateActCtx, CreateActCtxW, DeactivateActCtx, ReleaseActCtx,
            },
            LibraryLoader::{
                GetProcAddress, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
                LoadLibraryExW,
            },
        },
        UI::{
            HiDpi::GetDpiForWindow,
            WindowsAndMessaging::{
                GWL_STYLE, GetClientRect, GetWindowLongPtrW, SetWindowLongPtrW, WS_CAPTION,
            },
        },
    },
    core::{Error, HRESULT, HSTRING, Result, s},
};

#[derive(Clone)]
struct State {
    _window: AppWindow,
    bar: AppWindowTitleBar,
    input: InputNonClientPointerSource,
    metrics: Rc<Cell<CaptionMetrics>>,
}
/// Logical-pixel metrics survive transient COM failures, including a DPI transition.
#[derive(Clone, Copy, Debug, PartialEq)]
struct CaptionMetrics {
    height: f32,
    width: f32,
}

impl CaptionMetrics {
    fn read(height: Result<i32>, width: Result<i32>, dpi: u32) -> Result<Self> {
        let (height, width) = (height?, width?);
        if height <= 0 || width <= 0 || dpi == 0 {
            return Err(Error::new(
                HRESULT(0x80004005u32 as i32),
                "Invalid caption metrics",
            ));
        }
        let scale = dpi as f32 / 96.0;
        Ok(Self {
            height: height as f32 / scale,
            width: width as f32 / scale,
        })
    }

    fn refresh(self, height: Result<i32>, width: Result<i32>, dpi: u32) -> Self {
        Self::read(height, width, dpi).unwrap_or(self)
    }
}
thread_local! {
    static WINDOWS: RefCell<HashMap<usize, State>> = RefCell::new(HashMap::new());
    static RUNTIME: RefCell<Option<Runtime>> = const { RefCell::new(None) };
    static QUEUE: RefCell<Option<DispatcherQueueController>> = const { RefCell::new(None) };
    static LAST_REGION: RefCell<HashMap<usize, (i32, i32, i32, u32)>> = RefCell::new(HashMap::new());
}

fn hwnd(window: &Window) -> Option<HWND> {
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(HWND(handle.hwnd.get() as *mut _)),
        _ => None,
    }
}

// SDK libraries remain loaded while its cached activation factories can be used.
unsafe fn load(name: &str) -> Result<HMODULE> {
    let path = std::env::current_exe()
        .map_err(|_| Error::from_hresult(HRESULT(0x80004005u32 as i32)))?
        .with_file_name(name);
    unsafe {
        LoadLibraryExW(
            &HSTRING::from(path.as_os_str()),
            None,
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
        )
    }
}

struct Runtime {
    context: HANDLE,
    cookie: usize,
    interop: HMODULE,
}

impl Drop for Runtime {
    fn drop(&mut self) {
        // SAFETY: created and dropped on the UI thread, after queue shutdown and window release.
        unsafe {
            let _ = DeactivateActCtx(0, self.cookie);
            ReleaseActCtx(self.context);
        }
    }
}

fn runtime() -> Result<HMODULE> {
    if let Some(module) = RUNTIME.with_borrow(|runtime| runtime.as_ref().map(|r| r.interop)) {
        return Ok(module);
    }
    let manifest = std::env::current_exe()
        .map_err(|error| Error::new(HRESULT(0x80004005u32 as i32), error.to_string()))?
        .with_file_name("githux-wasdk.manifest");
    let source = HSTRING::from(manifest.as_os_str());
    // SAFETY: the manifest path is owned for the call; activation is local to this UI thread.
    unsafe {
        let context = CreateActCtxW(&ACTCTXW {
            cbSize: std::mem::size_of::<ACTCTXW>() as u32,
            lpSource: windows::core::PCWSTR(source.as_ptr()),
            ..Default::default()
        })?;
        let mut cookie = 0;
        if let Err(error) = ActivateActCtx(Some(context), &mut cookie) {
            ReleaseActCtx(context);
            return Err(error);
        }
        // Own the activation immediately so later initialization failures unwind correctly.
        let mut runtime = Runtime {
            context,
            cookie,
            interop: HMODULE::default(),
        };
        let _ = load("Microsoft.WindowsAppRuntime.dll")?;
        runtime.interop = load("Microsoft.Internal.FrameworkUdk.dll")?;
        let module = runtime.interop;
        RUNTIME.with_borrow_mut(|slot| *slot = Some(runtime));
        Ok(module)
    }
}

pub fn install(window: &Window, background: gpui::Hsla, foreground: gpui::Hsla) -> Result<()> {
    let Some(handle) = hwnd(window) else {
        return Ok(());
    };
    if WINDOWS.with_borrow(|windows| windows.contains_key(&(handle.0 as usize))) {
        return Ok(());
    }
    let interop = runtime()?;
    // SAFETY: Microsoft.UI.Interop.h defines this export and ABI. The library stays loaded for
    // the lifetime of the process because WinRT activation factories may cache DLL pointers.
    let id = unsafe {
        let proc = GetProcAddress(interop, s!("Windowing_GetWindowIdFromWindow"))
            .ok_or_else(Error::from_thread)?;
        let get_id: unsafe extern "system" fn(HWND, *mut WindowId) -> HRESULT =
            std::mem::transmute(proc);
        let mut id = WindowId::default();
        get_id(handle, &mut id).ok()?;
        id
    };
    if DispatcherQueue::GetForCurrentThread().is_err() {
        let controller = DispatcherQueueController::CreateOnCurrentThread()?;
        QUEUE.with_borrow_mut(|queue| *queue = Some(controller));
    }
    if !AppWindowTitleBar::IsCustomizationSupported()? {
        return Err(Error::from_hresult(HRESULT(0x80004001u32 as i32)));
    }
    let app = AppWindow::GetFromWindowId(id)?;
    let bar = app.TitleBar()?;
    let input = InputNonClientPointerSource::GetForWindowId(id)?;
    // SAFETY: GPUI owns this HWND; no other thread mutates its styles.
    let original = unsafe { GetWindowLongPtrW(handle, GWL_STYLE) };
    unsafe {
        SetWindowLongPtrW(handle, GWL_STYLE, original | WS_CAPTION.0 as isize);
    }
    let configured = (|| {
        bar.SetExtendsContentIntoTitleBar(true)?;
        bar.SetPreferredHeightOption(TitleBarHeightOption::Tall)?;
        bar.SetPreferredTheme(TitleBarTheme::Dark)?;
        // GPUI leaves the system-button surface transparent. Supply the same opaque Ely
        // palette to AppWindow for both active and inactive windows; keep native hover/press.
        let background = color::boxed(background);
        let foreground = color::boxed(foreground);
        bar.SetBackgroundColor(&background)?;
        bar.SetInactiveBackgroundColor(&background)?;
        bar.SetButtonBackgroundColor(&background)?;
        bar.SetButtonInactiveBackgroundColor(&background)?;
        bar.SetForegroundColor(&foreground)?;
        bar.SetButtonForegroundColor(&foreground)?;
        CaptionMetrics::read(bar.Height(), bar.RightInset(), unsafe {
            GetDpiForWindow(handle)
        })
    })();
    let metrics = match configured {
        Ok(metrics) => metrics,
        Err(error) => {
            let _ = bar.ResetToDefault();
            unsafe {
                SetWindowLongPtrW(handle, GWL_STYLE, original);
            }
            return Err(error);
        }
    };
    window.set_background_appearance(gpui::WindowBackgroundAppearance::Transparent);
    WINDOWS.with_borrow_mut(|windows| {
        windows.insert(
            handle.0 as usize,
            State {
                _window: app,
                bar,
                input,
                metrics: Rc::new(Cell::new(metrics)),
            },
        );
    });
    Ok(())
}

/// Called after the last GPUI window closes, while the UI thread's COM apartment is still live.
pub fn shutdown() {
    let states = WINDOWS.with_borrow_mut(std::mem::take);
    drop(states);
    LAST_REGION.with_borrow_mut(HashMap::clear);
    let queue = QUEUE.with_borrow_mut(Option::take);
    if let Some(queue) = queue
        && let Err(error) = queue.ShutdownQueue()
    {
        eprintln!("AppWindow queue shutdown: {error}");
    }
    let runtime = RUNTIME.with_borrow_mut(Option::take);
    drop(runtime);
}
pub fn area(window: &Window) -> Option<CaptionArea> {
    let handle = hwnd(window)?;
    let state = WINDOWS.with_borrow(|windows| windows.get(&(handle.0 as usize)).cloned())?;
    let dpi = unsafe { GetDpiForWindow(handle) };
    let metrics = state
        .metrics
        .get()
        .refresh(state.bar.Height(), state.bar.RightInset(), dpi);
    state.metrics.set(metrics);
    // A valid HWND normally reports DPI >= 96; keep the cached logical layout if it does not.
    let scale = dpi.max(96) as f32 / 96.0;
    let height = (metrics.height * scale).round() as i32;
    let width = (metrics.width * scale).round() as i32;
    let mut client = RECT::default();
    if dpi > 0 && unsafe { GetClientRect(handle, &mut client) }.is_ok() {
        let rect = (client.right, height, width, dpi);
        let changed = LAST_REGION.with_borrow(|last| last.get(&(handle.0 as usize)) != Some(&rect));
        if changed {
            let search_width = ((crate::ui::geometry::SEARCH_WIDTH + 8.0) * scale).round() as i32;
            let brand_width = (100.0 * scale).round() as i32;
            let search_left = (client.right - width - search_width).max(brand_width);
            let drag = RectInt32 {
                X: brand_width,
                Y: 0,
                Width: search_left - brand_width,
                Height: height,
            };
            let search = RectInt32 {
                X: search_left,
                Y: 0,
                Width: search_width,
                Height: height,
            };
            if let Err(error) = state.bar.SetDragRectangles(&[drag]).and_then(|_| {
                state
                    .input
                    .SetRegionRects(NonClientRegionKind::Passthrough, &[search])
            }) {
                eprintln!("AppWindow regions: {error}");
            } else {
                LAST_REGION.with_borrow_mut(|last| {
                    last.insert(handle.0 as usize, rect);
                });
            }
        }
    }
    Some(CaptionArea {
        width: metrics.width,
        top: 0.0,
        bottom: metrics.height,
        right: 0.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transient_metric_failures_keep_the_last_native_area() {
        let previous = CaptionMetrics::read(Ok(72), Ok(216), 144).unwrap();
        let failure = || Error::from_hresult(HRESULT(0x80004005u32 as i32));
        assert_eq!(previous.refresh(Err(failure()), Ok(288), 192), previous);
        assert_eq!(previous.refresh(Ok(96), Err(failure()), 192), previous);
        assert_eq!(previous.refresh(Ok(0), Ok(288), 192), previous);
        assert_eq!(previous.refresh(Ok(96), Ok(-1), 192), previous);
        assert_eq!(previous.refresh(Ok(96), Ok(288), 0), previous);
        assert!(CaptionMetrics::read(Err(failure()), Ok(216), 144).is_err());
    }

    #[test]
    fn cached_metrics_are_logical_and_update_after_recovery() {
        let previous = CaptionMetrics::read(Ok(72), Ok(216), 144).unwrap();
        assert_eq!(
            previous,
            CaptionMetrics {
                height: 48.0,
                width: 144.0
            }
        );
        assert_eq!(previous.refresh(Ok(96), Ok(288), 192), previous);
        assert_eq!(
            previous.refresh(Ok(100), Ok(300), 192),
            CaptionMetrics {
                height: 50.0,
                width: 150.0
            }
        );
    }
}
