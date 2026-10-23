//! Native DWM caption buttons alongside GPUI's custom title-bar content.
//!
//! This adapter only subclasses the application's own HWND on its UI thread. No dependency
//! sources are modified. Unknown/client messages continue through GPUI's original procedure.

use std::mem::size_of;

use gpui::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::{
    Win32::{
        Foundation::{HANDLE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
        Graphics::Dwm::{
            DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DwmDefWindowProc, DwmExtendFrameIntoClientArea,
            DwmSetWindowAttribute,
        },
        Graphics::Gdi::ClientToScreen,
        UI::{
            Controls::MARGINS,
            HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi},
            Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
            WindowsAndMessaging::*,
        },
    },
    core::{Error, Result, w},
};

const SUBCLASS_ID: usize = 0x47495843;
const INSTALLED: windows::core::PCWSTR = w!("Githux.NativeCaption");

fn hwnd(window: &Window) -> Option<HWND> {
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(HWND(handle.hwnd.get() as *mut _)),
        _ => None,
    }
}

/// Install before the first Shell render. On failure, leave the existing custom controls active.
pub fn install(window: &Window, background: gpui::Hsla, foreground: gpui::Hsla) -> Result<()> {
    let Some(hwnd) = hwnd(window) else {
        return Ok(());
    };
    // SAFETY: GPUI owns this live HWND; startup and every callback run on its UI thread.
    unsafe {
        if !GetPropW(hwnd, INSTALLED).is_invalid() {
            return Ok(());
        }
        let original = GetWindowLongPtrW(hwnd, GWL_STYLE);
        if !SetWindowSubclass(hwnd, Some(caption_proc), SUBCLASS_ID, 0).as_bool() {
            return Err(Error::from_thread());
        }
        window.set_background_appearance(gpui::WindowBackgroundAppearance::Transparent);
        let configured = (|| {
            windows::Win32::Foundation::SetLastError(windows::Win32::Foundation::WIN32_ERROR(0));
            let previous = SetWindowLongPtrW(hwnd, GWL_STYLE, original | WS_CAPTION.0 as isize);
            if previous == 0 && windows::Win32::Foundation::GetLastError().0 != 0 {
                return Err(Error::from_thread());
            }
            // Windows 11 supports an explicit caption palette. Older systems keep their native
            // colors; an unsupported optional color attribute must not disable the controls.
            for (attribute, color) in [
                (DWMWA_CAPTION_COLOR, background),
                (DWMWA_TEXT_COLOR, foreground),
            ] {
                let rgb = color.to_rgb();
                let value = (rgb.r * 255.0).round() as u32
                    | ((rgb.g * 255.0).round() as u32) << 8
                    | ((rgb.b * 255.0).round() as u32) << 16;
                if let Err(error) = DwmSetWindowAttribute(
                    hwnd,
                    attribute,
                    &value as *const _ as _,
                    size_of::<u32>() as u32,
                ) {
                    eprintln!("Native caption palette unavailable: {error}");
                }
            }
            extend_frame(hwnd)?;
            SetWindowPos(
                hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            )?;
            SetPropW(hwnd, INSTALLED, Some(HANDLE(hwnd.0)))
        })();
        if configured.is_err() {
            window.set_background_appearance(gpui::WindowBackgroundAppearance::Opaque);
            SetWindowLongPtrW(hwnd, GWL_STYLE, original);
            let _ = DwmExtendFrameIntoClientArea(hwnd, &MARGINS::default());
            let _ = RemoveWindowSubclass(hwnd, Some(caption_proc), SUBCLASS_ID);
            let _ = SetWindowPos(
                hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
        }
        configured
    }
}

// The top frame is extended into the custom title bar. DWM retains caption-button ownership;
// GPUI calculates the client edges except for our maximized top-edge correction below.
unsafe fn extend_frame(hwnd: HWND) -> Result<()> {
    // SAFETY: called only for the live HWND on its owning thread.
    unsafe {
        let top = if GetWindowLongPtrW(hwnd, GWL_STYLE) & WS_CAPTION.0 as isize != 0 {
            (crate::ui::geometry::TITLEBAR_HEIGHT * GetDpiForWindow(hwnd) as f32 / 96.0).ceil()
                as i32
                + hidden_top(hwnd)
        } else {
            0
        };
        DwmExtendFrameIntoClientArea(
            hwnd,
            &MARGINS {
                cyTopHeight: top,
                ..Default::default()
            },
        )
    }
}

// Keeping a top non-client inset while maximized disables DWM caption hover/hit tests.
// Include the invisible resize frame in the client instead, then pad our content by its height.
// See Chromium HWNDMessageHandler::OnNCCalcSize (native-frame DwmDefWindowProc workaround).
unsafe fn hidden_top(hwnd: HWND) -> i32 {
    unsafe {
        if IsZoomed(hwnd).as_bool()
            && GetWindowLongPtrW(hwnd, GWL_STYLE) & WS_CAPTION.0 as isize != 0
        {
            let dpi = GetDpiForWindow(hwnd);
            GetSystemMetricsForDpi(SM_CYSIZEFRAME, dpi)
                + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi)
        } else {
            0
        }
    }
}

