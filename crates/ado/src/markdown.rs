//! Azure DevOps rich text as Markdown, and Markdown back as the HTML it
//! stores. Ported from ticket-tui's `html.rs` tokenizer and `markdown.rs`.
//!
//! Work item descriptions, acceptance criteria and comments come back as
//! whatever the browser editor wrote. An agent reads Markdown far better than
//! `<div>` soup, and writes it too, so reads go through [`html_to_markdown`]
//! and writes through [`markdown_to_html`]. The pair round-trips the
//! formatting a description actually uses: paragraphs, nested lists, links,
//! inline code, fenced blocks, headings, bold and rules. A table or an image
//! reads as its plain text.
//!
//! A mention anchor reads back as `@<Display Name>`, which `compose`
//! resolves again on the way out, so a round trip keeps it.
//!
//! Nothing here parses HTML strictly. Unknown tags are transparent (their
//! text survives, their markup does not), and malformed input is rendered as
//! the text it looks like rather than dropped.

/// Longest entity body worth looking at: `&middot;` and `&#x1F600;` both fit.
const MAX_ENTITY: usize = 12;

/// The named entities Azure DevOps's editor actually emits. Every replacement
/// is a single character, and an unknown name is left standing as it was
/// written rather than swallowed.
const NAMED_ENTITIES: &[(&str, char)] = &[
    ("nbsp", ' '),
    ("amp", '&'),
    ("lt", '<'),
    ("gt", '>'),
    ("quot", '"'),
    ("apos", '\''),
    ("mdash", '—'),
    ("ndash", '–'),
    ("hellip", '…'),
    ("copy", '©'),
    ("rsquo", '’'),
    ("lsquo", '‘'),
    ("rdquo", '”'),
    ("ldquo", '“'),
    ("bull", '•'),
    ("middot", '·'),
    ("times", '×'),
];

/// Walks `html` once, handing every text node and every tag to `writer` in
/// the order they were written. Markup that does not parse as a tag is handed
/// over as the text it looks like, so nothing is dropped on the way.
fn walk(html: &str, writer: &mut MarkdownWriter) {
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        writer.text(&rest[..start]);
        let after = &rest[start + 1..];
        // A comment runs to `-->` however much markup it swallows on the way.
        if let Some(body) = after.strip_prefix("!--") {
            let Some(end) = body.find("-->") else {
                return;
            };
            rest = &body[end + 3..];
            continue;
        }
        let Some(end) = after.find('>') else {
            // A `<` with nothing closing it is text someone typed.
            writer.text(&rest[start..]);
            return;
        };
        let raw = &after[..end];
        // A `<` inside what looked like a tag means the outer one was never a
        // tag at all: `a < b <br>` opens no element. Keep it as text and start
        // again from the inner `<`.
        if let Some(inner) = raw.find('<') {
            writer.text(&rest[start..start + 1 + inner]);
            rest = &after[inner..];
            continue;
        }
        let literal = &rest[start..start + end + 2];
        rest = &after[end + 1..];
        // A doctype or processing instruction says nothing a reader wants.
        if raw.starts_with(['!', '?']) {
            continue;
        }
        match Tag::parse(raw) {
            Some(tag) => writer.tag(&tag),
            None => writer.text(literal),
        }
    }
    writer.text(rest);
}

/// One tag, reduced to the parts a renderer reads.
struct Tag<'a> {
    name: String,
    closing: bool,
    attributes: &'a str,
}

impl<'a> Tag<'a> {
    /// Parses the text between `<` and `>`, or returns `None` when it does not
    /// begin like a tag name: `a < 5` is arithmetic, not markup.
    fn parse(raw: &'a str) -> Option<Self> {
        let trimmed = raw.trim_start();
        let (closing, body) = match trimmed.strip_prefix('/') {
            Some(rest) => (true, rest.trim_start()),
            None => (false, trimmed),
        };
        if !body.starts_with(|character: char| character.is_ascii_alphabetic()) {
            return None;
        }
        let split = body
            .find(|character: char| !character.is_ascii_alphanumeric())
            .unwrap_or(body.len());
        let (name, attributes) = body.split_at(split);
        Some(Self {
            name: name.to_ascii_lowercase(),
            closing,
            attributes,
        })
    }

