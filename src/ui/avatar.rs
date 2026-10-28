//! Drawing a real avatar with a stable fallback.
//!
//! Ely's `Avatar` accepts an `ImageSource` but falls back to a broken-image icon, not to initials,
//! when the image fails. This module keeps the accepted look — circle, tinted initials, fixed size —
//! and only swaps in the picture when one is actually decoded, so the broken-image icon never
//! appears and the layout never moves.
//!
//! It is deliberately small: the account footer, each repository row's namespace and the saved
//! account list all use it, so there is one avatar renderer for the whole account surface instead
//! of a second loader per component.

use ely_core::theme::{ActiveTheme, AvatarSize, TextSize};
use gpui::{AnyElement, App, FontWeight, IntoElement, ParentElement, Styled, div, prelude::*};

use crate::app::avatar::AvatarKey;

/// Up to two capitals from a name's first and last words, matching Ely's convention so the fallback
/// does not look like a different component.
fn initials(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().collect();
    let first = |word: &str| word.chars().next().into_iter().flat_map(char::to_uppercase);
    match words.as_slice() {
        [] => String::new(),
        [only] => first(only).collect(),
        [head, .., tail] => first(head).chain(first(tail)).collect(),
    }
}

/// The tint a name keeps, so the same identity reads the same way everywhere.
fn tint(name: &str, cx: &App) -> gpui::Hsla {
    let colors = &cx.theme().colors;
    let hash = name.bytes().fold(0u32, |hash, byte| {
        hash.wrapping_mul(31).wrapping_add(u32::from(byte))
    });
    colors.chart[hash as usize % colors.chart.len()]
}

/// Which size slot an avatar occupies.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AvatarSlot {
    /// A fixed Ely avatar size (used by the sidebar rows and the account list).
    Size(AvatarSize),
    /// A continuously interpolated diameter in rems (used while the sidebar expands/collapses, so
    /// the avatar grows with its row instead of snapping between two sizes).
    Rems(gpui::Rems),
}

impl AvatarSlot {
    fn rems(self, cx: &App) -> gpui::Rems {
        match self {
            Self::Size(size) => cx.theme().avatar_size(size),
            Self::Rems(value) => value,
        }
    }
}

/// One avatar frame for the account surface.
pub struct AvatarView<'a> {
    /// A stable id, so GPUI keeps per-image state between frames without crossing identities.
    pub id: &'a str,
    /// The debug selector the existing tests and layout lookups use for this avatar.
    pub selector: &'a str,
    /// The picture to draw, when one is decoded.
    pub image: Option<gpui::ImageSource>,
    /// The identity the initials and tint come from (display name, login or owner login).
    pub name: &'a str,
    pub slot: AvatarSlot,
}

/// Renders the avatar: the decoded picture when present, otherwise initials on the name's tint at
/// the same size. The element chain never changes size between those two states, so a late image
/// does not push the row or the user button around.
pub fn avatar(view: AvatarView<'_>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let side = view.slot.rems(cx);
    // `Image::new` wants an owned id; the debug selector needs its own copy.
    let element_id: gpui::ElementId = gpui::ElementId::Name(view.id.to_string().into());
    let selector = view.selector.to_string();
    let name_owned = view.name.to_string();
    let color = tint(&name_owned, cx);
    let text = match view.slot {
        AvatarSlot::Size(AvatarSize::Xs | AvatarSize::Sm) => TextSize::Xs,
        AvatarSlot::Size(AvatarSize::Md) => TextSize::Sm,
        AvatarSlot::Size(AvatarSize::Lg) => TextSize::Md,
        AvatarSlot::Size(AvatarSize::Xl) => TextSize::Lg,
        AvatarSlot::Rems(_) => TextSize::Xs,
    };
    let face: AnyElement = match view.image {
        Some(source) => {
            // The picture fills the frame. `Image::new` already round-clips via its own corners; we
            // rely on the outer `rounded_full` + `overflow_hidden` to keep the circle.
            ely_core::primitives::Image::new(element_id, source)
                .fit(gpui::ObjectFit::Cover)
                .size_full()
                .into_any_element()
        }
        None => div()
            .text_size(theme.text_size(text))
            .font_weight(FontWeight::SEMIBOLD)
            .child(initials(&name_owned))
            .into_any_element(),
    };
    div()
        .debug_selector(move || selector.clone())
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(side)
        .rounded_full()
        .overflow_hidden()
        .bg(color.opacity(0.16))
        .text_color(color)
        .child(face)
        .into_any_element()
}

/// The avatar key for a repository's namespace owner, when it has a URL. The numeric owner id keeps
/// the identity stable across a rename, and the URL distinguishes a genuinely new picture.
pub fn owner_key(owner: &crate::domain::RepoOwner, host: &str) -> Option<AvatarKey> {
    owner
        .avatar_url
        .as_deref()
        .filter(|url| !url.trim().is_empty())
        .map(|url| AvatarKey::new(host, owner.id, url))
}

/// The avatar key for an account, using its stable account id.
pub fn account_key(identity: &crate::domain::AccountIdentity) -> Option<AvatarKey> {
    identity
        .avatar_url
        .as_deref()
        .filter(|url| !url.trim().is_empty())
        .map(|url| AvatarKey::new(identity.key.host.as_str(), identity.key.id, url))
}
