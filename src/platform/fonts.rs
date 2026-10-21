//! Register fixed font bytes embedded in the executable, independent of the working directory.

use std::borrow::Cow;

use gpui::{App, Result};

pub const UI_FAMILY: &str = "Noto Sans";
pub const CODE_FAMILY: &str = "Google Sans Code";

pub fn register(cx: &App) -> Result<()> {
    cx.text_system().add_fonts(vec![
        Cow::Borrowed(include_bytes!(
            "../../assets/fonts/noto-sans/NotoSans-Regular.ttf"
        )),
        Cow::Borrowed(include_bytes!(
            "../../assets/fonts/google-sans-code/GoogleSansCode.ttf"
        )),
    ])
}