pub fn content_top_inset(window: &Window) -> f32 {
    let Some(hwnd) = hwnd(window) else {
        return 0.0;
    };
    // SAFETY: read-only queries of the current window, on its owning thread.
    unsafe {
        if GetPropW(hwnd, INSTALLED).is_invalid() {
            return 0.0;
        }
        hidden_top(hwnd) as f32 / (GetDpiForWindow(hwnd) as f32 / 96.0).max(1.0)
    }
}

/// The transparent hole uses visible-content coordinates derived from native screen rectangles.
/// The maximized window's hidden frame is accounted for by the shell's padding.
#[derive(Clone, Copy, Debug)]
pub struct CaptionArea {
    pub width: f32,
    pub top: f32,
    pub bottom: f32,
    pub right: f32,
}

pub fn area(window: &Window) -> Option<CaptionArea> {
    let hwnd = hwnd(window)?;
    // SAFETY: read-only queries on our live window's UI thread.
    unsafe {
        if GetPropW(hwnd, INSTALLED).is_invalid() {
            return None;
        }
        Some(query_area(hwnd).unwrap_or(CaptionArea {
            width: 146.0,
            top: 0.0,
            bottom: crate::ui::geometry::TITLEBAR_HEIGHT,
            right: 0.0,
        }))
    }
}

/// Windows supplies separate screen rectangles even when DWM declines a maximized hit test.
unsafe fn button_info(hwnd: HWND) -> Option<TITLEBARINFOEX> {
    unsafe {
        if !IsWindowVisible(hwnd).as_bool()
            || IsIconic(hwnd).as_bool()
            || GetWindowLongPtrW(hwnd, GWL_STYLE) & WS_CAPTION.0 as isize == 0
        {
            return None;
        }
        let mut info = TITLEBARINFOEX {
            cbSize: size_of::<TITLEBARINFOEX>() as u32,
            ..Default::default()
        };
        DefWindowProcW(
            hwnd,
            WM_GETTITLEBARINFOEX,
            WPARAM(0),
            LPARAM(&mut info as *mut _ as isize),
        );
        if [2, 3, 5].into_iter().any(|i| {
            info.rgrect[i].right <= info.rgrect[i].left
                || info.rgrect[i].bottom <= info.rgrect[i].top
        }) {
            return None;
        }
        Some(info)
    }
}

