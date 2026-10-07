//! Confluence's storage format (XHTML with `ac:` and `ri:` elements, CDATA
//! and HTML entities) as a tree that remembers where each element sits in
//! the source.
//!
//! ado's tokenizer streams HTML and keeps no positions, names or CDATA, so
//! storage gets its own: section edits splice the source by those positions
//! and never re-serialize what they keep (a generic HTML library reorders
//! attributes, expands `<ri:page/>` and decodes `&nbsp;`, which other tools
//! shipped as noisy diffs). Parsing is lenient, as Confluence's is: an
//! unclosed element ends with its parent, a stray end tag is dropped, and a
//! `<` that starts no tag is text.

use std::ops::Range;

use agent_cli_core::Failure;
use anyhow::Result;

#[derive(Debug)]
pub(crate) enum Node {
    /// Text with its entities decoded.
    Text(String),
    /// A CDATA section's text, as written (code macro bodies).
    Cdata(String),
    Element(Element),
}

#[derive(Debug)]
pub(crate) struct Element {
    /// Lowercase, prefix included: `p`, `ac:structured-macro`, `ri:page`.
    pub(crate) name: String,
    attrs: Vec<(String, String)>,
    pub(crate) children: Vec<Node>,
    /// From its start tag's `<` to its end tag's `>`.
    pub(crate) span: Range<usize>,
    /// Between its start tag and its end tag.
    pub(crate) inner: Range<usize>,
}

impl Element {
    /// An attribute's decoded value, by its name in any case.
    pub(crate) fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    pub(crate) fn elements(&self) -> impl Iterator<Item = &Element> {
        self.children.iter().filter_map(|node| match node {
            Node::Element(element) => Some(element),
            _ => None,
        })
    }

    pub(crate) fn child(&self, name: &str) -> Option<&Element> {
        self.elements().find(|element| element.name == name)
    }

    /// A macro's `ac:parameter` of that name (`""` is the unnamed one), as text.
    pub(crate) fn param(&self, name: &str) -> Option<String> {
        self.elements()
            .find(|element| element.name == "ac:parameter" && element.attr("ac:name") == Some(name))
            .map(|element| text_of(&element.children))
    }

    pub(crate) fn text(&self) -> String {
        text_of(&self.children)
    }
}

/// Every text and CDATA node under `nodes`, in order.
pub(crate) fn text_of(nodes: &[Node]) -> String {
    let mut out = String::new();
    for node in nodes {
        match node {
            Node::Text(text) | Node::Cdata(text) => out.push_str(text),
            Node::Element(element) => out.push_str(&text_of(&element.children)),
        }
    }
    out
}

/// Elements that never hold content, closed or not.
const VOID: &[&str] = &[
    "br", "hr", "img", "col", "input", "meta", "link", "wbr", "area", "base", "source",
];

pub(crate) fn parse(source: &str) -> Vec<Node> {
    let mut root = Vec::new();
    let mut open: Vec<Element> = Vec::new();
    let mut at = 0;
    while let Some(offset) = source[at..].find('<') {
        let start = at + offset;
        push_text(&mut root, &mut open, &source[at..start]);
        let rest = &source[start..];
        if let Some(cdata) = rest.strip_prefix("<![CDATA[") {
            let end = cdata.find("]]>").unwrap_or(cdata.len());
            push(&mut root, &mut open, Node::Cdata(cdata[..end].to_owned()));
            at = (start + 9 + end + 3).min(source.len());
        } else if let Some(comment) = rest.strip_prefix("<!--") {
            at = comment
                .find("-->")
                .map_or(source.len(), |end| start + 4 + end + 3);
        } else if rest.starts_with("<!") || rest.starts_with("<?") {
            at = rest.find('>').map_or(source.len(), |end| start + end + 1);
        } else if let Some(name) = rest.strip_prefix("</") {
            let Some(end) = name.find('>') else {
                push_text(&mut root, &mut open, rest);
                at = source.len();
                break;
            };
            let name = name[..end].trim().to_ascii_lowercase();
            at = start + 2 + end + 1;
            if let Some(depth) = open.iter().rposition(|element| element.name == name) {
                while open.len() > depth + 1 {
                    close(&mut root, &mut open, start, start);
                }
                close(&mut root, &mut open, start, at);
            }
        } else if let Some((element, end, closed)) = start_tag(source, start) {
            at = end;
            if closed || VOID.contains(&element.name.as_str()) {
                push(&mut root, &mut open, Node::Element(element));
            } else {
                open.push(element);
            }
        } else {
            push_text(&mut root, &mut open, "<");
            at = start + 1;
        }
    }
    push_text(&mut root, &mut open, &source[at..]);
    while !open.is_empty() {
        close(&mut root, &mut open, source.len(), source.len());
    }
    root
}

