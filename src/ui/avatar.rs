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
use gpui::{
    AnyElement, App, Bounds, Element, ElementId, FontWeight, GlobalElementId, InspectorElementId,
    IntoElement, LayoutId, ParentElement, Pixels, Styled, Window, div, prelude::*,
};

/// The stable identity of one avatar. It is deliberately not a row index or a login string: the
/// account/owner key stays the same when the list reorders, and a changed URL is a different key so
/// the old image is never shown for the new identity.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AvatarKey {
    /// `host` + numeric owner/account id, when known, so one host's id cannot match another's.
    pub subject: String,
    /// The exact URL GitHub returned. A different URL is a different key, so a renamed account or a
    /// refreshed URL cannot keep serving the previous picture.
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
            // Ely forwards the base style's corner radii to the inner `gpui::img`, so the rounding
            // belongs on the image node itself: outer clipping alone can still leave opaque square
            // corners painted. The cover fit keeps the crop centred and undistorted.
            ely_core::primitives::Image::new(element_id, source)
                .fit(gpui::ObjectFit::Cover)
                .size_full()
                .rounded_full()
                .into_any_element()
        }
        None => div()
            .text_size(theme.text_size(text))
            .font_weight(FontWeight::SEMIBOLD)
            .child(initials(&name_owned))
            .into_any_element(),
    };
    VisibleAvatar(
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
            .into_any_element(),
    )
    .into_any_element()
}

/// Keeps GPUI 0.3.10 from reviving an empty ancestor mask during device-pixel snapping.
/// Layout and prepaint still run normally; only completely clipped avatar painting is skipped.
/// Guard the frame too: its tinted fallback background must not revive an empty mask either.
/// Partially visible avatars retain the original ancestor mask and the image's own circle/cover.
struct VisibleAvatar(AnyElement);

impl IntoElement for VisibleAvatar {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for VisibleAvatar {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.0.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.0.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let visible = window.content_mask().bounds.intersect(&bounds);
        if visible.size.width > Pixels::ZERO && visible.size.height > Pixels::ZERO {
            self.0.paint(window, cx);
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, ImageSource, Render, ScrollHandle, TestAppContext, px};
    use std::sync::{Arc, Mutex};

    struct ClipProbe {
        image: ImageSource,
        scroll: ScrollHandle,
        ready: bool,
    }
    impl Render for ClipProbe {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let face = avatar(
                AvatarView {
                    id: "disjoint",
                    selector: "disjoint",
                    image: self.ready.then(|| self.image.clone()),
                    name: "Opaque Square",
                    slot: AvatarSlot::Rems(gpui::rems(2.0)),
                },
                cx,
            );
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(
                    gpui::canvas(
                        move |bounds, window, cx| {
                            let mut face = face;
                            face.prepaint_as_root(
                                bounds.origin,
                                gpui::size(
                                    gpui::AvailableSpace::Definite(px(32.0)),
                                    gpui::AvailableSpace::Definite(px(32.0)),
                                ),
                                window,
                                cx,
                            );
                            face
                        },
                        |bounds, mut face, window, cx| {
                            let first = gpui::ContentMask {
                                bounds: Bounds::new(bounds.origin, gpui::size(px(32.0), px(8.0))),
                            };
                            let second = gpui::ContentMask {
                                bounds: Bounds::new(
                                    bounds.origin + gpui::point(px(0.0), px(12.25)),
                                    gpui::size(px(32.0), px(20.0)),
                                ),
                            };
                            window.with_content_mask(Some(first), |window| {
                                window.with_content_mask(Some(second), |window| {
                                    face.paint(window, cx)
                                })
                            });
                        },
                    )
                    .absolute()
                    .left(px(100.0))
                    .top(px(200.0))
                    .size(px(32.0)),
                )
                .child(
                    div()
                        .id("viewport")
                        .h(px(80.25))
                        .w(px(80.0))
                        .flex_none()
                        .overflow_y_scroll()
                        .overflow_x_hidden()
                        .track_scroll(&self.scroll)
                        .child(div().flex().flex_col().children((0..12).map(|row| {
                            div().h(px(32.25)).flex_none().child(avatar(
                                AvatarView {
                                    id: &format!("clip-{row}"),
                                    selector: &format!("clip-{row}"),
                                    image: self.ready.then(|| self.image.clone()),
                                    name: "Opaque Square",
                                    slot: AvatarSlot::Rems(gpui::rems(2.0)),
                                },
                                cx,
                            ))
                        }))),
                )
                .child(
                    div()
                        .id("fixed-footer")
                        .debug_selector(|| "fixed-footer".to_string())
                        .h(px(100.0))
                        .w_full()
                        .flex_none()
                        .child("Fixed footer"),
                )
        }
    }

    #[gpui::test]
    fn offscreen_images_never_reach_paint_with_an_empty_fractional_mask(cx: &mut TestAppContext) {
        cx.update(ely_core::init_for_tests);
        let pixels = image::RgbaImage::from_pixel(16, 16, image::Rgba([255, 12, 32, 255]));
        let mut png = std::io::Cursor::new(Vec::new());
        pixels.write_to(&mut png, image::ImageFormat::Png).unwrap();
        let image = cx.update(|cx| {
            gpui::Image::from_bytes(gpui::ImageFormat::Png, png.into_inner())
                .to_image_data(cx.svg_renderer())
                .unwrap()
        });
        let masks = Arc::new(Mutex::new(Vec::new()));
        let recorded = masks.clone();
        let source = ImageSource::Custom(Arc::new(move |window, _| {
            recorded.lock().unwrap().push(window.content_mask().bounds);
            Some(Ok(image.clone()))
        }));
        let (view, cx) = cx.add_window_view(|_, _| ClipProbe {
            image: source,
            scroll: ScrollHandle::new(),
            ready: false,
        });
        // Scroll while still showing initials, then settle the delayed image response.
        view.update(cx, |probe, cx| {
            probe.scroll.set_offset(gpui::point(px(0.0), px(-16.5)));
            cx.notify();
        });
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        view.update(cx, |probe, cx| {
            probe.ready = true;
            cx.notify();
        });
        for scale in [1.0, 1.5] {
            cx.simulate_scale_factor_change(scale);
            for offset in [0.0, -0.25, -16.5, -48.25] {
                masks.lock().unwrap().clear();
                view.update(cx, |probe, cx| {
                    probe.scroll.set_offset(gpui::point(px(0.0), px(offset)));
                    cx.notify();
                });
                cx.update(|window, cx| {
                    window.refresh();
                    window.draw(cx).clear(cx);
                });
                let samples = masks.lock().unwrap();
                assert!(!samples.is_empty(), "partially visible images still draw");
                assert!(
                    samples
                        .iter()
                        .all(|mask| mask.size.width > Pixels::ZERO
                            && mask.size.height > Pixels::ZERO),
                    "offscreen image paint revived an empty mask at scale {scale}, offset {offset}: {samples:?}"
                );
                assert!(
                    samples
                        .iter()
                        .any(|mask| mask.size.height > Pixels::ZERO && mask.size.height < px(32.0)),
                    "boundary avatar keeps its partial clip"
                );
                assert!(cx.debug_bounds("fixed-footer").is_some());
            }
        }
    }
}