    /// The value of one attribute, with its entities decoded. Quoted and bare
    /// values both parse, and a name that only appears inside another value is
    /// not mistaken for the attribute itself.
    fn attribute(&self, wanted: &str) -> Option<String> {
        let lowered = self.attributes.to_ascii_lowercase();
        let mut from = 0;
        while let Some(offset) = lowered[from..].find(wanted) {
            let start = from + offset;
            from = start + wanted.len();
            let standalone = start == 0
                || lowered[..start].ends_with(|character: char| character.is_whitespace());
            if !standalone {
                continue;
            }
            if let Some(value) = self.attributes[from..].trim_start().strip_prefix('=') {
                return Some(attribute_value(value.trim_start()));
            }
        }
        None
    }
}

fn attribute_value(raw: &str) -> String {
    let value = if let Some(rest) = raw.strip_prefix('"') {
        rest.split('"').next().unwrap_or_default()
    } else if let Some(rest) = raw.strip_prefix('\'') {
        rest.split('\'').next().unwrap_or_default()
    } else {
        raw.split(|character: char| character.is_whitespace())
            .next()
            .unwrap_or_default()
    };
    decode_entities(value)
}

/// Decodes the entities in one text node: `&#8217;` and `&#x2014;` by their
/// code point, the names in [`NAMED_ENTITIES`] by table. Anything else (a
/// bare `&`, an unknown name) is left exactly as it was written, and each
/// entity is decoded once, so `&amp;lt;` is the text `&lt;`.
fn decode_entities(raw: &str) -> String {
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
            .filter(|end| *end <= MAX_ENTITY)
            .map(|end| &after[..end])
            .and_then(|body| decode_entity(body).map(|character| (body.len(), character)));
        match decoded {
            Some((length, character)) => {
                out.push(character);
                rest = &after[length + 1..];
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

fn decode_entity(body: &str) -> Option<char> {
    if let Some(number) = body.strip_prefix('#') {
        let code = match number.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => number.parse().ok()?,
        };
        return char::from_u32(code);
    }
    let lowered = body.to_ascii_lowercase();
    NAMED_ENTITIES
        .iter()
        .find(|(name, _)| *name == lowered)
        .map(|(_, character)| *character)
}

/// The Markdown one piece of Azure DevOps rich text reads as.
///
/// Paragraphs are separated by a blank line, `<ul>` items take `- ` and `<ol>`
/// items their number, nested lists indent two spaces a level, links read as
/// `[text](url)`, `<code>` keeps its backticks, `<pre>` becomes a fenced
/// block, headings take their `#`s, and `<b>` becomes `**bold**`.
#[must_use]
pub(crate) fn html_to_markdown(html: &str) -> String {
    let mut writer = MarkdownWriter {
        out: String::with_capacity(html.len()),
        ..MarkdownWriter::default()
    };
    walk(html, &mut writer);
    writer.out.trim().to_owned()
}

/// What a block boundary asks of the layout before the next content lands.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
enum Break {
    #[default]
    None,
    /// Start a new line: `<div>` soup, list items, table rows.
    Line,
    /// Leave one blank line: paragraphs, headings, tables, `<pre>` blocks.
    Paragraph,
}

/// One open `<ul>` or `<ol>`, and how many items it has numbered so far.
struct List {
    ordered: bool,
    item: usize,
}

/// What [`MarkdownWriter::links`] holds for a mention anchor in place of a
/// target: its text, not a link, is what reads back.
const MENTION: &str = "\0mention";

/// Writes Markdown as the walk hands it the document.
#[derive(Default)]
struct MarkdownWriter {
    out: String,
    /// The `<ul>` and `<ol>` elements open around the current position.
    lists: Vec<List>,
    /// One entry per open `<a>`: its target, and where its text started.
    links: Vec<(String, usize)>,
    /// The block boundary the markup has asked for but no content has needed
    /// yet, so a run of closing tags costs one break rather than four.
    pending: Break,
    /// Depth of `<pre>` nesting: inside one, whitespace is the author's.
    pre: usize,
    /// Whether a `<pre>` has just opened, so the line break editors write
    /// straight after the tag is the markup's rather than the author's.
    pre_start: bool,
    /// Cells written in the current table row, so every cell but the first is
    /// preceded by a separator.
    cells: usize,
}

impl MarkdownWriter {
    fn tag(&mut self, tag: &Tag<'_>) {
        match tag.name.as_str() {
            "br" => self.hard_break(),
            "p" | "blockquote" | "table" => self.request(Break::Paragraph),
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => self.heading(tag),
            "div" => self.request(Break::Line),
            "ul" | "ol" => self.list(tag),
            "li" => self.item(tag),
            "tr" => {
                self.cells = 0;
                self.request(Break::Line);
            }
            "td" | "th" if !tag.closing => {
                if self.cells > 0 {
                    self.trim_trailing_spaces();
                    self.push(" | ");
                }
                self.cells += 1;
            }
            "pre" => self.fence(tag),
            // Inside a `<pre>` the block is already fenced, so the backticks
            // would only be noise.
            "code" if self.pre == 0 => self.push("`"),
            "a" => self.link(tag),
            "b" | "strong" => self.push("**"),
            "img" if !tag.closing => {
                let alt = tag.attribute("alt").unwrap_or_default();
                let alt = alt.trim();
                if alt.is_empty() {
                    self.push("[image]");
                } else {
                    self.push(&format!("[image: {alt}]"));
                }
            }
            "hr" if !tag.closing => {
                self.request(Break::Paragraph);
                self.push("---");
                self.request(Break::Paragraph);
            }
            // Italics, spans, fonts, and everything unrecognised: the text is
            // the part worth keeping.
            _ => {}
        }
    }

    /// Opens or closes a heading. Markdown here goes three levels deep, which
    /// is as far as a description ever reasonably nests; anything below folds
    /// into `###`.
    fn heading(&mut self, tag: &Tag<'_>) {
        self.request(Break::Paragraph);
        if tag.closing {
            return;
        }
        let level = tag.name[1..].parse::<usize>().unwrap_or(1).clamp(1, 3);
        self.push(&format!("{} ", "#".repeat(level)));
    }

    /// Opens or closes a `<pre>` as a fenced block, which is the one place
    /// where the author's own whitespace is kept.
    fn fence(&mut self, tag: &Tag<'_>) {
        if tag.closing {
            self.pre = self.pre.saturating_sub(1);
            if !self.out.ends_with('\n') {
                self.out.push('\n');
            }
            self.out.push_str("```");
            self.request(Break::Paragraph);
        } else {
            self.request(Break::Paragraph);
            self.flush();
            self.out.push_str("```\n");
            self.pre += 1;
            self.pre_start = true;
        }
    }

    /// Opens or closes a list. A list at the top level stands apart from the
    /// prose around it; one nested inside an item only starts a new line, so
    /// the items stay a single block.
    fn list(&mut self, tag: &Tag<'_>) {
        if tag.closing {
            self.lists.pop();
        }
        let wanted = if self.lists.is_empty() {
            Break::Paragraph
        } else {
            Break::Line
        };
        self.request(wanted);
        if !tag.closing {
            self.lists.push(List {
                ordered: tag.name == "ol",
                item: 0,
            });
        }
    }

    /// Writes one list item's marker: `- ` for a bullet list, `1.`, `2.` for a
    /// numbered one, indented two spaces per level of nesting, which is how
    /// [`markdown_to_html`] reads the nesting back.
    fn item(&mut self, tag: &Tag<'_>) {
        self.request(Break::Line);
        if tag.closing {
            return;
        }
        let indent = "  ".repeat(self.lists.len().saturating_sub(1));
        let marker = match self.lists.last_mut() {
            Some(list) if list.ordered => {
                list.item += 1;
                format!("{}. ", list.item)
            }
            _ => "- ".to_owned(),
        };
        self.push(&format!("{indent}{marker}"));
    }

    /// Opens an `<a>` on the hope of a `[text](url)`, which [`Self::close_link`]
    /// either finishes or takes back.
    fn link(&mut self, tag: &Tag<'_>) {
        if tag.closing {
            self.close_link();
            return;
        }
        self.flush();
        let href = if tag.attribute("data-vss-mention").is_some() {
            MENTION.to_owned()
        } else {
            tag.attribute("href").unwrap_or_default()
        };
        self.out.push('[');
        let mark = self.out.len();
        self.links.push((href, mark));
    }

    /// Closes an `<a>`. A link with no target, or whose text already is its
    /// target, is written as the text alone: the brackets would say nothing.
    fn close_link(&mut self) {
        let Some((href, mark)) = self.links.pop() else {
            return;
        };
        let mark = mark.min(self.out.len());
        let text = self.out[mark..].trim().to_owned();
        if href == MENTION {
            self.out.truncate(mark - 1);
            self.push(&format!("@<{}>", text.trim_start_matches('@')));
            return;
        }
        if href.is_empty() {
            // The `[` was written in hope of a target that never came.
            self.out.remove(mark - 1);
            return;
        }
        if text.is_empty() || text == href {
            self.out.truncate(mark - 1);
            self.push(&href);
            return;
        }
        self.push(&format!("]({href})"));
    }

    /// Writes one text node. Outside a `<pre>` a run of whitespace is one
    /// space, the way a browser would lay it out; inside one every space and
    /// line break is the author's.
    fn text(&mut self, raw: &str) {
        if raw.is_empty() {
            return;
        }
        let decoded = decode_entities(raw);
        if self.pre > 0 {
            let mut content = decoded.as_str();
            if std::mem::take(&mut self.pre_start) {
                content = content
                    .strip_prefix("\r\n")
                    .or_else(|| content.strip_prefix('\n'))
                    .unwrap_or(content);
            }
            if content.is_empty() {
                return;
            }
            self.flush();
            self.out.push_str(content);
            return;
        }
        let spaced_before = decoded.starts_with(char::is_whitespace);
        let spaced_after = decoded.ends_with(char::is_whitespace);
        let mut words = decoded.split_whitespace();
        let Some(first) = words.next() else {
            // Whitespace alone still separates two inline tags, unless a block
            // boundary is about to swallow it.
            if self.pending == Break::None && self.mid_line() {
                self.out.push(' ');
            }
            return;
        };
        self.flush();
        if spaced_before && self.mid_line() {
            self.out.push(' ');
        }
        self.out.push_str(first);
        for word in words {
            self.out.push(' ');
            self.out.push_str(word);
        }
        if spaced_after {
            self.out.push(' ');
        }
    }

    /// Whether the next character would land beside text already written.
    fn mid_line(&self) -> bool {
        !self.out.is_empty() && !self.out.ends_with([' ', '\n'])
    }

    fn push(&mut self, text: &str) {
        self.flush();
        self.out.push_str(text);
    }

    fn request(&mut self, wanted: Break) {
        self.pending = self.pending.max(wanted);
    }

    /// Applies the boundary the markup asked for. Nothing is written at the
    /// start of the document, and repeated boundaries never stack past one
    /// blank line.
    fn flush(&mut self) {
        let newlines = match std::mem::take(&mut self.pending) {
            Break::None => return,
            Break::Line => 1,
            Break::Paragraph => 2,
        };
        self.trim_trailing_spaces();
        if self.out.is_empty() {
            return;
        }
        for _ in self.trailing_newlines()..newlines {
            self.out.push('\n');
        }
    }

    /// A `<br>`, which unlike a block boundary stacks: it is the blank line an
    /// editor writes between two lines of the same paragraph.
    fn hard_break(&mut self) {
        self.flush();
        self.trim_trailing_spaces();
        if !self.out.is_empty() && self.trailing_newlines() < 2 {
            self.out.push('\n');
        }
    }

    /// Drops the spaces a line ended on: they came from the gaps between tags
    /// rather than from anything the author typed.
    fn trim_trailing_spaces(&mut self) {
        if self.pre > 0 {
            return;
        }
        while self.out.ends_with([' ', '\t']) {
            self.out.pop();
        }
    }

    fn trailing_newlines(&self) -> usize {
        self.out
            .chars()
            .rev()
            .take_while(|character| *character == '\n')
            .count()
    }
}

/// Rebuilds the HTML Azure DevOps stores from Markdown.
///
/// A blank line ends a paragraph and the lines inside one are joined with
/// `<br>`; `- `, `* `, `1. `, and `1) ` are list items, nested two spaces a
/// level; ` ``` ` fences a `<pre>`; `#` through `###` are headings; `---` is a
/// rule; and `[text](url)`, `` `code` ``, and `**bold**` are what they look
/// like. Everything else is text, with `&`, `<`, and `>` escaped. An empty
/// document stays empty, which is how a description is cleared.
#[must_use]
pub(crate) fn markdown_to_html(markdown: &str) -> String {
    let normalized = markdown.replace("\r\n", "\n");
    let mut builder = HtmlBuilder::default();
    for line in normalized.lines() {
        builder.line(line);
    }
    builder.finish()
}

#[derive(Default)]
struct HtmlBuilder {
    out: String,
    /// The lines of the paragraph being read, joined with `<br>` when it ends.
    paragraph: Vec<String>,
    /// Whether each list open around the current item is ordered, outermost
    /// first. Every level holds one open `<li>`.
    lists: Vec<bool>,
    /// The lines of the fenced block being read, if a fence is open.
    fenced: Option<Vec<String>>,
    /// How many backticks opened it, which is how many close it.
    fence_len: usize,
}

/// The run of backticks a line starts with, after any indent.
pub(crate) fn backticks(line: &str) -> usize {
    line.trim_start()
        .chars()
        .take_while(|held| *held == '`')
        .count()
}

impl HtmlBuilder {
    fn line(&mut self, line: &str) {
        if let Some(fenced) = self.fenced.as_mut() {
            // The closing fence is at least as long as the opening one and
            // stands alone, so a shorter run of backticks inside is content.
            let run = backticks(line);
            if run >= self.fence_len && line.trim_start()[run..].trim().is_empty() {
                self.close_fence();
            } else {
                fenced.push(line.to_owned());
            }
            return;
        }
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            self.close_blocks();
            self.fence_len = backticks(trimmed);
            self.fenced = Some(Vec::new());
        } else if trimmed.is_empty() {
            self.close_blocks();
        } else if let Some((level, text)) = heading(trimmed) {
            self.close_blocks();
            self.out
                .push_str(&format!("<h{level}>{}</h{level}>", inline(text, 0)));
        } else if is_rule(trimmed) {
            self.close_blocks();
            self.out.push_str("<hr>");
        } else if let Some((depth, ordered, text)) = list_item(line) {
            self.close_paragraph();
            self.item(depth, ordered, text);
        } else {
            self.close_lists();
            self.paragraph.push(trimmed.to_owned());
        }
    }

    fn finish(mut self) -> String {
        self.close_fence();
        self.close_blocks();
        self.out
    }

    /// Opens one list item at `depth`, closing and opening whatever lists that
    /// takes. A jump of more than one level deep is read as one level: two
    /// stray spaces are far likelier than an item with no parent.
    fn item(&mut self, depth: usize, ordered: bool, text: &str) {
        let depth = depth.min(self.lists.len());
        while self.lists.len() > depth + 1 {
            self.out.push_str("</li>");
            let ordered = self.lists.pop().unwrap_or(false);
            self.out.push_str(close_list(ordered));
        }
        if self.lists.len() == depth + 1 {
            self.out.push_str("</li>");
            if self.lists[depth] != ordered {
                self.lists.pop();
                self.out.push_str(close_list(!ordered));
                self.out.push_str(open_list(ordered));
                self.lists.push(ordered);
            }
        } else {
            self.out.push_str(open_list(ordered));
            self.lists.push(ordered);
        }
        self.out.push_str("<li>");
        self.out.push_str(&inline(text, 0));
    }

    fn close_blocks(&mut self) {
        self.close_paragraph();
        self.close_lists();
    }

    fn close_paragraph(&mut self) {
        if self.paragraph.is_empty() {
            return;
        }
        let lines: Vec<String> = std::mem::take(&mut self.paragraph)
            .iter()
            .map(|line| inline(line, 0))
            .collect();
        self.out.push_str("<p>");
        self.out.push_str(&lines.join("<br>"));
        self.out.push_str("</p>");
    }

    fn close_lists(&mut self) {
        while let Some(ordered) = self.lists.pop() {
            self.out.push_str("</li>");
            self.out.push_str(close_list(ordered));
        }
    }

    /// Closes the fenced block, if one is open. A fence left unclosed at the
    /// end of the text still holds code.
    fn close_fence(&mut self) {
        let Some(fenced) = self.fenced.take() else {
            return;
        };
        self.out.push_str("<pre>");
        self.out.push_str(&escape(&fenced.join("\n")));
        self.out.push_str("</pre>");
    }
}

const fn open_list(ordered: bool) -> &'static str {
    if ordered { "<ol>" } else { "<ul>" }
}