unsafe fn query_area(hwnd: HWND) -> Option<CaptionArea> {
    unsafe {
        let info = button_info(hwnd)?;
        let rects = [info.rgrect[2], info.rgrect[3], info.rgrect[5]];
        let bounds = RECT {
            left: rects.iter().map(|r| r.left).min()?,
            top: rects.iter().map(|r| r.top).min()?,
            right: rects.iter().map(|r| r.right).max()?,
            bottom: rects.iter().map(|r| r.bottom).max()?,
        };
        let mut client = RECT::default();
        let mut origin = POINT::default();
        GetClientRect(hwnd, &mut client).ok()?;
        if !ClientToScreen(hwnd, &mut origin).as_bool() {
            return None;
        }
        // The shell's padding excludes the invisible top resize frame from its layout.
        origin.y += hidden_top(hwnd);
        convert_area(
            bounds,
            origin,
            client.right,
            GetDpiForWindow(hwnd) as f32 / 96.0,
        )
    }
}

fn convert_area(bounds: RECT, origin: POINT, client_width: i32, scale: f32) -> Option<CaptionArea> {
    if !scale.is_finite()
        || scale <= 0.0
        || client_width <= 0
        || bounds.right <= bounds.left
        || bounds.bottom <= bounds.top
    {
        return None;
    }
    let left = (i64::from(bounds.left) - i64::from(origin.x)).clamp(0, i64::from(client_width));
    let right = (i64::from(bounds.right) - i64::from(origin.x)).clamp(0, i64::from(client_width));
    let height = crate::ui::geometry::TITLEBAR_HEIGHT;
    let top = ((i64::from(bounds.top) - i64::from(origin.y)) as f32 / scale).clamp(0.0, height);
    let bottom =
        ((i64::from(bounds.bottom) - i64::from(origin.y)) as f32 / scale).clamp(0.0, height);
    if right <= left || bottom <= top {
        return None;
    }
    Some(CaptionArea {
        width: (i64::from(client_width) - left) as f32 / scale,
        right: (i64::from(client_width) - right) as f32 / scale,
        top,
        bottom,
    })
}

fn button_at(info: &TITLEBARINFOEX, point: POINT) -> Option<u32> {
    for (index, hit) in [(2, HTMINBUTTON), (3, HTMAXBUTTON), (5, HTCLOSE)] {
        let r = info.rgrect[index];
        // STATE_SYSTEM_UNAVAILABLE | INVISIBLE | OFFSCREEN. Disabled buttons stay disabled.
        if info.rgstate[index] & 0x00018001 == 0
            && point.x >= r.left
            && point.x < r.right
            && point.y >= r.top
            && point.y < r.bottom
        {
            return Some(hit);
        }
    }
    None
}

