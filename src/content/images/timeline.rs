//! A finite, script-free CSS timeline evaluated before resvg rasterization.
//! Unsupported animation values leave the underlying presentation intact.
use resvg::usvg::roxmltree::{Document, Node};
use simplecss::{AttributeOperator, Element, PseudoClass};
use std::collections::BTreeMap;

const MAX_TRACKS: usize = 128;
pub(super) const MAX_DURATION_MS: f64 = 5_000.0;
type Properties = BTreeMap<String, String>;

#[derive(Clone, Copy)]
struct CssNode<'a, 'input>(Node<'a, 'input>);
impl Element for CssNode<'_, '_> {
    fn parent_element(&self) -> Option<Self> {
        self.0.parent_element().map(Self)
    }
    fn prev_sibling_element(&self) -> Option<Self> {
        self.0.prev_sibling_element().map(Self)
    }
    fn has_local_name(&self, name: &str) -> bool {
        if name == "__githux_root" {
            self.0.parent_element().is_none()
        } else {
            self.0.tag_name().name() == name
        }
    }
    fn attribute_matches(&self, name: &str, op: AttributeOperator<'_>) -> bool {
        self.0.attribute(name).is_some_and(|v| op.matches(v))
    }
    fn pseudo_class_matches(&self, class: PseudoClass<'_>) -> bool {
        matches!(class, PseudoClass::FirstChild) && self.0.prev_sibling_element().is_none()
    }
}

#[derive(Clone)]
struct Track {
    delay: f64,
    duration: f64,
    fill: String,
    easing: String,
    stops: Vec<(f64, Properties)>,
}
struct NodeStyle {
    at: usize,
    remove: Vec<std::ops::Range<usize>>,
    properties: Properties,
    tracks: Vec<Track>,
}
pub(super) struct Timeline {
    nodes: Vec<NodeStyle>,
    styles: Vec<std::ops::Range<usize>>,
    pub duration_ms: f64,
}

/// Split top-level CSS blocks, respecting strings, comments and nested braces.
fn blocks(css: &str) -> Vec<(&str, &str)> {
    let b = css.as_bytes();
    let mut out = Vec::new();
    let mut start = 0;
    let mut open = 0;
    let mut depth = 0;
    let mut quote = 0;
    let mut i = 0;
    while i < b.len() {
        if quote != 0 {
            if b[i] == b'\\' {
                i += 2;
                continue;
            }
            if b[i] == quote {
                quote = 0;
            }
        } else if b[i..].starts_with(b"/*") {
            i += 2;
            while i + 1 < b.len() && !b[i..].starts_with(b"*/") {
                i += 1;
            }
            i += 2;
            continue;
        } else if b[i] == b'\'' || b[i] == b'"' {
            quote = b[i];
        } else if b[i] == b'{' {
            if depth == 0 {
                open = i;
            }
            depth += 1;
        } else if b[i] == b'}' && depth > 0 {
            depth -= 1;
            if depth == 0 {
                out.push((css[start..open].trim(), &css[open + 1..i]));
                start = i + 1;
            }
        }
        i += 1;
    }
    out
}
struct Declaration {
    name: String,
    value: String,
    important: bool,
}
fn split_css(css: &str, delimiter: u8) -> Vec<&str> {
    let bytes = css.as_bytes();
    let mut out = Vec::new();
    let mut start = 0;
    let mut quote = 0;
    let mut depth = 0usize;
    let mut i = 0;
    while i < bytes.len() {
        let byte = bytes[i];
        if quote != 0 {
            if byte == b'\\' {
                i += 2;
                continue;
            }
            if byte == quote {
                quote = 0;
            }
        } else {
            match byte {
                b'\'' | b'"' => quote = byte,
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => depth = depth.saturating_sub(1),
                _ => {}
            }
            if byte == delimiter && depth == 0 {
                out.push(&css[start..i]);
                start = i + 1;
            }
        }
        i += 1;
    }
    out.push(&css[start..]);
    out
}
fn css_declarations(css: &str) -> Vec<Declaration> {
    split_css(css, b';')
        .into_iter()
        .filter_map(|decl| {
            let (name, value) = decl.split_once(':')?;
            let name = name.trim();
            if name.is_empty() {
                return None;
            }
            let mut value = value.trim();
            let important = value
                .rsplit_once('!')
                .is_some_and(|(_, tail)| tail.trim().eq_ignore_ascii_case("important"));
            if important {
                value = value.rsplit_once('!')?.0.trim();
            }
            Some(Declaration {
                name: if name.starts_with("--") {
                    name.to_string()
                } else if name.eq_ignore_ascii_case("background") {
                    "background-color".to_string()
                } else {
                    name.to_ascii_lowercase()
                },
                value: value.to_string(),
                important,
            })
        })
        .collect()
}
/// usvg converts the outer SVG's children directly, omitting its non-inherited
/// opacity/display. Apply those to the completed surface, including its paper.
pub(super) fn root_opacity(doc: &Document<'_>) -> f32 {
    let root = doc.root_element();
    let props = declarations(root.attribute("style").unwrap_or_default());
    if props
        .get("display")
        .map(String::as_str)
        .or_else(|| root.attribute("display"))
        == Some("none")
    {
        return 0.0;
    }
    let value = props
        .get("opacity")
        .map(String::as_str)
        .or_else(|| root.attribute("opacity"))
        .unwrap_or("1");
    let opacity = if let Some(value) = value.strip_suffix('%') {
        value.parse::<f32>().ok().map(|v| v / 100.0)
    } else {
        value.parse::<f32>().ok()
    };
    opacity
        .filter(|v| v.is_finite())
        .unwrap_or(1.0)
        .clamp(0.0, 1.0)
}

