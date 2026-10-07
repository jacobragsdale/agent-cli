//! Markdown as storage, for the bodies and comments agents write. v2 writes
//! take storage, ADF or wiki markup but never Markdown, and Confluence's own
//! Markdown converter is an undocumented input to a deprecated endpoint.
//!
//! The mapping is `markdown`'s read backwards: an alert is an info, tip, note
//! or warning macro, a fence a code macro, `[text](<KEY:Title>)` a page link,
//! `attachment:NAME` a file on the page, `@<Name>` a mention, `<details>`
//! with a `<summary>` an expand. A list or table cell holds its text in a
//! `<p>`, as the editor writes it. Other HTML passes through as storage.

use std::collections::HashMap;

use pulldown_cmark::{BlockQuoteKind, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

use crate::storage::{decode_entities, escape};
use crate::syntax::{cdata, emoticons, text_escape};

/// Where a body goes: its space (a page link into it needs no key) and the
/// account ids of the people it mentions, by the name written.
pub(crate) struct Target<'a> {
    pub(crate) space: &'a str,
    pub(crate) people: &'a HashMap<String, String>,
}

/// HTML Markdown passes through, which `@<…>` must not be read as.
const INLINE_TAGS: &[&str] = &[
    "u", "sub", "sup", "br", "b", "i", "em", "strong", "s", "code", "span", "time", "details",
    "summary",
];

const OPEN: char = '\u{e000}';
const SHUT: char = '\u{e001}';

/// The names a body mentions as `@<Name>`, each once, in order.
pub(crate) fn mentioned(markdown: &str) -> Vec<String> {
    let mut names = hide_mentions(markdown).1;
    let mut seen = Vec::new();
    names.retain(|name| {
        let new = !seen.contains(name);
        seen.push(name.clone());
        new
    });
    names
}

/// `@<Name>` as a placeholder Markdown leaves alone (`<Name>` would read as
/// an HTML tag), and the names in placeholder order.
fn hide_mentions(markdown: &str) -> (String, Vec<String>) {
    let mut out = String::with_capacity(markdown.len());
    let mut names = Vec::new();
    let mut rest = markdown;
    while let Some(at) = rest.find("@<") {
        let escaped = rest[..at].ends_with('\\');
        let name_end = rest[at + 2..].find(['>', '<', '\n']);
        // `@<u>id</u>` is an address with a tag in it, not a mention.
        let tag = |end: usize| {
            let name = &rest[at + 2..at + 2 + end];
            INLINE_TAGS.contains(&name.trim_start_matches('/'))
        };
        match name_end {
            Some(end)
                if !escaped && end > 0 && !tag(end) && rest[at + 2 + end..].starts_with('>') =>
            {
                out.push_str(&rest[..at]);
                out.push(OPEN);
                out.push_str(&names.len().to_string());
                out.push(SHUT);
                names.push(rest[at + 2..at + 2 + end].trim().to_owned());
                rest = &rest[at + 2 + end + 1..];
            }
            _ => {
                out.push_str(&rest[..at + 2]);
                rest = &rest[at + 2..];
            }
        }
    }
    out.push_str(rest);
    (out, names)
}

pub(crate) fn to_storage(markdown: &str, target: &Target) -> String {
    let (hidden, names) = hide_mentions(markdown);
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_GFM;
    let events: Vec<Event> = Parser::new_ext(&hidden, options).collect();
    let mut composer = Composer {
        target,
        names,
        out: String::with_capacity(markdown.len() * 2),
        blocks: Vec::new(),
        lists: Vec::new(),
        captures: Vec::new(),
        code: None,
        head: false,
        expands: 0,
    };
    for (at, event) in events.iter().enumerate() {
        composer.event(event, &events[at + 1..]);
    }
    composer.out
}

enum Block {
    /// A list item, and whether the `<p>` its loose text needed is open.
    Item(bool),
    /// A task, and whether its status and body have been written.
    Task(bool),
    Paragraph,
    Other,
}

struct Capture {
    start: usize,
    plain: String,
    dest: String,
}

