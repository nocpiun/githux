//! Drawing a real avatar with a stable fallback, NSH native.
use nsh_core::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AvatarKey {
    pub subject: String,
    pub url: String,
}

impl AvatarKey {
    pub fn new(host: impl AsRef<str>, id: u64, url: impl Into<String>) -> Self {
        Self {
            subject: format!("{}#{id}", host.as_ref()),
            url: url.into(),
        }
    }
}

impl crate::data::avatar_cache::AvatarKeyLike for AvatarKey {
    fn subject(&self) -> &str {
        &self.subject
    }
    fn url(&self) -> &str {
        &self.url
    }
}

pub fn initials(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().collect();
    let first = |word: &str| word.chars().next().into_iter().flat_map(char::to_uppercase);
    match words.as_slice() {
        [] => String::new(),
        [only] => first(only).collect(),
        [head, .., tail] => first(head).chain(first(tail)).collect(),
    }
}

pub fn tint(name: &str) -> Hsla {
    let hash = name.bytes().fold(0u32, |hash, byte| {
        hash.wrapping_mul(31).wrapping_add(u32::from(byte))
    });
    let hues = [200.0, 150.0, 30.0, 280.0, 340.0, 80.0, 190.0];
    let h = hues[hash as usize % hues.len()];
    hsla(h, 0.45, 0.40, 1.0)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AvatarSlot {
    Xs,
    Sm,
    Md,
    Lg,
    Xl,
    Custom(f32),
}

impl AvatarSlot {
    pub fn points(self) -> f32 {
        match self {
            Self::Xs => 20.0,
            Self::Sm => 24.0,
            Self::Md => 32.0,
            Self::Lg => 48.0,
            Self::Xl => 64.0,
            Self::Custom(pts) => pts,
        }
    }
}

pub struct AvatarView<'a> {
    pub id: &'a str,
    pub selector: &'a str,
    pub url: Option<&'a str>,
    pub name: &'a str,
    pub slot: AvatarSlot,
}

pub fn avatar(view: AvatarView<'_>) -> AnyElement {
    let size = view.slot.points();
    let radius = size / 2.0;
    let fallback_initials = initials(view.name);
    let name_tint = tint(view.name);

    let container = nsh_core::element::view()
        .size(px(size))
        .rounded(px(radius))
        .overflow_hidden()
        .items_center()
        .justify_center()
        .bg(name_tint)
        .test_id(view.selector)
        .role(AccessibilityRole::Image)
        .label(view.name.to_string());

    if let Some(url) = view.url.filter(|url| !url.trim().is_empty()) {
        return container
            .child(image(url.to_owned()).size(px(size)).rounded(px(radius)))
            .into_element();
    }

    container
        .child(
            text(fallback_initials)
                .font_family(crate::platform::fonts::UI_FAMILY)
                .text_size(px((size * 0.45).clamp(10.0, 20.0)))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(0xffffff)),
        )
        .into_element()
}
