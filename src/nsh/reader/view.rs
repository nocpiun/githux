use super::{
    LinkTarget, ReaderBody, ReaderDocument,
    images::{ImageBridge, Picture},
    links,
};
use nsh_core::prelude::*;
use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::Arc};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReaderLayout {
    Default,
    Repository,
    Flow,
}
#[derive(Debug, Clone, PartialEq)]
pub enum ReaderAction {
    Link(LinkTarget),
    LayoutChanged { height: f32 },
    AuthorizationRetired,
}
type ActionHandler = Rc<dyn Fn(ReaderAction)>;
struct Inner {
    document: Signal<Option<Arc<ReaderDocument>>>,
    pictures: Signal<u64>,
    bridge: RefCell<ImageBridge>,
    layout: ReaderLayout,
    action: ActionHandler,
    scroll: ElementRef,
    document_top: RefCell<f32>,
    anchors: RefCell<HashMap<String, f32>>,
}
/// Create on the NSH UI thread inside a component's reactive scope. Hold one per page reader.
#[derive(Clone)]
pub struct ReaderHandle(Rc<Inner>);
impl ReaderHandle {
    pub fn new(
        document: Arc<ReaderDocument>,
        bridge: ImageBridge,
        layout: ReaderLayout,
        on_action: impl Fn(ReaderAction) + 'static,
    ) -> Self {
        Self(Rc::new(Inner {
            document: signal(Some(document)),
            pictures: signal(0),
            bridge: RefCell::new(bridge),
            layout,
            action: Rc::new(on_action),
            scroll: element_ref(),
            document_top: RefCell::new(0.0),
            anchors: RefCell::new(HashMap::new()),
        }))
    }
    /// Host drain, before publishing/rendering a snapshot. No I/O; provider.start only queues background work.
    pub fn poll(&self) -> bool {
        let changed = self.0.bridge.borrow_mut().poll();
        let current = self.0.bridge.borrow().is_current();
        if !current && self.0.document.get_untracked().is_some() {
            self.0.document.set(None);
            self.0.anchors.borrow_mut().clear();
            (self.0.action)(ReaderAction::AuthorizationRetired);
        }
        if changed {
            self.0.pictures.update(|n| *n += 1);
        }
        changed
    }
    pub fn has_pending_images(&self) -> bool {
        self.0.bridge.borrow().has_work()
    }
    /// Already prepared by a background worker. Rebuilds the document subtree, clearing old selection.
    pub fn replace(&self, document: Arc<ReaderDocument>, bridge: ImageBridge) {
        self.0.bridge.replace(bridge);
        self.0.anchors.borrow_mut().clear();
        self.0.document.set(Some(document));
        self.0.scroll.scroll_to(point(px(0.0), px(0.0)), false);
    }
    pub fn suspend(&self) {
        self.0.bridge.borrow_mut().suspend();
    }
    pub fn retire(&self) {
        self.0.bridge.borrow_mut().retire();
        self.poll();
    }
    pub fn handle_link(&self, url: &str) {
        if !self.0.bridge.borrow().is_current() {
            self.retire();
            return;
        }
        if let Some(document) = self.0.document.get_untracked() {
            let target = links::resolve(url, &document.context);
            if let LinkTarget::Anchor { id } = &target {
                self.jump_to_anchor(id);
            }
            (self.0.action)(ReaderAction::Link(target));
        }
    }
    /// Bounded readers jump locally. In Flow the host receives the link and scrolls its page.
    pub fn jump_to_anchor(&self, id: &str) -> bool {
        if self.0.layout == ReaderLayout::Flow {
            return false;
        }
        if let Some(y) = self.0.anchors.borrow().get(id) {
            self.0.scroll.scroll_to(
                point(px(0.0), px((*y - *self.0.document_top.borrow()).max(0.0))),
                false,
            );
            true
        } else {
            false
        }
    }
    pub fn scroll_to_end(&self) {
        if self.0.layout != ReaderLayout::Flow {
            self.0.scroll.scroll_to_end(false);
        }
    }
}