struct Composer<'a> {
    target: &'a Target<'a>,
    names: Vec<String>,
    out: String,
    blocks: Vec<Block>,
    /// Per open list: whether it is a task list.
    lists: Vec<bool>,
    captures: Vec<Capture>,
    code: Option<(String, String)>,
    head: bool,
    expands: usize,
}

impl Composer<'_> {
    fn event(&mut self, event: &Event, after: &[Event]) {
        match event {
            Event::Start(tag) => self.start(tag, after),
            Event::End(tag) => self.end(*tag),
            Event::Text(text) if self.code.is_some() => {
                let restored = self.restore(text);
                if let Some((_, code)) = &mut self.code {
                    code.push_str(&restored);
                }
            }
            Event::Text(text) => self.text(text),
            Event::Code(code) => {
                self.inline_start();
                let code = self.restore(code);
                self.capture_plain(&code);
                self.out
                    .push_str(&format!("<code>{}</code>", text_escape(&code)));
            }
            Event::InlineHtml(html) => {
                self.inline_start();
                let tag = html.trim().to_ascii_lowercase().replace(' ', "");
                if matches!(tag.as_str(), "<br>" | "<br/>") {
                    self.out.push_str("<br />");
                } else {
                    self.out.push_str(html);
                }
            }
            Event::Html(html) => self.html(html),
            Event::SoftBreak => {
                self.capture_plain(" ");
                self.out.push(' ');
            }
            Event::HardBreak => self.out.push_str("<br />"),
            Event::Rule => {
                self.close_implicit();
                self.out.push_str("<hr />");
            }
            Event::TaskListMarker(done) => {
                if let Some(Block::Task(false)) = self.blocks.last() {
                    self.open_task(*done);
                }
            }
            _ => {}
        }
    }

    fn start(&mut self, tag: &Tag, after: &[Event]) {
        match tag {
            Tag::Paragraph => {
                if !matches!(self.blocks.last(), Some(Block::Task(_))) {
                    self.close_implicit();
                    self.out.push_str("<p>");
                    self.blocks.push(Block::Paragraph);
                }
            }
            Tag::Heading { level, .. } => {
                self.close_implicit();
                self.out.push_str(&format!("<{level}>"));
                self.blocks.push(Block::Other);
            }
            Tag::BlockQuote(kind) => {
                self.close_implicit();
                self.out.push_str(&match kind.map(alert) {
                    Some(name) => {
                        format!("<ac:structured-macro ac:name=\"{name}\"><ac:rich-text-body>")
                    }
                    None => "<blockquote>".to_owned(),
                });
                self.blocks.push(Block::Other);
            }
            Tag::CodeBlock(kind) => {
                self.close_implicit();
                let language = match kind {
                    CodeBlockKind::Fenced(info) => info
                        .split_whitespace()
                        .next()
                        .unwrap_or_default()
                        .to_owned(),
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some((language, String::new()));
            }
            Tag::HtmlBlock => self.close_implicit(),
            Tag::List(start) => {
                self.close_implicit();
                let tasks = is_task_list(after);
                self.out.push_str(&match (tasks, start) {
                    (true, _) => "<ac:task-list>".to_owned(),
                    (false, Some(1)) => "<ol>".to_owned(),
                    (false, Some(start)) => format!("<ol start=\"{start}\">"),
                    (false, None) => "<ul>".to_owned(),
                });
                self.lists.push(tasks);
            }
            Tag::Item => {
                if self.lists.last() == Some(&true) {
                    self.out.push_str("<ac:task>");
                    self.blocks.push(Block::Task(false));
                } else {
                    self.out.push_str("<li>");
                    self.blocks.push(Block::Item(false));
                }
            }
            Tag::Table(_) => {
                self.close_implicit();
                self.out.push_str("<table><tbody>");
            }
            Tag::TableHead => {
                self.head = true;
                self.out.push_str("<tr>");
            }
            Tag::TableRow => self.out.push_str("<tr>"),
            Tag::TableCell => {
                self.out
                    .push_str(if self.head { "<th><p>" } else { "<td><p>" });
                self.blocks.push(Block::Other);
            }
            Tag::Emphasis => self.inline_tag("<em>"),
            Tag::Strong => self.inline_tag("<strong>"),
            Tag::Strikethrough => self.inline_tag("<s>"),
            Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } => {
                self.inline_start();
                self.captures.push(Capture {
                    start: self.out.len(),
                    plain: String::new(),
                    dest: dest_url.to_string(),
                });
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                if matches!(self.blocks.last(), Some(Block::Paragraph)) {
                    self.blocks.pop();
                    self.out.push_str("</p>");
                }
            }
            TagEnd::Heading(level) => {
                self.blocks.pop();
                self.out.push_str(&format!("</{level}>"));
            }
            TagEnd::BlockQuote(kind) => {
                self.blocks.pop();
                self.out.push_str(if kind.is_some() {
                    "</ac:rich-text-body></ac:structured-macro>"
                } else {
                    "</blockquote>"
                });
            }
            TagEnd::CodeBlock => {
                if let Some((language, mut code)) = self.code.take() {
                    if code.ends_with('\n') {
                        code.pop();
                    }
                    self.out.push_str(&code_macro(&language, &code));
                }
            }
            TagEnd::List(ordered) => {
                let tasks = self.lists.pop() == Some(true);
                self.out.push_str(match (tasks, ordered) {
                    (true, _) => "</ac:task-list>",
                    (false, true) => "</ol>",
                    (false, false) => "</ul>",
                });
            }
            TagEnd::Item => match self.blocks.pop() {
                Some(Block::Task(opened)) => {
                    if !opened {
                        self.open_task(false);
                    }
                    self.out.push_str("</ac:task-body></ac:task>");
                }
                Some(Block::Item(open)) => {
                    if open {
                        self.out.push_str("</p>");
                    }
                    self.out.push_str("</li>");
                }
                _ => {}
            },
            TagEnd::Table => self.out.push_str("</tbody></table>"),
            TagEnd::TableHead => {
                self.head = false;
                self.out.push_str("</tr>");
            }
            TagEnd::TableRow => self.out.push_str("</tr>"),
            TagEnd::TableCell => {
                self.blocks.pop();
                self.out
                    .push_str(if self.head { "</p></th>" } else { "</p></td>" });
            }
            TagEnd::Emphasis => self.out.push_str("</em>"),
            TagEnd::Strong => self.out.push_str("</strong>"),
            TagEnd::Strikethrough => self.out.push_str("</s>"),
            TagEnd::Link => self.finish_link(),
            TagEnd::Image => self.finish_image(),
            _ => {}
        }
    }

    /// Text in a tight list item opens the `<p>` the editor would have
    /// written; in a task, it opens the task's body.
    fn inline_start(&mut self) {
        match self.blocks.last_mut() {
            Some(Block::Item(open @ false)) => {
                *open = true;
                self.out.push_str("<p>");
            }
            Some(Block::Task(false)) => self.open_task(false),
            _ => {}
        }
    }

    /// A block starting inside a list item closes the item's `<p>`.
    fn close_implicit(&mut self) {
        if let Some(Block::Item(open @ true)) = self.blocks.last_mut() {
            *open = false;
            self.out.push_str("</p>");
        }
    }

    fn open_task(&mut self, done: bool) {
        if let Some(Block::Task(opened)) = self.blocks.last_mut() {
            *opened = true;
        }
        let status = if done { "complete" } else { "incomplete" };
        self.out.push_str(&format!(
            "<ac:task-status>{status}</ac:task-status><ac:task-body>"
        ));
    }

    fn inline_tag(&mut self, tag: &str) {
        self.inline_start();
        self.out.push_str(tag);
    }

    fn capture_plain(&mut self, text: &str) {
        for capture in &mut self.captures {
            capture.plain.push_str(text);
        }
    }

    fn restore(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(at) = rest.find(OPEN) {
            out.push_str(&rest[..at]);
            let after = &rest[at + OPEN.len_utf8()..];
            let end = after.find(SHUT).unwrap_or(after.len());
            let name = after[..end]
                .parse::<usize>()
                .ok()
                .and_then(|index| self.names.get(index));
            out.push_str(&format!("@<{}>", name.map_or("", String::as_str)));
            rest = after.get(end + SHUT.len_utf8()..).unwrap_or_default();
        }
        out.push_str(rest);
        out
    }

    fn text(&mut self, text: &str) {
        self.inline_start();
        let plain = self.restore(text);
        self.capture_plain(&plain);
        let mut rest = text;
        while let Some(at) = rest.find(OPEN) {
            self.out.push_str(&emoticons(&text_escape(&rest[..at])));
            let after = &rest[at + OPEN.len_utf8()..];
            let end = after.find(SHUT).unwrap_or(after.len());
            let name = after[..end]
                .parse::<usize>()
                .ok()
                .and_then(|index| self.names.get(index))
                .cloned()
                .unwrap_or_default();
            self.out.push_str(&self.mention(&name));
            rest = after.get(end + SHUT.len_utf8()..).unwrap_or_default();
        }
        self.out.push_str(&emoticons(&text_escape(rest)));
    }

    fn mention(&self, name: &str) -> String {
        match self.target.people.get(name) {
            Some(id) => format!(
                "<ac:link><ri:user ri:account-id=\"{}\" /></ac:link>",
                escape(id)
            ),
            None => text_escape(&format!("@<{name}>")),
        }
    }

    fn finish_link(&mut self) {
        let Some(capture) = self.captures.pop() else {
            return;
        };
        let inner = self.out.split_off(capture.start);
        let body = if capture.plain.is_empty() {
            String::new()
        } else if inner != text_escape(&capture.plain) {
            format!("<ac:link-body>{inner}</ac:link-body>")
        } else {
            format!(
                "<ac:plain-text-link-body><![CDATA[{}]]></ac:plain-text-link-body>",
                cdata(&capture.plain)
            )
        };
        let dest = capture.dest;
        let link = if let Some(file) = dest.strip_prefix("attachment:") {
            format!(
                "<ac:link><ri:attachment ri:filename=\"{}\" />{body}</ac:link>",
                escape(file)
            )
        } else if let Some(anchor) = dest.strip_prefix('#') {
            format!("<ac:link ac:anchor=\"{}\">{body}</ac:link>", escape(anchor))
        } else if let Some((space, title)) = page_ref(&dest) {
            let key = if space == self.target.space {
                String::new()
            } else {
                format!("ri:space-key=\"{}\" ", escape(space))
            };
            format!(
                "<ac:link><ri:page {key}ri:content-title=\"{}\" />{body}</ac:link>",
                escape(title)
            )
        } else {
            format!("<a href=\"{}\">{inner}</a>", escape(&dest))
        };
        self.out.push_str(&link);
    }

    fn finish_image(&mut self) {
        let Some(capture) = self.captures.pop() else {
            return;
        };
        self.out.truncate(capture.start);
        let alt = if capture.plain.is_empty() {
            String::new()
        } else {
            format!(" ac:alt=\"{}\"", escape(&capture.plain))
        };
        let target = match capture.dest.strip_prefix("attachment:") {
            Some(file) => format!("<ri:attachment ri:filename=\"{}\" />", escape(file)),
            None => format!("<ri:url ri:value=\"{}\" />", escape(&capture.dest)),
        };
        self.out
            .push_str(&format!("<ac:image{alt}>{target}</ac:image>"));
    }

    /// `<details><summary>Title</summary>`, a blank line, the content, a
    /// blank line and `</details>` is an expand; other HTML is storage as
    /// written.
    fn html(&mut self, html: &str) {
        let trimmed = html.trim();
        if let Some(rest) = trimmed.strip_prefix("<details>") {
            let title = rest
                .split_once("<summary>")
                .and_then(|(_, rest)| rest.split_once("</summary>"))
                .map(|(title, _)| decode_entities(title.trim()))
                .unwrap_or_default();
            let parameter = if title.is_empty() {
                String::new()
            } else {
                format!(
                    "<ac:parameter ac:name=\"title\">{}</ac:parameter>",
                    text_escape(&title)
                )
            };
            self.out.push_str(&format!(
                "<ac:structured-macro ac:name=\"expand\">{parameter}<ac:rich-text-body>"
            ));
            self.expands += 1;
        } else if trimmed == "</details>" && self.expands > 0 {
            self.expands -= 1;
            self.out
                .push_str("</ac:rich-text-body></ac:structured-macro>");
        } else {
            self.out.push_str(html.trim_end_matches('\n'));
        }
    }
}