fn declarations(css: &str) -> Properties {
    css_declarations(css)
        .into_iter()
        .filter(|d| !d.important)
        .map(|d| (d.name, d.value))
        .collect()
}
fn milliseconds(s: &str) -> Option<f64> {
    let v = if let Some(n) = s.strip_suffix("ms") {
        n.parse().ok()?
    } else {
        s.strip_suffix('s')?.parse::<f64>().ok()? * 1000.0
    };
    v.is_finite().then_some(v)
}
fn resolve_vars(value: &str, vars: &Properties, depth: usize) -> Option<String> {
    if depth > 16 {
        return None;
    }
    let Some(start) = value.find("var(") else {
        return Some(value.to_string());
    };
    let mut level = 1;
    let mut end = start + 4;
    for (i, c) in value[start + 4..].char_indices() {
        match c {
            '(' => level += 1,
            ')' => level -= 1,
            _ => {}
        }
        if level == 0 {
            end = start + 4 + i;
            break;
        }
    }
    if level != 0 {
        return None;
    }
    let (name, fallback) = value[start + 4..end]
        .split_once(',')
        .map_or((&value[start + 4..end], None), |(a, b)| (a, Some(b)));
    let replacement = vars.get(name.trim()).map(String::as_str).or(fallback)?;
    let replacement = resolve_vars(replacement, vars, depth + 1)?;
    resolve_vars(
        &format!("{}{}{}", &value[..start], replacement, &value[end + 1..]),
        vars,
        depth + 1,
    )
}
fn strip_comments(css: &str) -> String {
    let b = css.as_bytes();
    let mut out = String::new();
    let mut start = 0;
    let mut i = 0;
    let mut quote = 0;
    while i < b.len() {
        if quote != 0 {
            if b[i] == b'\\' {
                i += 2;
                continue;
            }
            if b[i] == quote {
                quote = 0;
            }
        } else if matches!(b[i], b'\'' | b'"') {
            quote = b[i];
        } else if b[i..].starts_with(b"/*") {
            out.push_str(&css[start..i]);
            i += 2;
            while i + 1 < b.len() && !b[i..].starts_with(b"*/") {
                i += 1;
            }
            i = (i + 2).min(b.len());
            start = i;
            out.push(' ');
            continue;
        }
        i += 1;
    }
    out.push_str(&css[start..]);
    out
}
impl Timeline {
    pub fn new(doc: &Document<'_>) -> Result<Self, super::ImageFailure> {
        let all_css = strip_comments(
            &doc.descendants()
                .filter(|n| n.has_tag_name("style"))
                .filter_map(|n| n.text())
                .collect::<Vec<_>>()
                .join("\n"),
        );
        // Web-font payloads and conditional/browser-specific rules are not
        // evaluated. In particular an ignored WOFF data URL must not consume
        // the small budget for the declarations we actually evaluate.
        let mut css = String::new();
        for (header, body) in blocks(&all_css) {
            if !header.starts_with('@') || header.starts_with("@keyframes ") {
                if css
                    .len()
                    .saturating_add(header.len())
                    .saturating_add(body.len())
                    > 128 * 1024
                {
                    return Err(super::ImageFailure::TooLarge { limit: 128 * 1024 });
                }
                css.push_str(header);
                css.push('{');
                css.push_str(body);
                css.push('}');
            }
        }
        if css.len() > 128 * 1024 {
            return Err(super::ImageFailure::TooLarge { limit: 128 * 1024 });
        }
        let mut key_bytes = 0usize;
        let mut keys = BTreeMap::new();
        for (header, body) in blocks(&css) {
            if let Some(name) = header.strip_prefix("@keyframes ") {
                let mut stops = Vec::new();
                for (positions, body) in blocks(body) {
                    for pos in positions.split(',') {
                        let pos = match pos.trim() {
                            "from" => Some(0.0),
                            "to" => Some(1.0),
                            p => p
                                .strip_suffix('%')
                                .and_then(|v| v.parse::<f64>().ok())
                                .map(|v| v / 100.0),
                        };
                        if let Some(pos) = pos.filter(|v| v.is_finite() && (0.0..=1.0).contains(v))
                        {
                            key_bytes = key_bytes.saturating_add(body.len());
                            if key_bytes > 2 * 1024 * 1024 || stops.len() >= 256 {
                                return Err(super::ImageFailure::TooLarge {
                                    limit: 2 * 1024 * 1024,
                                });
                            }
                            stops.push((pos, declarations(body)));
                        }
                    }
                }
                stops.sort_by(|a, b| a.0.total_cmp(&b.0));
                keys.insert(name.trim().to_string(), stops);
            }
        }
        let css = css.replace(":root", "__githux_root");
        let mut rules = Vec::new();
        let mut rule_bytes = 0usize;
        for (selectors, body) in blocks(&css) {
            if selectors.starts_with('@') {
                continue;
            }
            for selector in split_css(selectors, b',') {
                if let Some(selector) = simplecss::Selector::parse(selector.trim()) {
                    rule_bytes = rule_bytes.saturating_add(body.len());
                    if rule_bytes > 2 * 1024 * 1024 {
                        return Err(super::ImageFailure::TooLarge {
                            limit: 2 * 1024 * 1024,
                        });
                    }
                    rules.push((selector, css_declarations(body)));
                }
            }
        }
        rules.sort_by_key(|(selector, _)| selector.specificity());
        if rules
            .len()
            .saturating_mul(doc.descendants().filter(Node::is_element).count())
            > 2_000_000
        {
            return Err(super::ImageFailure::TooLarge { limit: 2_000_000 });
        }
        let mut expanded_bytes = 0usize;
        let mut inherited = std::collections::HashMap::<_, Properties>::new();
        let mut nodes = Vec::new();
        let mut duration_ms: f64 = 0.0;
        let mut tracks_count = 0;
        for node in doc.descendants().filter(Node::is_element) {
            if node.ancestors().any(|n| n.has_tag_name("style")) {
                continue;
            }
            let mut props = Properties::new();
            let mut important = Properties::new();
            for attr in node.attributes() {
                if attr.name() != "style" && attr.value().contains("var(") {
                    props.insert(attr.name().to_string(), attr.value().to_string());
                }
            }
            for (selector, declarations) in &rules {
                if selector.matches(&CssNode(node)) {
                    for d in declarations {
                        if d.important {
                            important.insert(d.name.to_string(), d.value.to_string());
                        } else {
                            props.insert(d.name.to_string(), d.value.to_string());
                        }
                    }
                }
            }
            for d in css_declarations(&strip_comments(node.attribute("style").unwrap_or_default()))
            {
                if d.important {
                    important.insert(d.name.to_string(), d.value.to_string());
                } else {
                    props.insert(d.name.to_string(), d.value.to_string());
                }
            }
            props.extend(important.clone());
            let mut vars = node
                .parent_element()
                .and_then(|p| inherited.get(&p.id()))
                .cloned()
                .unwrap_or_default();
            vars.extend(
                props
                    .iter()
                    .filter(|(k, _)| k.starts_with("--"))
                    .map(|(k, v)| (k.clone(), v.clone())),
            );
            expanded_bytes = expanded_bytes
                .saturating_add(vars.iter().map(|(k, v)| k.len() + v.len()).sum::<usize>());
            if expanded_bytes > 2 * 1024 * 1024 {
                return Err(super::ImageFailure::TooLarge {
                    limit: 2 * 1024 * 1024,
                });
            }
            inherited.insert(node.id(), vars.clone());
            props = props
                .into_iter()
                .filter(|(k, _)| !k.starts_with("--"))
                .filter_map(|(k, v)| resolve_vars(&v, &vars, 0).map(|v| (k, v)))
                .collect();
            // The SVG root's background is a box paint, not a visible child
            // that may override inherited visibility.
            if node.parent_element().is_none()
                && matches!(
                    props
                        .get("visibility")
                        .map(String::as_str)
                        .or_else(|| node.attribute("visibility")),
                    Some("hidden" | "collapse")
                )
            {
                props.insert("background-color".into(), "transparent!important".into());
            }
            let mut tracks = Vec::new();
            let animation = props
                .get("animation")
                .or_else(|| props.get("animation-name"));
            if tracks_count < MAX_TRACKS
                && let Some(shorthand) = animation
            {
                // Finite, single-iteration entry animations. Unsupported iteration/direction
                // and multiple tracks retain their static underlying presentation.
                let tokens = shorthand.split_whitespace().collect::<Vec<_>>();
                let name = tokens.iter().find(|v| keys.contains_key(**v));
                let times = tokens
                    .iter()
                    .filter_map(|v| milliseconds(v))
                    .collect::<Vec<_>>();
                let iterations_ok = props
                    .get("animation-iteration-count")
                    .is_none_or(|v| v == "1")
                    && props
                        .get("animation-direction")
                        .is_none_or(|v| v == "normal")
                    && tokens
                        .iter()
                        .all(|v| v.parse::<f64>().is_err() || *v == "1");
                if !shorthand.contains(',')
                    && !tokens.contains(&"infinite")
                    && iterations_ok
                    && let Some(name) = name
                {
                    let duration = props
                        .get("animation-duration")
                        .and_then(|v| milliseconds(v))
                        .or(times.first().copied())
                        .unwrap_or(0.0);
                    let delay = props
                        .get("animation-delay")
                        .and_then(|v| milliseconds(v))
                        .or(times.get(1).copied())
                        .unwrap_or(0.0);
                    if duration >= 0.0 && (duration + delay).is_finite() {
                        let fill = props
                            .get("animation-fill-mode")
                            .cloned()
                            .unwrap_or_else(|| {
                                tokens
                                    .iter()
                                    .find(|v| matches!(**v, "forwards" | "backwards" | "both"))
                                    .unwrap_or(&"none")
                                    .to_string()
                            });
                        let easing = props
                            .get("animation-timing-function")
                            .cloned()
                            .unwrap_or_else(|| {
                                tokens
                                    .iter()
                                    .find(|v| {
                                        matches!(
                                            **v,
                                            "linear"
                                                | "ease"
                                                | "ease-in"
                                                | "ease-out"
                                                | "ease-in-out"
                                        )
                                    })
                                    .unwrap_or(&"ease")
                                    .to_string()
                            });
                        let mut stops = keys[*name].clone();
                        for (_, values) in &mut stops {
                            values.retain(|key, _| !important.contains_key(key));
                        }
                        if stops.len() >= 2 {
                            tracks.push(Track {
                                delay,
                                duration,
                                fill,
                                easing,
                                stops,
                            });
                            tracks_count += 1;
                            duration_ms = duration_ms.max(duration + delay);
                        }
                    }
                }
            }
            props.retain(|k, _| !k.starts_with("animation"));
            expanded_bytes = expanded_bytes
                .saturating_add(props.iter().map(|(k, v)| k.len() + v.len()).sum::<usize>())
                .saturating_add(
                    tracks
                        .iter()
                        .flat_map(|t| &t.stops)
                        .flat_map(|(_, p)| p.iter())
                        .map(|(k, v)| k.len() + v.len())
                        .sum::<usize>(),
                );
            if expanded_bytes > 2 * 1024 * 1024 {
                return Err(super::ImageFailure::TooLarge {
                    limit: 2 * 1024 * 1024,
                });
            }
            if !props.is_empty() || !tracks.is_empty() {
                let remove = node
                    .attributes()
                    .filter(|a| {
                        a.name() == "style"
                            || (a.name() == "transform"
                                && (props.contains_key("transform")
                                    || tracks.iter().any(|t| {
                                        t.stops.iter().any(|(_, s)| s.contains_key("transform"))
                                    })))
                    })
                    .map(|a| a.range())
                    .collect();
                nodes.push(NodeStyle {
                    at: node.range().start
                        + 1
                        + doc.input_text().as_bytes()[node.range().start + 1..]
                            .iter()
                            .position(|b| b.is_ascii_whitespace() || matches!(b, b'>' | b'/'))
                            .unwrap_or(node.tag_name().name().len()),
                    remove,
                    properties: props,
                    tracks,
                });
            }
        }
        let styles = doc
            .descendants()
            .filter(|n| {
                n.has_tag_name("style") && !n.ancestors().skip(1).any(|p| p.has_tag_name("style"))
            })
            .map(|n| n.range())
            .collect();
        Ok(Self {
            nodes,
            styles,
            duration_ms,
        })
    }
    pub fn sample(&self, xml: &str, time_ms: f64) -> String {
        // All supported declarations have already been cascaded and are inline.
        // Keeping the original sheets would let usvg recascade !important with
        // its different inline priority and undo sampled/visibility values.
        let mut edits = self
            .styles
            .iter()
            .map(|range| (range.clone(), String::new()))
            .collect::<Vec<_>>();
        for node in &self.nodes {
            let mut props = node.properties.clone();
            for track in &node.tracks {
                let elapsed = time_ms - track.delay;
                if (elapsed < 0.0 && !matches!(track.fill.as_str(), "backwards" | "both"))
                    || (elapsed >= track.duration
                        && !matches!(track.fill.as_str(), "forwards" | "both"))
                {
                    continue;
                }
                let progress = if track.duration == 0.0 {
                    if elapsed < 0.0 { 0.0 } else { 1.0 }
                } else {
                    (elapsed / track.duration).clamp(0.0, 1.0)
                };
                let pair = track
                    .stops
                    .windows(2)
                    .find(|s| progress <= s[1].0)
                    .unwrap_or(&track.stops[track.stops.len() - 2..]);
                let p = ((progress - pair[0].0) / (pair[1].0 - pair[0].0)).clamp(0.0, 1.0);
                let p = ease(p, &track.easing);
                for (key, a) in &pair[0].1 {
                    if let Some(b) = pair[1].1.get(key)
                        && let Some(value) = interpolate(key, a, b, p)
                    {
                        props.insert(key.clone(), value);
                    }
                }
            }
            let transform = props.remove("transform").map(|value| {
                let origin = props.remove("transform-origin").unwrap_or_default();
                let origin = origin.replace("px", "");
                let parts = origin.split_whitespace().collect::<Vec<_>>();
                let value = value.replace("px", "").replace("deg", "");
                if parts.len() == 2
                    && let (Ok(x), Ok(y)) = (parts[0].parse::<f64>(), parts[1].parse::<f64>())
                {
                    format!("translate({x} {y}) {value} translate({} {})", -x, -y)
                } else {
                    value
                }
            });
            let style = props
                .iter()
                .map(|(k, v)| format!("{k}:{v};"))
                .collect::<String>();
            let mut attrs = format!(" style=\"{}\"", escape(&style));
            if let Some(transform) = transform {
                attrs.push_str(&format!(" transform=\"{}\"", escape(&transform)));
            }
            edits.push((node.at..node.at, attrs));
            for range in &node.remove {
                edits.push((range.clone(), String::new()));
            }
        }
        edits.sort_by_key(|(range, _)| range.start);
        let mut result = String::with_capacity(xml.len());
        let mut cursor = 0;
        for (range, text) in edits {
            result.push_str(&xml[cursor..range.start]);
            result.push_str(&text);
            cursor = range.end;
        }
        result.push_str(&xml[cursor..]);
        result
    }
}
fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
}
fn interpolate(key: &str, a: &str, b: &str, p: f64) -> Option<String> {
    if key == "transform" {
        let parse = |s: &str| {
            s.replace("px", "")
                .replace("deg", "")
                .parse::<svgtypes::Transform>()
                .ok()
        };
        let a = parse(a)?;
        let b = parse(b)?;
        let mix = |a, b| a + (b - a) * p;
        return Some(format!(
            "matrix({} {} {} {} {} {})",
            mix(a.a, b.a),
            mix(a.b, b.b),
            mix(a.c, b.c),
            mix(a.d, b.d),
            mix(a.e, b.e),
            mix(a.f, b.f)
        ));
    }
    if !matches!(
        key,
        "opacity" | "fill-opacity" | "stroke-opacity" | "stroke-dashoffset"
    ) {
        return None;
    }
    let a = a.parse::<f64>().ok()?;
    let b = b.parse::<f64>().ok()?;
    (a.is_finite() && b.is_finite()).then(|| (a + (b - a) * p).to_string())
}
fn ease(x: f64, name: &str) -> f64 {
    let (x1, y1, x2, y2) = match name {
        "ease-in-out" => (0.42, 0.0, 0.58, 1.0),
        "ease-in" => (0.42, 0.0, 1.0, 1.0),
        "ease-out" => (0.0, 0.0, 0.58, 1.0),
        "linear" => return x,
        _ => (0.25, 0.1, 0.25, 1.0),
    };
    let curve = |t: f64, a: f64, b: f64| {
        3.0 * (1.0 - t).powi(2) * t * a + 3.0 * (1.0 - t) * t * t * b + t * t * t
    };
    let (mut low, mut high) = (0.0, 1.0);
    for _ in 0..24 {
        let t = (low + high) / 2.0;
        if curve(t, x1, x2) < x {
            low = t;
        } else {
            high = t;
        }
    }
    curve((low + high) / 2.0, y1, y2)
}
