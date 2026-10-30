//! Register fixed font bytes embedded in the executable, independent of the working directory.
//!
//! The private UI family has original Noto Sans glyphs and Noto Sans SC glyphs for characters
//! outside Noto Sans's coverage. This avoids GPUI/Windows' system-only fallback lookup: Chinese
//! resolves directly in our in-memory font, even without an OS installation. The reproducible
//! OFL derivative and per-character outline verification live in scripts/build-ui-fonts.py.
//! Google Sans Code remains independent.

use std::borrow::Cow;

use gpui::{App, Result};

pub const UI_FAMILY: &str = "Githux UI";
pub const CODE_FAMILY: &str = "Google Sans Code";

pub fn register(cx: &App) -> Result<()> {
    cx.text_system().add_fonts(vec![
        Cow::Borrowed(include_bytes!(
            "../../assets/fonts/githux-ui/GithuxUI-Regular.ttf"
        )),
        Cow::Borrowed(include_bytes!(
            "../../assets/fonts/githux-ui/GithuxUI-Medium.ttf"
        )),
        Cow::Borrowed(include_bytes!(
            "../../assets/fonts/githux-ui/GithuxUI-SemiBold.ttf"
        )),
        Cow::Borrowed(include_bytes!(
            "../../assets/fonts/githux-ui/GithuxUI-Bold.ttf"
        )),
        Cow::Borrowed(include_bytes!(
            "../../assets/fonts/google-sans-code/GoogleSansCode.ttf"
        )),
    ])
}
