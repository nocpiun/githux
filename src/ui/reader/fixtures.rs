//! Deterministic reader fixtures for the preview and the tests.
//!
//! These are acceptance material only: they never claim that the main window
//! has real repository data wired in. The text covers the Markdown minimum
//! (headings, paragraphs, lists, quotes, links, fenced code, tables), Chinese
//! and emoji, long lines, relative resources, a dangerous scheme and the
//! empty/unsupported/too-large states.

use crate::content::{PreparedBody, UnsupportedContent};

pub const OWNER: &str = "nocpiun";
pub const NAME: &str = "githux";
pub const COMMIT: &str = "42291f79bbe73188852bb375fd296623885ce9b8";
pub const PATH: &str = "docs/guide.md";

/// The Markdown acceptance fixture.
pub const MARKDOWN: &str = r##"# Githux reader 阅读器

This fixture exercises the reader's Markdown coverage: **strong**, *emphasis*,
`inline code`, ~~strikethrough~~, a [relative link](guide.md#setup), an
[absolute-in-repo link](/src/main.rs) and an [external link](https://example.com).

## Lists

- first item with 中文和 emoji 🚀
- second item
  - nested item

1. ordered one
2. ordered two

> A quote that keeps the reader honest.
> It spans two lines.

## Code

```rust
fn main() {
    let greeting = "你好，世界 🌍";
    println!("{greeting}");
}
```

## Table

| Column | Meaning |
| --- | --- |
| Empty | a successful zero-byte file |
| Unsupported | non-UTF-8 or binary bytes |
| TooLarge | past the 2 MiB budget |

## Resources

![relative picture](images/diagram.png)

![web picture](https://example.com/diagram.png)

<script>alert("inert text")</script>

<a href="javascript:alert(1)">a dangerous link</a>
"##;

/// The first block is one link, so a press can be aimed at it.
pub const LINK_ONLY: &str = "[press me](guide.md#setup)\n";

/// The first block is one dangerous link.
pub const DANGEROUS_LINK_ONLY: &str = "[do not run](javascript:alert(1))\n";

/// One short inline-code span, for the padding/background layout check.
///
/// Exactly one code span, so `reader-inline-code` names one element.
pub fn inline_code_short() -> PreparedBody {
    PreparedBody::Utf8 {
        text: "# Inline code\n\nA short `x` span.\n".to_string(),
        has_bom: false,
    }
}

/// A long inline-code span that must wrap in a narrow window, with Chinese and
/// emoji around it. Exactly one code span, so `reader-inline-code` is
/// unambiguous.
pub fn inline_code_long() -> PreparedBody {
    let long = "code_".repeat(60);
    PreparedBody::Utf8 {
        text: format!(
            "# Inline code\n\n中文和 emoji 🚀 在长代码 `{long}` 之间。\n\n普通正文 `短代码` 连续代码：`first` `second`，以及 emoji `🚀中文` 与正文交错。\n"
        ),
        has_bom: false,
    }
}

/// The source acceptance fixture: Chinese, emoji and one very long line.
pub const SOURCE: &str = r#"// Githux reader source fixture
use std::collections::HashMap;

/// 一个很长的行，用来验证横向滚动不会把内容裁掉：aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
pub fn greet(name: &str) -> String {
    let emoji = "🌍";
    format!("你好，{name} {emoji}")
}

fn unused() -> HashMap<&'static str, usize> {
    HashMap::new()
}
"#;

pub fn markdown() -> PreparedBody {
    PreparedBody::Utf8 {
        text: MARKDOWN.to_string(),
        has_bom: false,
    }
}

/// The raw-HTML acceptance fixture: a README-like document that mixes Markdown
/// with the HTML users actually write (centred `div`, commented-out markup,
/// `img`, `details`, `table`, lists, inline formatting) and shows that HTML
/// inside code stays verbatim.
pub const HTML: &str = r##"# HTML 混排

<div align="center">
  <img src="https://example.com/logo.png" alt="项目徽标">
  <!-- 下面是已废弃的旧表格：
  <table>
    <tr><td>hidden cell</td></tr>
  </table>
  -->
</div>

普通段落，带 <strong>加粗</strong>、<em>斜体</em>、<code>inline &lt;tag&gt;</code> 与 <a href="guide.md#setup">相对链接</a>。

<details>
<summary>展开说明</summary>
<p>内部段落与 <kbd>Ctrl</kbd> 键。</p>
</details>

<table align="center">
  <thead>
    <tr><th align="left">列</th><th align="right">含义</th></tr>
  </thead>
  <tbody>
    <tr><td>空文件</td><td align="right">0 字节</td></tr>
    <tr><td>二进制</td><td align="right">非 UTF-8</td></tr>
  </tbody>
</table>

列表：

<ul>
  <li>第一项</li>
  <li>第二项<br>换行</li>
</ul>

行内代码里的 HTML 必须原样显示：`<div align="center">` 与 ``<span>x</span>``。

```html
<!-- 代码块中的注释保留 -->
<div align="center">raw</div>
```

<script>alert("never runs")</script>

<!-- 单行注释不进入正文 -->
尾部文本。
"##;

pub fn html() -> PreparedBody {
    PreparedBody::Utf8 {
        text: HTML.to_string(),
        has_bom: false,
    }
}

pub fn link_only() -> PreparedBody {
    PreparedBody::Utf8 {
        text: LINK_ONLY.to_string(),
        has_bom: false,
    }
}

pub fn dangerous_link_only() -> PreparedBody {
    PreparedBody::Utf8 {
        text: DANGEROUS_LINK_ONLY.to_string(),
        has_bom: false,
    }
}

pub fn source() -> PreparedBody {
    PreparedBody::Utf8 {
        text: SOURCE.to_string(),
        has_bom: false,
    }
}

pub fn empty() -> PreparedBody {
    PreparedBody::Empty
}

pub fn unsupported() -> PreparedBody {
    PreparedBody::Unsupported(UnsupportedContent::NonUtf8)
}

pub fn too_large() -> PreparedBody {
    PreparedBody::TooLarge {
        size: 3 * 1024 * 1024,
        limit: 2 * 1024 * 1024,
    }
}

/// A source file just under the 2 MiB engineering limit.
pub fn large_source() -> PreparedBody {
    let mut text = String::with_capacity(2 * 1024 * 1024);
    let mut line = 0usize;
    while text.len() < 2 * 1024 * 1024 - 128 {
        text.push_str(&format!(
            "fn generated_{line}() -> usize {{ {line} }} // 填充 padding padding padding padding padding padding padding\n"
        ));
        line += 1;
    }
    PreparedBody::Utf8 {
        text,
        has_bom: false,
    }
}

/// A Markdown file just under the 2 MiB engineering limit.
pub fn large_markdown() -> PreparedBody {
    let mut text = String::with_capacity(2 * 1024 * 1024);
    let mut section = 0usize;
    while text.len() < 2 * 1024 * 1024 - 256 {
        text.push_str(&format!(
            "## Section {section}\n\nA paragraph with 中文 and emoji 🚀 that pads the document so the reader has to virtualize it. `code` and [a link](guide.md).\n\n"
        ));
        section += 1;
    }
    PreparedBody::Utf8 {
        text,
        has_bom: false,
    }
}