fn push(root: &mut Vec<Node>, open: &mut [Element], node: Node) {
    match open.last_mut() {
        Some(parent) => parent.children.push(node),
        None => root.push(node),
    }
}

fn push_text(root: &mut Vec<Node>, open: &mut [Element], raw: &str) {
    if !raw.is_empty() {
        push(root, open, Node::Text(decode_entities(raw)));
    }
}

/// Closes the innermost open element: its content ends at `inner_end`, the
/// element at `end`.
fn close(root: &mut Vec<Node>, open: &mut Vec<Element>, inner_end: usize, end: usize) {
    if let Some(mut element) = open.pop() {
        element.inner.end = inner_end;
        element.span.end = end;
        push(root, open, Node::Element(element));
    }
}

/// The start tag at `start`: the element (no children yet), where the tag
/// ends, and whether it closed itself. `None` when no tag name follows `<`.
fn start_tag(source: &str, start: usize) -> Option<(Element, usize, bool)> {
    let tag = &source[start + 1..];
    if !tag.starts_with(|c: char| c.is_ascii_alphabetic()) {
        return None;
    }
    let name_end = tag
        .find(|c: char| c.is_whitespace() || c == '/' || c == '>')
        .unwrap_or(tag.len());
    let name = tag[..name_end].to_ascii_lowercase();
    let mut attrs = Vec::new();
    let bytes = tag.as_bytes();
    let mut index = name_end;
    loop {
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        match bytes.get(index) {
            None => return None,
            Some(b'>') => {
                let end = start + 1 + index + 1;
                return Some((element(name, attrs, start, end), end, false));
            }
            Some(b'/') if bytes.get(index + 1) == Some(&b'>') => {
                let end = start + 1 + index + 2;
                return Some((element(name, attrs, start, end), end, true));
            }
            Some(b'/') => index += 1,
            Some(_) => {
                let key_end = tag[index..]
                    .find(|c: char| c.is_whitespace() || c == '=' || c == '>' || c == '/')
                    .map_or(tag.len(), |end| index + end);
                let key = tag[index..key_end].to_owned();
                index = key_end;
                while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                    index += 1;
                }
                let mut value = String::new();
                if bytes.get(index) == Some(&b'=') {
                    index += 1;
                    while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                        index += 1;
                    }
                    match bytes.get(index) {
                        Some(&quote @ (b'"' | b'\'')) => {
                            let close = tag[index + 1..].find(char::from(quote))?;
                            value = decode_entities(&tag[index + 1..index + 1 + close]);
                            index += close + 2;
                        }
                        _ => {
                            let end = tag[index..]
                                .find(|c: char| c.is_whitespace() || c == '>')
                                .map_or(tag.len(), |end| index + end);
                            value = decode_entities(&tag[index..end]);
                            index = end;
                        }
                    }
                }
                if key.is_empty() {
                    index += 1;
                } else {
                    attrs.push((key, value));
                }
            }
        }
    }
}

fn element(name: String, attrs: Vec<(String, String)>, start: usize, end: usize) -> Element {
    Element {
        name,
        attrs,
        children: Vec::new(),
        span: start..end,
        inner: end..end,
    }
}

/// The named entities storage bodies carry; an unknown name stays as written.
const NAMED_ENTITIES: &[(&str, char)] = &[
    ("nbsp", '\u{a0}'),
    ("amp", '&'),
    ("lt", '<'),
    ("gt", '>'),
    ("quot", '"'),
    ("apos", '\''),
    ("mdash", '—'),
    ("ndash", '–'),
    ("hellip", '…'),
    ("copy", '©'),
    ("reg", '®'),
    ("trade", '™'),
    ("rsquo", '’'),
    ("lsquo", '‘'),
    ("rdquo", '”'),
    ("ldquo", '“'),
    ("laquo", '«'),
    ("raquo", '»'),
    ("bull", '•'),
    ("middot", '·'),
    ("times", '×'),
    ("deg", '°'),
    ("euro", '€'),
    ("pound", '£'),
    ("rarr", '→'),
    ("larr", '←'),
    ("harr", '↔'),
    ("le", '≤'),
    ("ge", '≥'),
    ("ne", '≠'),
    ("plusmn", '±'),
    ("frac12", '½'),
    ("shy", '\u{ad}'),
    ("zwj", '\u{200d}'),
    ("zwnj", '\u{200c}'),
];

