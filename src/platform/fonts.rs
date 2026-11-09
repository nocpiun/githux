//! Register fixed font bytes embedded in the executable, independent of the working directory.
//!
//! The private UI family has original Noto Sans glyphs and Noto Sans SC glyphs for characters
//! outside Noto Sans's coverage.
//! Google Sans Code remains independent.

use nsh_engine::Root;

pub const UI_FAMILY: &str = "Githux UI";
pub const CODE_FAMILY: &str = "Google Sans Code";

pub fn install(root: Root) -> Root {
    root.font(&include_bytes!("../../assets/fonts/githux-ui/GithuxUI-Regular.ttf")[..])
        .font(&include_bytes!("../../assets/fonts/githux-ui/GithuxUI-Medium.ttf")[..])
        .font(&include_bytes!("../../assets/fonts/githux-ui/GithuxUI-SemiBold.ttf")[..])
        .font(&include_bytes!("../../assets/fonts/githux-ui/GithuxUI-Bold.ttf")[..])
        .font(&include_bytes!("../../assets/fonts/google-sans-code/GoogleSansCode.ttf")[..])
}