unsafe extern "system" fn caption_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _: usize,
    _: usize,
) -> LRESULT {
    // SAFETY: this procedure is installed only on our HWND. It owns no heap data, and removes
    // its marker/subclass on destruction. All unhandled messages stay with the GPUI procedure.
    unsafe {
        if msg == WM_NCDESTROY {
            let _ = RemovePropW(hwnd, INSTALLED);
            let _ = RemoveWindowSubclass(hwnd, Some(caption_proc), SUBCLASS_ID);
            return DefSubclassProc(hwnd, msg, wparam, lparam);
        }
        if msg == WM_NCCALCSIZE && wparam.0 != 0 && hidden_top(hwnd) > 0 {
            let params = lparam.0 as *mut NCCALCSIZE_PARAMS;
            let top = (*params).rgrc[0].top;
            let _ = DefSubclassProc(hwnd, msg, wparam, lparam);
            (*params).rgrc[0].top = top;
            // The application redraws on resize. Do not preserve stale valid-rectangle offsets.
            return LRESULT(0);
        }
        let mut dwm = LRESULT(0);
        let handled = DwmDefWindowProc(hwnd, msg, wparam, lparam, &mut dwm).as_bool();
        if msg == WM_NCHITTEST {
            // DWM is not guaranteed to claim every hit-test message. The standard window
            // procedure can still identify the genuine caption buttons. Never use its caption
            // or client result here: GPUI owns drag/search/resize areas outside the buttons.
            let mut native = if handled {
                dwm
            } else {
                DefWindowProcW(hwnd, msg, wparam, lparam)
            };
            if !matches!(native.0 as u32, HTMINBUTTON | HTMAXBUTTON | HTCLOSE) {
                let point = POINT {
                    x: lparam.0 as i16 as i32,
                    y: (lparam.0 >> 16) as i16 as i32,
                };
                if let Some(hit) = button_info(hwnd).and_then(|info| button_at(&info, point)) {
                    native = LRESULT(hit as isize);
                }
            }
            if matches!(native.0 as u32, HTMINBUTTON | HTMAXBUTTON | HTCLOSE) {
                return native;
            }
        }
        if handled
            && (msg != WM_NCHITTEST || matches!(dwm.0 as u32, HTMINBUTTON | HTMAXBUTTON | HTCLOSE))
        {
            return dwm;
        }
        if matches!(
            msg,
            WM_NCLBUTTONDOWN | WM_NCLBUTTONUP | WM_NCLBUTTONDBLCLK | WM_NCMOUSEMOVE
        ) && matches!(wparam.0 as u32, HTMINBUTTON | HTMAXBUTTON | HTCLOSE)
        {
            // GPUI normally maps these to custom controls. Native controls must go to Windows.
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        let result = DefSubclassProc(hwnd, msg, wparam, lparam);
        if matches!(
            msg,
            WM_ACTIVATE | WM_SIZE | WM_DPICHANGED | WM_DWMCOMPOSITIONCHANGED | WM_STYLECHANGED
        ) {
            let _ = extend_frame(hwnd);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maximized_caption_does_not_subtract_the_hidden_frame_twice() {
        // Captured at 150% DPI: window origin (-11,-11), client origin (0,0).
        // TITLEBARINFOEX is already screen-relative and includes no invisible frame offset.
        let area = convert_area(
            RECT {
                left: 3627,
                top: 0,
                right: 3840,
                bottom: 43,
            },
            POINT { x: 0, y: 0 },
            3840,
            1.5,
        )
        .unwrap();
        assert_eq!(area.width, 142.0);
        assert_eq!(area.bottom, 43.0 / 1.5);
        assert_eq!(area.top, 0.0);
        assert_eq!(area.right, 0.0);
    }

    #[test]
    fn screen_coordinates_support_negative_monitor_origins() {
        let area = convert_area(
            RECT {
                left: -213,
                top: -1200,
                right: 0,
                bottom: -1157,
            },
            POINT { x: -1800, y: -1200 },
            1800,
            1.5,
        )
        .unwrap();
        assert_eq!(
            (area.width, area.top, area.bottom),
            (142.0, 0.0, 43.0 / 1.5)
        );
        assert!(convert_area(RECT::default(), POINT::default(), 1800, 1.5).is_none());
    }

    #[test]
    fn maximized_button_centers_hit_the_native_controls() {
        let mut info = TITLEBARINFOEX::default();
        info.rgrect[2] = RECT {
            left: 3627,
            top: 0,
            right: 3698,
            bottom: 43,
        };
        info.rgrect[3] = RECT {
            left: 3698,
            top: 0,
            right: 3768,
            bottom: 43,
        };
        info.rgrect[5] = RECT {
            left: 3768,
            top: 0,
            right: 3840,
            bottom: 43,
        };
        assert_eq!(
            button_at(&info, POINT { x: 3660, y: 21 }),
            Some(HTMINBUTTON)
        );
        assert_eq!(
            button_at(&info, POINT { x: 3733, y: 21 }),
            Some(HTMAXBUTTON)
        );
        assert_eq!(button_at(&info, POINT { x: 3800, y: 21 }), Some(HTCLOSE));
        assert_eq!(button_at(&info, POINT { x: 3733, y: 43 }), None);
        info.rgstate[3] = 1;
        assert_eq!(button_at(&info, POINT { x: 3733, y: 21 }), None);
    }
}