const fn close_list(ordered: bool) -> &'static str {
    if ordered { "</ol>" } else { "</ul>" }
}

/// The heading level and text of a `#` line, or `None` when the line is not
/// one. Levels below three read as three, which is as deep as the HTML goes.
fn heading(trimmed: &str) -> Option<(usize, &str)> {
    let hashes = trimmed
        .chars()
        .take_while(|character| *character == '#')
        .count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let text = trimmed[hashes..].strip_prefix(' ')?;
    Some((hashes.min(3), text.trim()))
}

/// Whether a line is a horizontal rule: three or more of `-` or `*`, nothing
/// else on it.
fn is_rule(trimmed: &str) -> bool {
    trimmed.len() >= 3
        && (trimmed.chars().all(|character| character == '-')
            || trimmed.chars().all(|character| character == '*'))
}

/// The nesting depth, kind, and text of a list item, or `None` when the line
/// is not one. Every two leading spaces is one level.
fn list_item(line: &str) -> Option<(usize, bool, &str)> {
    let rest = line.trim_start_matches(' ');
    let depth = (line.len() - rest.len()) / 2;
    if let Some(text) = rest.strip_prefix("- ").or_else(|| rest.strip_prefix("* ")) {
        return Some((depth, false, text.trim()));
    }
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let after = &rest[digits..];
    let text = after
        .strip_prefix(". ")
        .or_else(|| after.strip_prefix(") "))?;
    Some((depth, true, text.trim()))
}

