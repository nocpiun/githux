//! Offline native visual check for the production avatar renderer.
//!
//! `cargo run --example avatar` renders an opaque square with bright red corners next to the
//! same image through the shared avatar renderer. It never opens account storage or the network.
//! The columns cover repository, interpolated account and Settings sizes; the last uses initials.

#[path = "../src/app/mod.rs"]
#[allow(dead_code)]
mod app;
#[path = "../src/auth/mod.rs"]
mod auth;
#[path = "../src/content/mod.rs"]
mod content;
#[path = "../src/data/mod.rs"]
mod data;
#[path = "../src/diagnostics/mod.rs"]
mod diagnostics;
#[path = "../src/domain/mod.rs"]
mod domain;
#[path = "../src/github/mod.rs"]
mod github;
#[path = "../src/network/mod.rs"]
mod network;
#[path = "../src/platform/mod.rs"]
#[allow(dead_code)]
mod platform;
#[path = "../src/storage/mod.rs"]
mod storage;
#[path = "../src/ui/mod.rs"]
#[allow(dead_code)]
mod ui;
#[path = "../src/workspace/mod.rs"]
#[allow(dead_code)]
mod workspace;

use gpui::{
    App, AppContext, Bounds, Context, ImageSource, InteractiveElement, IntoElement, ObjectFit,
    ParentElement, Render, ScrollHandle, StatefulInteractiveElement, Styled, StyledImage, Window,
    WindowBounds, WindowOptions, div, img, px, rems, rgb, size,
};
use std::{io::Cursor, sync::Arc};
use ui::avatar::{AvatarSlot, AvatarView, avatar};

struct Probe {
    image: ImageSource,
    scroll: ScrollHandle,
}

impl Render for Probe {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let background = rgb(0x18212a);
        let mut row = div()
            .absolute()
            .left(px(32.0))
            .top(px(64.0))
            .flex()
            .gap(px(32.0));
        for (index, diameter) in [64.0, 20.0, 26.0, 32.0, 24.0, 32.0].into_iter().enumerate() {
            let face = if index == 0 {
                img(self.image.clone())
                    .size(px(diameter))
                    .object_fit(ObjectFit::Cover)
                    .into_any_element()
            } else {
                avatar(
                    AvatarView {
                        id: &format!("probe-{index}"),
                        selector: &format!("probe-{index}"),
                        image: (index != 5).then(|| self.image.clone()),
                        name: "Round Avatar",
                        slot: AvatarSlot::Rems(rems(diameter / 16.0)),
                    },
                    cx,
                )
            };
            row = row.child(div().w(px(64.0)).h(px(64.0)).child(face));
        }
        let mask_face = avatar(
            AvatarView {
                id: "disjoint-mask",
                selector: "disjoint-mask",
                image: Some(self.image.clone()),
                name: "Disjoint Mask",
                slot: AvatarSlot::Rems(rems(2.0)),
            },
            cx,
        );
        div()
            .size_full()
            .bg(background)
            .text_color(rgb(0xffffff))
            .child(
                div().absolute().left(px(32.0)).top(px(24.0)).child(
                    "Square source / repository / transition / account / Settings / fallback",
                ),
            )
            .child(row)
            .child(
                div()
                    .absolute()
                    .left(px(464.0))
                    .top(px(300.0))
                    .child("Disjoint masks"),
            )
            .child(
                gpui::canvas(
                    move |bounds, window, cx| {
                        let mut face = mask_face;
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
                        let viewport = gpui::ContentMask {
                            bounds: Bounds::new(bounds.origin, size(px(32.0), px(8.0))),
                        };
                        let nested = gpui::ContentMask {
                            bounds: Bounds::new(
                                bounds.origin + gpui::point(px(0.0), px(12.25)),
                                size(px(32.0), px(20.0)),
                            ),
                        };
                        window.with_content_mask(Some(viewport), |window| {
                            window.with_content_mask(Some(nested), |window| face.paint(window, cx))
                        });
                    },
                )
                .absolute()
                .left(px(480.0))
                .top(px(330.0))
                .size(px(32.0)),
            )
            .child(
                div()
                    .absolute()
                    .left(px(32.0))
                    .top(px(158.0))
                    .w(px(400.0))
                    .flex()
                    .flex_col()
                    .child(div().child("Bounded list: scroll wheel; fixed footer below"))
                    .child(
                        div()
                            .id("avatar-viewport")
                            .h(px(80.0))
                            .w_full()
                            .overflow_y_scroll()
                            .overflow_x_hidden()
                            .track_scroll(&self.scroll)
                            .child(div().flex().flex_col().children((0..12).map(|index| {
                                div()
                                    .h(px(31.0))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .gap(px(12.0))
                                    .child(avatar(
                                        AvatarView {
                                            id: &format!("scroll-avatar-{index}"),
                                            selector: "scroll-avatar",
                                            image: Some(self.image.clone()),
                                            name: "Square Image",
                                            slot: AvatarSlot::Rems(rems(1.5)),
                                        },
                                        cx,
                                    ))
                                    .child(format!("Owner {index}"))
                            }))),
                    )
                    // No opaque cover-up: any escaped red fragment is visible on the same background.
                    .child(
                        div()
                            .h(px(160.0))
                            .flex_none()
                            .child("Fixed account / shortcuts area"),
                    ),
            )
    }
}

fn main() {
    gpui_platform::application()
        .with_assets(ely_core::Assets)
        .run(|cx: &mut App| {
            ely_core::init(cx).expect("Ely");
            platform::fonts::register(cx).expect("fonts");
            let mut pixels = image::RgbaImage::from_pixel(64, 64, image::Rgba([255, 24, 48, 255]));
            for y in 16..48 {
                for x in 16..48 {
                    pixels.put_pixel(x, y, image::Rgba([24, 224, 112, 255]));
                }
            }
            let mut encoded = Cursor::new(Vec::new());
            pixels
                .write_to(&mut encoded, image::ImageFormat::Png)
                .expect("PNG fixture");
            let image = gpui::Image::from_bytes(gpui::ImageFormat::Png, encoded.into_inner())
                .to_image_data(cx.svg_renderer())
                .expect("decoded fixture");
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                        None,
                        size(px(640.0), px(460.0)),
                        cx,
                    ))),
                    titlebar: Some(gpui::TitlebarOptions {
                        title: Some("Githux · circular avatar check".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                |_, cx| {
                    cx.new(|_| Probe {
                        image: ImageSource::Render(Arc::clone(&image)),
                        scroll: ScrollHandle::new(),
                    })
                },
            )
            .expect("avatar check window");
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            cx.activate(true);
        });
}