/// Decodes `&#8217;`, `&#x2014;` and the names in [`NAMED_ENTITIES`], each
/// once; a bare `&` or an unknown name is left as written.
pub(crate) fn decode_entities(raw: &str) -> String {
    if !raw.contains('&') {
        return raw.to_owned();
    }
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let decoded = after
            .find(';')
            .filter(|end| *end <= 12)
            .and_then(|end| entity(&after[..end]).map(|character| (end, character)));
        match decoded {
            Some((end, character)) => {
                out.push(character);
                rest = &after[end + 1..];
            }
            None => {
                out.push('&');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn entity(body: &str) -> Option<char> {
    if let Some(number) = body.strip_prefix('#') {
        let code = match number.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => number.parse().ok()?,
        };
        return char::from_u32(code);
    }
    NAMED_ENTITIES
        .iter()
        .find(|(name, _)| *name == body)
        .map(|(_, character)| *character)
}

/// Text as storage holds it: `&`, `<` and `>` escaped (and `"` for attributes).
pub(crate) fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

/// One heading an edit can address, and the source it rules.
#[derive(Debug)]
pub(crate) struct Section {
    pub(crate) heading: String,
    pub(crate) level: u8,
    /// From the heading's `<` to the next heading of its level or above in
    /// the same container, or the container's end.
    pub(crate) range: Range<usize>,
}

/// The sections a page's storage has: headings at the root, or directly in
/// a layout cell (each cell is a container of its own). A heading inside a
/// panel or an expand rules nothing, so an edit cannot cut a macro in two.
pub(crate) fn sections(source: &str, nodes: &[Node]) -> Vec<Section> {
    let mut found = Vec::new();
    collect(nodes, source.len(), &mut found);
    found.sort_by_key(|section| section.range.start);
    found
}

fn collect(nodes: &[Node], end: usize, found: &mut Vec<Section>) {
    let headings: Vec<(u8, &Element)> = nodes
        .iter()
        .filter_map(|node| match node {
            Node::Element(element) => level(&element.name).map(|level| (level, element)),
            _ => None,
        })
        .collect();
    for (at, (level, heading)) in headings.iter().enumerate() {
        let until = headings[at + 1..]
            .iter()
            .find(|(next, _)| next <= level)
            .map_or(end, |(_, next)| next.span.start);
        found.push(Section {
            heading: collapse(&heading.text()),
            level: *level,
            range: heading.span.start..until,
        });
    }
    for node in nodes {
        if let Node::Element(element) = node {
            match element.name.as_str() {
                "ac:layout-cell" => collect(&element.children, element.inner.end, found),
                "ac:layout" | "ac:layout-section" => collect_cells(element, found),
                _ => {}
            }
        }
    }
}

fn collect_cells(element: &Element, found: &mut Vec<Section>) {
    for child in element.elements() {
        match child.name.as_str() {
            "ac:layout-cell" => collect(&child.children, child.inner.end, found),
            "ac:layout-section" => collect_cells(child, found),
            _ => {}
        }
    }
}

pub(crate) fn level(name: &str) -> Option<u8> {
    match name.as_bytes() {
        [b'h', digit @ b'1'..=b'6'] => Some(digit - b'0'),
        _ => None,
    }
}

/// Whitespace runs as one space, trimmed: how a heading is matched.
pub(crate) fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The one section named `name` (its heading's text, any case, with the
/// Markdown an outline shows it in, `**` or `\_`, ignored). None is exit 4
/// naming the headings there are; two or more is exit 2 naming them.
pub(crate) fn find<'s>(sections: &'s [Section], name: &str, page: &str) -> Result<&'s Section> {
    let plain = |text: &str| collapse(&text.replace(['*', '`', '\\'], "")).to_lowercase();
    let wanted = plain(name);
    let matches: Vec<&Section> = sections
        .iter()
        .filter(|section| plain(&section.heading) == wanted)
        .collect();
    match matches[..] {
        [one] => Ok(one),
        [] => {
            let headings: Vec<&str> = sections
                .iter()
                .map(|section| section.heading.as_str())
                .collect();
            let there = if headings.is_empty() {
                "it has no headings at the top level".to_owned()
            } else {
                format!("its sections: {}", headings.join("; "))
            };
            Err(
                Failure::not_found(format!("page {page} has no section {name:?}; {there}"))
                    .hint(format!("agent-cli confluence page get {page}"))
                    .into(),
            )
        }
        _ => Err(Failure::usage(format!(
            "page {page} has {} sections named {name:?}, at levels {}",
            matches.len(),
            matches
                .iter()
                .map(|section| format!("h{}", section.level))
                .collect::<Vec<_>>()
                .join(", ")
        ))
        .hint(format!(
            "rename one, or use agent-cli confluence page update {page} --storage"
        ))
        .into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first(source: &str) -> Element {
        match parse(source).into_iter().next() {
            Some(Node::Element(element)) => element,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn elements_keep_their_names_attributes_spans_and_cdata() {
        let source = r#"<ac:structured-macro ac:name="code"><ac:parameter ac:name="language">rust</ac:parameter><ac:plain-text-body><![CDATA[a < b && c]]></ac:plain-text-body></ac:structured-macro>"#;
        let code = first(source);
        assert_eq!(code.name, "ac:structured-macro");
        assert_eq!(code.attr("AC:NAME"), Some("code"));
        assert_eq!(code.param("language").as_deref(), Some("rust"));
        assert_eq!(
            code.child("ac:plain-text-body").unwrap().text(),
            "a < b && c"
        );
        assert_eq!(code.span, 0..source.len());
        assert_eq!(&source[code.inner.clone()][..13], "<ac:parameter");
    }

    #[test]
    fn leniency_keeps_text_and_closes_what_was_left_open() {
        let nodes =
            parse("<p>a &lt; b &amp;c &bogus; <br>x <ri:page ri:content-title=\"T&amp;C\"/><p>y");
        let Node::Element(p) = &nodes[0] else {
            panic!()
        };
        assert_eq!(p.text(), "a < b &c &bogus; x y");
        let page = p.elements().find(|e| e.name == "ri:page").unwrap();
        assert_eq!(page.attr("ri:content-title"), Some("T&C"));
        assert_eq!(text_of(&parse("1 < 2 and </stray> 3")), "1 < 2 and  3");
    }

    #[test]
    fn sections_run_to_the_next_heading_of_their_level_or_above() {
        let source = "<p>intro</p><h2>One</h2><p>a</p><h3>Sub</h3><p>b</p><h2>Two</h2><p>c</p>";
        let nodes = parse(source);
        let sections = sections(source, &nodes);
        let one = find(&sections, " **one** ", "1").unwrap();
        assert_eq!(
            &source[one.range.clone()],
            "<h2>One</h2><p>a</p><h3>Sub</h3><p>b</p>"
        );
        let two = find(&sections, "Two", "1").unwrap();
        assert_eq!(&source[two.range.clone()], "<h2>Two</h2><p>c</p>");
        assert!(find(&sections, "three", "1").is_err());
    }

    #[test]
    fn a_layout_cell_is_a_container_and_a_panel_heading_rules_nothing() {
        let source = "<ac:layout><ac:layout-section><ac:layout-cell><h2>Left</h2><p>l</p></ac:layout-cell><ac:layout-cell><h2>Right</h2><p>r</p></ac:layout-cell></ac:layout-section></ac:layout><ac:structured-macro ac:name=\"info\"><ac:rich-text-body><h2>Hidden</h2></ac:rich-text-body></ac:structured-macro>";
        let nodes = parse(source);
        let sections = sections(source, &nodes);
        let names: Vec<&str> = sections.iter().map(|s| s.heading.as_str()).collect();
        assert_eq!(names, ["Left", "Right"]);
        assert_eq!(&source[sections[0].range.clone()], "<h2>Left</h2><p>l</p>");
    }

    #[test]
    fn two_sections_of_one_name_are_ambiguous() {
        let source = "<h2>Steps</h2><h3>Steps</h3>";
        let nodes = parse(source);
        let error = find(&sections(source, &nodes), "steps", "1101").unwrap_err();
        assert!(error.to_string().contains("2 sections named"), "{error}");
    }
}
