use crate::{
    content::{PreparedBody, images::ImageLimits},
    reader::{
        DocumentKind, ReaderDocument, ReaderHandle, ReaderLayout, ResourceContext,
        images::ImageBridge, reader,
    },
};
use nsh_core::prelude::*;
use std::sync::Arc;
pub const README: &str = r#"# 中文 README

<!-- secret-hidden -->
<div><p>原生 <strong>HTML</strong> 与中文、emoji 🌍。</p></div>

第一段可跨块选择。

第二段含有 [相对链接](../guide.md#目标) 和 [锚点](#文末)。

| 表头 | 值 |
| --- | --- |
| 中文 | `code` |

```rust
fn main() { println!("中文"); }
```

![拒绝图片](file:///C:/private.png)

![相对图片](images/chart.png)

## 文末

最后一段。
"#;
pub fn context() -> ResourceContext {
    ResourceContext::new(
        "octo",
        "reader",
        "1111111111111111111111111111111111111111",
        "docs/README.md",
    )
}
pub fn document(text: &str) -> Arc<ReaderDocument> {
    ReaderDocument::prepare(
        context(),
        PreparedBody::Utf8 {
            text: text.into(),
            has_bom: false,
        },
        DocumentKind::Markdown,
    )
}
pub fn bridge() -> ImageBridge {
    ImageBridge::new(None, Arc::new(|| true), ImageLimits::default())
}
#[component]
pub fn Preview() -> impl IntoElement {
    let action = signal(String::from("Ready"));
    let handle = ReaderHandle::new(
        document(README),
        bridge(),
        ReaderLayout::Repository,
        move |event| action.set(format!("{event:?}")),
    );
    let poll = handle.clone();
    let end = handle.clone();
    let source = handle.clone();
    let reset = handle.clone();
    let retire = handle.clone();
    // Fixture only. The real host supplies the existing runtime's UI drain.
    let pump = handle.clone();
    spawn(async move {
        loop {
            timer(std::time::Duration::from_millis(30)).await;
            pump.poll();
        }
    });
    view()
        .flex_col()
        .size_full()
        .child(
            view()
                .flex_row()
                .gap(px(12.0))
                .child(text("Poll").on_press(move || {
                    poll.poll();
                }))
                .child(text("End").on_press(move || end.scroll_to_end()))
                .child(text("Source").on_press(move || {
                    source.replace(
                        ReaderDocument::prepare(
                            context(),
                            PreparedBody::Utf8 {
                                text: "fn main() {\n    println!(\"只读中文\");\n}\n".into(),
                                has_bom: false,
                            },
                            DocumentKind::Source {
                                language: Some("rust".into()),
                            },
                        ),
                        bridge(),
                    )
                }))
                .child(text("Reset").on_press(move || reset.replace(document(README), bridge())))
                .child(text("Retire").on_press(move || retire.retire())),
        )
        .child(view().flex_1().min_h(px(0.0)).child(reader(handle)))
        .child(text(move || action.get()).text_size(px(11.0)))
}