fn alert(kind: BlockQuoteKind) -> &'static str {
    match kind {
        BlockQuoteKind::Note => "info",
        BlockQuoteKind::Tip => "tip",
        BlockQuoteKind::Important => "note",
        BlockQuoteKind::Warning | BlockQuoteKind::Caution => "warning",
    }
}

/// Whether the list starting here is a task list: its first item opens with
/// a task marker.
fn is_task_list(after: &[Event]) -> bool {
    after
        .iter()
        .find(|event| !matches!(event, Event::Start(Tag::Item | Tag::Paragraph)))
        .is_some_and(|event| matches!(event, Event::TaskListMarker(_)))
}

fn code_macro(language: &str, code: &str) -> String {
    let body = format!(
        "<ac:plain-text-body><![CDATA[{}]]></ac:plain-text-body>",
        cdata(code)
    );
    match language {
        "noformat" => {
            format!("<ac:structured-macro ac:name=\"noformat\">{body}</ac:structured-macro>")
        }
        "" => format!("<ac:structured-macro ac:name=\"code\">{body}</ac:structured-macro>"),
        language => format!(
            "<ac:structured-macro ac:name=\"code\"><ac:parameter ac:name=\"language\">{}</ac:parameter>{body}</ac:structured-macro>",
            text_escape(language)
        ),
    }
}

