//! Register fixed font bytes embedded in the executable, independent of the working directory.
//!
//! Noto Sans is bundled as three static faces (Regular, Medium, SemiBold) from the same official
//! release. GPUI chooses a face by the requested weight, so Medium and SemiBold have to be
//! registered explicitly: with only Regular present, a `.font_weight(MEDIUM)` or `SEMIBOLD` request
//! is answered by DirectWrite with a substituted or synthetic face instead of the family's real
//! weight. Google Sans Code stays one variable upright file and covers code text.

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
            "../../assets/fonts/noto-sans/NotoSans-Medium.ttf"
        )),
        Cow::Borrowed(include_bytes!(
            "../../assets/fonts/noto-sans/NotoSans-SemiBold.ttf"
        )),
        Cow::Borrowed(include_bytes!(
            "../../assets/fonts/google-sans-code/GoogleSansCode.ttf"
        )),
    ])
}