/// Mount once in a page. The host calls handle.poll() on its UI drain and retires it on invalidation.
pub fn reader(handle: ReaderHandle) -> AnyElement {
    let cleanup_handle = handle.clone();
    cleanup(move || cleanup_handle.0.bridge.borrow_mut().retire());
    let content_handle = handle.clone();
    let content = dynamic(move || {
        let h = content_handle.clone();
        match h.0.document.get() {
            None => view()
                .test_id("reader-retired")
                .p(px(12.0))
                .child(text("内容已撤权"))
                .into_element(),
            Some(document) => render_document(h, document),
        }
    });
    if handle.0.layout == ReaderLayout::Flow {
        content
    } else {
        view().size_full().child(content).into_element()
    }
}
/// NSH's default tokens with the app's font families. Fonts are `.font()`-registered by the host;
/// probing/fixture roots may call this with real, optional files. Host owns final palette/typography.
pub fn dark_tokens() -> nsh_core::element::Tokens {
    use nsh_core::element::Tokens;
    let mut tokens = Tokens::default();
    tokens.set_font(nsh_core::element::FONT_SANS, "Githux UI");
    tokens.set_font(nsh_core::element::FONT_MONO, "Google Sans Code");
    tokens
}
fn render_document(handle: ReaderHandle, document: Arc<ReaderDocument>) -> AnyElement {
    match &document.body {
        ReaderBody::Status(label) => view().p(px(12.0)).child(text(*label)).into_element(),
        ReaderBody::Source {
            text: source,
            language,
        } => view()
            .w_full()
            .h_full()
            .test_id("reader-source")
            .child(
                nsh_base::editor()
                    .default_value(source.clone())
                    .language(language.clone().map(SharedString::from))
                    .read_only(true)
                    .line_numbers(true)
                    .folding(false)
                    .active_line(false)
                    .indent_guides(false)
                    .scroll_beyond_last_line(nsh_base::EditorScrollBeyond::from(false))
                    .font_family("Google Sans Code")
                    .font_size(px(14.0))
                    .line_height(px(22.4))
                    .w_full()
                    .h_full(),
            )
            .into_element(),
        ReaderBody::Markdown(document_model) => {
            let origin_handle = handle.clone();
            let height = Rc::new(RefCell::new(-1.0f32));
            let mut column = view()
                .flex_col()
                .gap(px(12.0))
                .w_full()
                .py(px(12.0))
                .px(px(if handle.0.layout == ReaderLayout::Default {
                    8.0
                } else {
                    12.0
                }))
                .selectable(true)
                .test_id("reader-document")
                .on_layout(move |bounds: &Bounds<Points>| {
                    *origin_handle.0.document_top.borrow_mut() = bounds.origin.y.0;
                    if *height.borrow() != bounds.size.height.0 {
                        *height.borrow_mut() = bounds.size.height.0;
                        (origin_handle.0.action)(ReaderAction::LayoutChanged {
                            height: bounds.size.height.0,
                        });
                    }
                });
            for (index, block) in document_model.blocks.iter().enumerate() {
                let image_handle = handle.clone();
                let image_context = document.context.clone();
                let link_handle = handle.clone();
                let block_view = nsh_base::text_view()
                    .document(nsh_base::text_view::Document::new(vec![block.clone()]))
                    .selectable(true)
                    .links(nsh_base::TextViewLinks::Report)
                    .on_link_press(move |event: &nsh_base::TextViewLinkPressEvent| {
                        link_handle.handle_link(&event.url)
                    })
                    .render_image(move |image: &nsh_base::text_view::Image| {
                        let h = image_handle.clone();
                        let context = image_context.clone();
                        let raw = image.src.clone();
                        let alt = image.alt.clone();
                        dynamic(move || {
                            h.0.pictures.get();
                            let picture = h.0.bridge.borrow_mut().picture(&raw, &context);
                            match picture {
                                Picture::Ready {
                                    source,
                                    width,
                                    height,
                                } => {
                                    // Only internally generated, provider-validated bytes reach this loader.
                                    let ratio = width.max(1) as f32 / height.max(1) as f32;
                                    let display_width = (width as f32).min(640.0 * ratio);
                                    view()
                                        .flex_col()
                                        .max_w(px(display_width))
                                        .w_full()
                                        .child(
                                            nsh_core::element::image(source)
                                                .w_full()
                                                .aspect_ratio(ratio)
                                                .object_fit(nsh_core::ResizeMode::Contain)
                                                .label(alt.clone()),
                                        )
                                        .child(text(alt.clone()).select_text())
                                        .into_element()
                                }
                                Picture::Pending => view()
                                    .h(px(140.0))
                                    .child(text(alt.clone()).select_text())
                                    .child(text("loading picture").select_none())
                                    .into_element(),
                                Picture::Failed(error) => view()
                                    .h(px(140.0))
                                    .child(text(alt.clone()).select_text())
                                    .child(text(error.label()).select_none())
                                    .into_element(),
                            }
                        })
                    });
                let mut row = view()
                    .test_id(&format!("reader-block-{index}"))
                    .child(block_view);
                if let nsh_base::text_view::Block::Heading {
                    anchor: Some(anchor),
                    ..
                } = block
                {
                    let anchor = anchor.clone();
                    let h = handle.clone();
                    row = row.on_layout(move |bounds: &Bounds<Points>| {
                        h.0.anchors
                            .borrow_mut()
                            .insert(anchor.clone(), bounds.origin.y.0);
                    });
                }
                column = column.child(row);
            }
            if handle.0.layout == ReaderLayout::Flow {
                column.into_element()
            } else {
                scroll()
                    .element_ref(&handle.0.scroll)
                    .size_full()
                    .test_id("reader-scroll")
                    .child(column)
                    .into_element()
            }
        }
    }
}