/// `KEY:Title`, the page ref `page get` takes, and not a URL scheme.
fn page_ref(dest: &str) -> Option<(&str, &str)> {
    let (key, title) = dest.split_once(':')?;
    let scheme = [
        "http",
        "https",
        "mailto",
        "ftp",
        "tel",
        "file",
        "data",
        "javascript",
    ];
    let key_like = !key.is_empty()
        && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '~')
        && !scheme.contains(&key.to_ascii_lowercase().as_str());
    (key_like && !title.trim().is_empty() && !title.starts_with("//")).then_some((key, title))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::{Context, to_markdown};
    use crate::storage::parse;

    fn people() -> HashMap<String, String> {
        HashMap::from([("Sam Lee".to_owned(), "557058:sam".to_owned())])
    }

    fn storage(markdown: &str) -> String {
        to_storage(
            markdown,
            &Target {
                space: "ENG",
                people: &people(),
            },
        )
    }

    /// Storage → Markdown → storage.
    fn round_trip(source: &str) -> String {
        let names: HashMap<String, String> =
            people().into_iter().map(|(name, id)| (id, name)).collect();
        let read = to_markdown(
            &parse(source),
            &Context {
                space: "ENG",
                names: &names,
            },
        );
        assert!(read.lossy.is_empty(), "{source}: {:?}", read.lossy);
        storage(&read.text)
    }

    #[test]
    fn every_construct_markdown_carries_comes_back_as_the_same_storage() {
        for source in [
            "<h2>Steps</h2><p>Run <code>make</code>, then <strong>check</strong> the <em>logs</em> &amp; <s>pray</s> \"now\".</p>",
            "<h1>A</h1><h3>B</h3><h6>C</h6>",
            "<ul><li><p>one</p><ul><li><p>nested</p></li></ul></li><li><p>two</p></li></ul><ol start=\"3\"><li><p>three</p></li></ol>",
            "<ol><li><p>first</p></li><li><p>second</p><ol><li><p>deep</p></li></ol></li></ol>",
            "<hr /><blockquote><p>quoted</p><p>twice</p></blockquote>",
            "<ac:structured-macro ac:name=\"code\"><ac:parameter ac:name=\"language\">python</ac:parameter><ac:plain-text-body><![CDATA[def f(a_b):\n    return a_b * 2  # x < y && z]]></ac:plain-text-body></ac:structured-macro>",
            "<ac:structured-macro ac:name=\"code\"><ac:plain-text-body><![CDATA[plain\n]]></ac:plain-text-body></ac:structured-macro>",
            "<ac:structured-macro ac:name=\"noformat\"><ac:plain-text-body><![CDATA[```\nfenced]]]]><![CDATA[>]]></ac:plain-text-body></ac:structured-macro>",
            "<ac:structured-macro ac:name=\"info\"><ac:rich-text-body><p>Note this.</p></ac:rich-text-body></ac:structured-macro><ac:structured-macro ac:name=\"tip\"><ac:rich-text-body><p>A tip.</p></ac:rich-text-body></ac:structured-macro><ac:structured-macro ac:name=\"note\"><ac:rich-text-body><p>Important.</p><ul><li><p>a</p></li></ul></ac:rich-text-body></ac:structured-macro><ac:structured-macro ac:name=\"warning\"><ac:rich-text-body><p>Careful.</p></ac:rich-text-body></ac:structured-macro>",
            "<ac:structured-macro ac:name=\"expand\"><ac:parameter ac:name=\"title\">Details &amp; more</ac:parameter><ac:rich-text-body><p>Hidden.</p></ac:rich-text-body></ac:structured-macro>",
            "<table><tbody><tr><th><p>Step</p></th><th><p>Who</p></th></tr><tr><td><p>a | b</p></td><td><p><strong>Sam</strong></p></td></tr><tr><td><p></p></td><td><p>x</p></td></tr></tbody></table>",
            "<ac:task-list><ac:task><ac:task-status>complete</ac:task-status><ac:task-body>Fix the order</ac:task-body></ac:task><ac:task><ac:task-status>incomplete</ac:task-status><ac:task-body>Retry <strong>once</strong></ac:task-body></ac:task></ac:task-list>",
            "<p>See <ac:link><ri:page ri:content-title=\"Runbook: etl_nightly\" /><ac:plain-text-link-body><![CDATA[the runbook]]></ac:plain-text-link-body></ac:link>, <ac:link><ri:page ri:space-key=\"OPS\" ri:content-title=\"Pager (on call)\" /><ac:plain-text-link-body><![CDATA[Pager (on call)]]></ac:plain-text-link-body></ac:link> and <a href=\"https://contoso.example/a?b=1&amp;c=2\">docs <em>here</em></a>.</p>",
            "<p><ac:link><ri:attachment ri:filename=\"changes v1.4.2.txt\" /><ac:plain-text-link-body><![CDATA[the changes]]></ac:plain-text-link-body></ac:link> <ac:link ac:anchor=\"Rollback\"><ac:plain-text-link-body><![CDATA[rollback]]></ac:plain-text-link-body></ac:link></p>",
            "<p>Ask <ac:link><ri:user ri:account-id=\"557058:sam\" /></ac:link> first <ac:emoticon ac:name=\"tick\" /></p>",
            "<p><ac:image ac:alt=\"rollout\"><ri:attachment ri:filename=\"rollout.png\" /></ac:image><ac:image><ri:url ri:value=\"https://contoso.example/x.png\" /></ac:image></p>",
            "<p>line<br />break H<sub>2</sub>O <u>under</u></p>",
        ] {
            assert_eq!(round_trip(source), source);
        }
    }

    #[test]
    fn markdown_agents_write_becomes_storage() {
        assert_eq!(
            storage(
                "# Rollback\n\nUndo it, then tell @<Sam Lee> and @<Nobody>.\n\n> [!CAUTION]\n> Prod only.\n\n```\nkubectl rollout undo\n```"
            ),
            "<h1>Rollback</h1><p>Undo it, then tell <ac:link><ri:user ri:account-id=\"557058:sam\" /></ac:link> and @&lt;Nobody&gt;.</p><ac:structured-macro ac:name=\"warning\"><ac:rich-text-body><p>Prod only.</p></ac:rich-text-body></ac:structured-macro><ac:structured-macro ac:name=\"code\"><ac:plain-text-body><![CDATA[kubectl rollout undo]]></ac:plain-text-body></ac:structured-macro>"
        );
        assert_eq!(
            storage("- loose\n\n- items\n\n`@<Sam Lee>` stays"),
            "<ul><li><p>loose</p></li><li><p>items</p></li></ul><p><code>@&lt;Sam Lee&gt;</code> stays</p>"
        );
        assert_eq!(
            mentioned("@<Sam Lee>, @<Sam Lee> and \\@<x> and @<Priya Patel>"),
            ["Sam Lee", "Priya Patel"]
        );
    }
}