/// How far `[text](url)` and `**bold**` are followed into one another before
/// the rest is taken as plain text. Nothing an author writes nests this far;
/// a pathological text cannot make this recurse without end.
const MAX_INLINE_DEPTH: usize = 6;

/// Renders one line's inline markup: code spans, links, and bold, with
/// everything else escaped as the text it is.
fn inline(text: &str, depth: usize) -> String {
    let mut out = String::with_capacity(text.len());
    if depth >= MAX_INLINE_DEPTH {
        out.push_str(&escape(text));
        return out;
    }
    let mut rest = text;
    while let Some(index) = rest.find(['`', '[', '*']) {
        let (before, after) = rest.split_at(index);
        out.push_str(&escape(before));
        let taken = match after.as_bytes()[0] {
            b'`' => inline_code(after, &mut out),
            b'[' => inline_link(after, depth, &mut out),
            _ => inline_bold(after, depth, &mut out),
        };
        match taken {
            Some(length) => rest = &after[length..],
            None => {
                out.push_str(&escape(&after[..1]));
                rest = &after[1..];
            }
        }
    }
    out.push_str(&escape(rest));
    out
}

/// A `` `code` `` span, and how much of `rest` it took.
fn inline_code(rest: &str, out: &mut String) -> Option<usize> {
    let end = rest[1..].find('`')?;
    out.push_str(&format!("<code>{}</code>", escape(&rest[1..=end])));
    Some(end + 2)
}

/// A `[text](url)` link, and how much of `rest` it took.
fn inline_link(rest: &str, depth: usize, out: &mut String) -> Option<usize> {
    let close = rest.find("](")?;
    let end = rest[close..].find(')')? + close;
    let text = &rest[1..close];
    let href = &rest[close + 2..end];
    out.push_str(&format!(
        "<a href=\"{}\">{}</a>",
        escape_attribute(href),
        inline(text, depth + 1)
    ));
    Some(end + 1)
}

/// A `**bold**` run, and how much of `rest` it took.
fn inline_bold(rest: &str, depth: usize, out: &mut String) -> Option<usize> {
    let body = rest.strip_prefix("**")?;
    let end = body.find("**")?;
    out.push_str(&format!("<b>{}</b>", inline(&body[..end], depth + 1)));
    Some(end + 4)
}

/// The three characters that would otherwise be read as markup.
pub(crate) fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            other => out.push(other),
        }
    }
    out
}

/// The same, plus the quote that would end an attribute early.
pub(crate) fn escape_attribute(text: &str) -> String {
    escape(text).replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_description_reads_as_paragraphs_bullets_code_and_bold() {
        assert_eq!(
            html_to_markdown(concat!(
                "<p><b>Problem.</b> Descriptions are the one long-form field.</p>",
                "<p>Approach:</p>",
                "<ul>",
                "<li>Hand the description to <code>$EDITOR</code>.</li>",
                "<li>Read the Markdown back as HTML.</li>",
                "</ul>",
            )),
            concat!(
                "**Problem.** Descriptions are the one long-form field.\n",
                "\n",
                "Approach:\n",
                "\n",
                "- Hand the description to `$EDITOR`.\n",
                "- Read the Markdown back as HTML.",
            )
        );
    }

    #[test]
    fn headings_numbered_lists_nesting_links_and_fences_keep_their_shape() {
        assert_eq!(
            html_to_markdown(concat!(
                "<h2>Steps</h2>",
                "<ol>",
                "<li>Open the <a href=\"https://dev.azure.com/contoso\">board</a>.</li>",
                "<li>Pick a ticket:<ul><li>ready</li><li>blocked</li></ul></li>",
                "</ol>",
                "<p>Then run:</p>",
                "<pre>cargo test --all-targets</pre>",
            )),
            concat!(
                "## Steps\n",
                "\n",
                "1. Open the [board](https://dev.azure.com/contoso).\n",
                "2. Pick a ticket:\n",
                "  - ready\n",
                "  - blocked\n",
                "\n",
                "Then run:\n",
                "\n",
                "```\n",
                "cargo test --all-targets\n",
                "```",
            )
        );
        assert_eq!(
            html_to_markdown(
                "<h1>One</h1><h5>Deep</h5><hr><div>a</div><div>b&nbsp;&amp;&#8212;c</div>"
            ),
            "# One\n\n### Deep\n\n---\n\na\nb &\u{2014}c"
        );
        assert_eq!(
            html_to_markdown("<p>See <a href=\"https://x/y\"></a> and <a>nothing</a></p>"),
            "See https://x/y and nothing"
        );
        assert_eq!(html_to_markdown("a < b <br>c"), "a < b\nc");
        assert_eq!(html_to_markdown(""), "");
    }

    #[test]
    fn markdown_builds_the_paragraphs_lists_links_code_and_headings_back() {
        assert_eq!(
            markdown_to_html("**Problem.** One line.\n\nApproach:"),
            "<p><b>Problem.</b> One line.</p><p>Approach:</p>"
        );
        assert_eq!(
            markdown_to_html("1. one\n2. two:\n  - deep\n  - deeper"),
            "<ol><li>one</li><li>two:<ul><li>deep</li><li>deeper</li></ul></li></ol>"
        );
        assert_eq!(
            markdown_to_html("## Steps\n\n[board](https://dev.azure.com/contoso?a=1&b=2)"),
            concat!(
                "<h2>Steps</h2>",
                "<p><a href=\"https://dev.azure.com/contoso?a=1&amp;b=2\">board</a></p>",
            )
        );
        assert_eq!(
            markdown_to_html("Run `cargo test`:\n\n```\nif a < b {\n    ok();\n}\n```"),
            "<p>Run <code>cargo test</code>:</p><pre>if a &lt; b {\n    ok();\n}</pre>"
        );
        assert_eq!(markdown_to_html("one\ntwo"), "<p>one<br>two</p>");
        assert_eq!(markdown_to_html("---"), "<hr>");
        assert_eq!(markdown_to_html("a & b < c"), "<p>a &amp; b &lt; c</p>");
        assert_eq!(markdown_to_html("   \n\n  "), "");
    }

    #[test]
    fn markdown_that_goes_out_as_html_reads_back_the_same() {
        for markdown in [
            "**Problem.** It breaks.\n\n- one\n- two `code`\n  1. deep",
            "## Steps\n\n1. Open the [board](https://dev.azure.com/contoso).\n\n```\nlet x = 1 < 2;\n```",
        ] {
            assert_eq!(html_to_markdown(&markdown_to_html(markdown)), markdown);
        }
    }

    #[test]
    fn malformed_and_unusual_markup_never_panics() {
        let document = concat!(
            "<h1>Title &amp; more</h1><ol><li>one<ul><li><a href=\"https://x/y\">link</a>",
            "</li></ul></li></ol><pre>code &lt;here&gt;</pre><table><tr><td>a</td>",
            "<td>b</td></tr></table><p><img alt=\"pic\"><code>x</code>&#8212;done</p><!-- x",
        );
        for end in 0..=document.len() {
            if document.is_char_boundary(end) {
                let markdown = html_to_markdown(&document[..end]);
                let _ = markdown_to_html(&markdown);
            }
        }
        for text in [
            "[",
            "[]",
            "[](",
            "`",
            "**",
            "***a**",
            "```",
            "```\nunclosed",
            "- ",
            "1.",
            "#",
            "#no space",
            "  - deep with no parent",
            "&<>",
        ] {
            let _ = html_to_markdown(&markdown_to_html(text));
        }
    }

    #[test]
    fn a_mention_anchor_reads_back_as_the_name_it_shows() {
        assert_eq!(
            html_to_markdown(
                r##"<div><a href="#" data-vss-mention="version:2.0,u-2">@Sam Lee</a> please look</div>"##
            ),
            "@<Sam Lee> please look"
        );
    }
}
