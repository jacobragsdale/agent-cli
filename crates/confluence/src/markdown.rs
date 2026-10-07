//! A page's storage as Markdown, converted here rather than by the server:
//! `body-format=markdown` is undocumented and drops images, panel types and
//! expand titles, and ADF exists only on Cloud.
//!
//! What Markdown cannot carry is named in `lossy` (`layout`, `jira ×2`,
//! `inline comment marks ×3`) and shown as a marker such as `⟦toc⟧`, so an
//! update can refuse to overwrite it. `compose` maps the rest back: storage
//! → Markdown → storage gives the same storage for everything not lossy,
//! but dates and emoji other than the classic emoticons, which read as
//! their text.

use std::collections::HashMap;

use crate::storage::{Element, Node, text_of};
use crate::syntax::{
    code_span, collapse_runs, escape_md, fence, guard_start, indent, item, link, quote, target,
    wrap,
};

/// What a body's Markdown needs from outside it.
pub(crate) struct Context<'a> {
    /// The page's space key: a page link with no `ri:space-key` is in it.
    pub(crate) space: &'a str,
    /// Display names by account id, for mentions.
    pub(crate) names: &'a HashMap<String, String>,
}

pub(crate) struct Markdown {
    pub(crate) text: String,
    /// What the Markdown simplified, each once, with a count past one.
    pub(crate) lossy: Vec<String>,
}

/// Macros a reader knows by name; any other is `macro NAME` in `lossy`.
const KNOWN_MACROS: &[&str] = &[
    "jira",
    "toc",
    "status",
    "children",
    "include",
    "excerpt-include",
    "details",
    "anchor",
    "panel",
    "excerpt",
    "attachments",
];

/// Macros that sit between blocks rather than in a line of text.
const BLOCK_MACROS: &[&str] = &[
    "code", "noformat", "info", "tip", "note", "warning", "expand", "panel",
];

pub(crate) fn to_markdown(nodes: &[Node], context: &Context) -> Markdown {
    let mut writer = Writer {
        context,
        lossy: Vec::new(),
    };
    let text = writer.blocks(nodes, false);
    Markdown {
        text,
        lossy: writer
            .lossy
            .into_iter()
            .map(|(what, count)| {
                if count > 1 {
                    format!("{what} \u{d7}{count}")
                } else {
                    what
                }
            })
            .collect(),
    }
}

/// Every account id a body mentions, for one users-bulk call.
pub(crate) fn mentions(nodes: &[Node]) -> Vec<String> {
    let mut found = Vec::new();
    for node in nodes {
        if let Node::Element(element) = node {
            if element.name == "ri:user"
                && let Some(id) = element.attr("ri:account-id")
                && !found.iter().any(|known| known == id)
            {
                found.push(id.to_owned());
            }
            for id in mentions(&element.children) {
                if !found.contains(&id) {
                    found.push(id);
                }
            }
        }
    }
    found
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Paragraph,
    List,
    Other,
}

struct Writer<'a> {
    context: &'a Context<'a>,
    lossy: Vec<(String, usize)>,
}

impl Writer<'_> {
    fn lose(&mut self, what: impl Into<String>) {
        let what = what.into();
        match self.lossy.iter_mut().find(|(known, _)| *known == what) {
            Some((_, count)) => *count += 1,
            None => self.lossy.push((what, 1)),
        }
    }

    /// Blocks joined by a blank line; in a list item (`tight`) a nested list
    /// follows its line directly.
    fn blocks(&mut self, nodes: &[Node], tight: bool) -> String {
        let chunks = self.chunks(nodes);
        let mut out = String::new();
        for (index, (kind, text)) in chunks.iter().enumerate() {
            if index > 0 {
                out.push_str(if tight && *kind == Kind::List {
                    "\n"
                } else {
                    "\n\n"
                });
            }
            out.push_str(text);
        }
        out
    }

    fn chunks(&mut self, nodes: &[Node]) -> Vec<(Kind, String)> {
        let mut chunks = Vec::new();
        let mut run: Vec<&Node> = Vec::new();
        for node in nodes {
            match node {
                Node::Element(element) if is_block(element) => {
                    self.paragraph(&mut run, &mut chunks);
                    self.block(element, &mut chunks);
                }
                Node::Text(text) if run.is_empty() && text.trim().is_empty() => {}
                other => run.push(other),
            }
        }
        self.paragraph(&mut run, &mut chunks);
        chunks
    }

    fn paragraph(&mut self, run: &mut Vec<&Node>, chunks: &mut Vec<(Kind, String)>) {
        let text = self.inline(run.drain(..));
        let text = text.trim();
        if !text.is_empty() {
            chunks.push((Kind::Paragraph, guard_start(text)));
        }
    }

    fn block(&mut self, element: &Element, chunks: &mut Vec<(Kind, String)>) {
        let name = element.name.as_str();
        let chunk = match name {
            "p" => {
                let mut run: Vec<&Node> = element.children.iter().collect();
                return self.paragraph(&mut run, chunks);
            }
            "ul" | "ol" => (Kind::List, self.list(element, name == "ol")),
            "ac:task-list" => (Kind::List, self.tasks(element)),
            "table" => (Kind::Other, self.table(element)),
            "blockquote" => (Kind::Other, quote(&self.blocks(&element.children, false))),
            "pre" => (Kind::Other, fence(&text_of(&element.children), "")),
            "hr" => (Kind::Other, "---".to_owned()),
            "ac:structured-macro" => return self.macro_block(element, chunks),
            "ac:adf-extension" => {
                if let Some(node) = element.child("ac:adf-node") {
                    self.lose(format!("adf {}", node.attr("type").unwrap_or("node")));
                    chunks.extend(self.adf(node));
                }
                return;
            }
            "ac:layout" => {
                self.lose("layout");
                return chunks.extend(self.chunks(&element.children));
            }
            _ => match crate::storage::level(name) {
                Some(level) => {
                    let text = self.inline(&element.children);
                    let text = text.trim();
                    if text.is_empty() {
                        return;
                    }
                    (Kind::Other, format!("{} {text}", "#".repeat(level.into())))
                }
                None => return chunks.extend(self.chunks(&element.children)),
            },
        };
        if !chunk.1.is_empty() {
            chunks.push(chunk);
        }
    }

    fn list(&mut self, list: &Element, ordered: bool) -> String {
        let mut number: u64 = list
            .attr("start")
            .and_then(|start| start.parse().ok())
            .unwrap_or(1);
        let mut items: Vec<String> = Vec::new();
        for child in list.elements() {
            match child.name.as_str() {
                "li" => {
                    let marker = if ordered {
                        format!("{number}. ")
                    } else {
                        "- ".to_owned()
                    };
                    number += 1;
                    let body = self.blocks(&child.children, true);
                    items.push(item(&marker, &body));
                }
                // A list straight inside a list belongs to the item before it.
                "ul" | "ol" => {
                    let nested = self.list(child, child.name == "ol");
                    match items.last_mut() {
                        Some(last) => {
                            last.push('\n');
                            last.push_str(&indent(&nested, "  "));
                        }
                        None => items.push(nested),
                    }
                }
                _ => {}
            }
        }
        items.join("\n")
    }

    fn tasks(&mut self, list: &Element) -> String {
        let mut items = Vec::new();
        for task in list.elements().filter(|task| task.name == "ac:task") {
            let done = task
                .child("ac:task-status")
                .is_some_and(|status| status.text().trim() == "complete");
            let body = task
                .child("ac:task-body")
                .map(|body| self.inline(&body.children))
                .unwrap_or_default();
            let mark = if done { "x" } else { " " };
            items.push(format!("- [{mark}] {}", body.trim()).trim_end().to_owned());
        }
        items.join("\n")
    }

    fn table(&mut self, table: &Element) -> String {
        let mut rows: Vec<Vec<String>> = Vec::new();
        let mut row_elements: Vec<&Element> = Vec::new();
        for child in table.elements() {
            match child.name.as_str() {
                "tr" => row_elements.push(child),
                "thead" | "tbody" | "tfoot" => {
                    row_elements.extend(child.elements().filter(|row| row.name == "tr"));
                }
                _ => {}
            }
        }
        for row in row_elements {
            let mut cells = Vec::new();
            for cell in row
                .elements()
                .filter(|cell| matches!(cell.name.as_str(), "th" | "td"))
            {
                if ["colspan", "rowspan"]
                    .iter()
                    .any(|span| cell.attr(span).is_some_and(|value| value.trim() != "1"))
                {
                    self.lose("merged table cells");
                }
                let chunks = self.chunks(&cell.children);
                if chunks.len() > 1 || chunks.iter().any(|(kind, _)| *kind != Kind::Paragraph) {
                    self.lose("table cell blocks");
                }
                let text: Vec<String> = chunks.into_iter().map(|(_, text)| text).collect();
                cells.push(text.join("<br>").replace('\n', "<br>").replace('|', "\\|"));
            }
            rows.push(cells);
        }
        let width = rows.iter().map(Vec::len).max().unwrap_or(0);
        if width == 0 {
            return String::new();
        }
        let line = |cells: &[String]| {
            let mut padded: Vec<&str> = cells.iter().map(String::as_str).collect();
            padded.resize(width, "");
            format!("| {} |", padded.join(" | "))
        };
        let mut out = vec![line(&rows[0]), line(&vec!["---".to_owned(); width])];
        out.extend(rows[1..].iter().map(|row| line(row)));
        out.join("\n")
    }

    fn macro_block(&mut self, element: &Element, chunks: &mut Vec<(Kind, String)>) {
        let name = macro_name(element);
        let body = element.child("ac:rich-text-body");
        let chunk = match name.as_str() {
            "code" | "noformat" => {
                if element
                    .param("title")
                    .is_some_and(|title| !title.trim().is_empty())
                {
                    self.lose("code title");
                }
                let language = if name == "noformat" {
                    "noformat".to_owned()
                } else {
                    element.param("language").unwrap_or_default()
                };
                let code = element
                    .child("ac:plain-text-body")
                    .map(Element::text)
                    .unwrap_or_default();
                fence(&code, language.trim())
            }
            "info" | "tip" | "note" | "warning" => {
                if element
                    .param("title")
                    .is_some_and(|title| !title.trim().is_empty())
                {
                    self.lose("panel title");
                }
                let kind = match name.as_str() {
                    "info" => "NOTE",
                    "tip" => "TIP",
                    "note" => "IMPORTANT",
                    _ => "WARNING",
                };
                let inside = body
                    .map(|body| self.blocks(&body.children, false))
                    .unwrap_or_default();
                quote(format!("[!{kind}]\n{inside}").trim_end())
            }
            "expand" => {
                let title = element.param("title").unwrap_or_default();
                let inside = body
                    .map(|body| self.blocks(&body.children, false))
                    .unwrap_or_default();
                format!(
                    "<details><summary>{}</summary>\n\n{inside}\n\n</details>",
                    crate::storage::escape(title.trim())
                )
            }
            _ => {
                chunks.push((Kind::Paragraph, self.marker(element, &name)));
                if let Some(body) = body {
                    chunks.extend(self.chunks(&body.children));
                } else if let Some(text) = element.child("ac:plain-text-body") {
                    chunks.push((Kind::Other, fence(&text.text(), "")));
                }
                return;
            }
        };
        chunks.push((Kind::Other, chunk));
    }

    /// `⟦jira PROJ-12⟧`: a macro Markdown cannot carry, named in `lossy`.
    fn marker(&mut self, element: &Element, name: &str) -> String {
        self.lose(if KNOWN_MACROS.contains(&name) {
            name.to_owned()
        } else {
            format!("macro {name}")
        });
        let argument = match name {
            "jira" => element
                .param("key")
                .or_else(|| element.param("jqlQuery").map(|jql| format!("jql {jql}"))),
            "status" => element.param("title"),
            "include" | "excerpt-include" => element
                .elements()
                .find(|param| param.name == "ac:parameter" && param.attr("ac:name") == Some(""))
                .and_then(page_title),
            _ => element.param(""),
        };
        match argument.map(|argument| argument.trim().to_owned()) {
            Some(argument) if !argument.is_empty() => format!("\u{27e6}{name} {argument}\u{27e7}"),
            _ => format!("\u{27e6}{name}\u{27e7}"),
        }
    }

    fn adf(&mut self, node: &Element) -> Vec<(Kind, String)> {
        let mut chunks = Vec::new();
        for child in node.elements() {
            match child.name.as_str() {
                "ac:adf-content" => chunks.extend(self.chunks(&child.children)),
                "ac:adf-node" => {
                    let inner = self.adf(child);
                    if child.attr("type") == Some("decision-item") {
                        let text: Vec<String> = inner.into_iter().map(|(_, text)| text).collect();
                        chunks.push((Kind::List, item("- ", &text.join("\n\n"))));
                    } else {
                        chunks.extend(inner);
                    }
                }
                _ => {}
            }
        }
        chunks
    }

    fn inline<'n>(&mut self, nodes: impl IntoIterator<Item = &'n Node>) -> String {
        let mut out = String::new();
        for node in nodes {
            match node {
                Node::Text(text) => out.push_str(&escape_md(&collapse_runs(text))),
                Node::Cdata(text) => out.push_str(&escape_md(text)),
                Node::Element(element) => {
                    let text = self.inline_element(element);
                    out.push_str(&text);
                }
            }
        }
        out
    }

    fn inline_element(&mut self, element: &Element) -> String {
        let name = element.name.as_str();
        match name {
            "strong" | "b" => wrap(&self.inline(&element.children), "**"),
            "em" | "i" => wrap(&self.inline(&element.children), "*"),
            "s" | "del" | "strike" => wrap(&self.inline(&element.children), "~~"),
            "code" => code_span(&text_of(&element.children)),
            "u" | "sub" | "sup" => format!("<{name}>{}</{name}>", self.inline(&element.children)),
            "br" => "<br>".to_owned(),
            "a" => {
                let text = self.inline(&element.children);
                link(&text, element.attr("href").unwrap_or_default())
            }
            "ac:link" => self.ac_link(element),
            "ac:image" => self.image(element),
            "ac:emoticon" => element
                .attr("ac:emoji-shortname")
                .map(str::to_owned)
                .or_else(|| element.attr("ac:name").map(|name| format!(":{name}:")))
                .unwrap_or_default(),
            "time" => element.attr("datetime").unwrap_or_default().to_owned(),
            "ac:inline-comment-marker" => {
                self.lose("inline comment marks");
                self.inline(&element.children)
            }
            "ac:structured-macro" => {
                let name = macro_name(element);
                self.marker(element, &name)
            }
            "ac:placeholder" => {
                self.lose("placeholder");
                String::new()
            }
            "ac:parameter" => String::new(),
            _ if name.starts_with("ri:") => String::new(),
            _ if name.starts_with("ac:") => {
                self.lose(name.trim_start_matches("ac:").to_owned());
                self.inline(&element.children)
            }
            _ => self.inline(&element.children),
        }
    }

    fn ac_link(&mut self, element: &Element) -> String {
        let body = match (
            element.child("ac:link-body"),
            element.child("ac:plain-text-link-body"),
        ) {
            (Some(body), _) => Some(self.inline(&body.children)),
            (None, Some(body)) => Some(escape_md(&body.text())),
            (None, None) => None,
        }
        .filter(|body| !body.trim().is_empty());
        let anchor = element.attr("ac:anchor");
        let target = element
            .elements()
            .find(|child| child.name.starts_with("ri:"));
        let Some(target) = target else {
            return match anchor {
                Some(anchor) => link(
                    &body.unwrap_or_else(|| escape_md(anchor)),
                    &format!("#{anchor}"),
                ),
                None => body.unwrap_or_default(),
            };
        };
        match target.name.as_str() {
            "ri:page" => {
                if anchor.is_some() {
                    self.lose("link anchor");
                }
                let title = target.attr("ri:content-title").unwrap_or_default();
                let space = target.attr("ri:space-key").unwrap_or(self.context.space);
                link(
                    &body.unwrap_or_else(|| escape_md(title)),
                    &format!("{space}:{title}"),
                )
            }
            "ri:attachment" => {
                if target.elements().next().is_some() {
                    self.lose("link to another page's file");
                }
                let file = target.attr("ri:filename").unwrap_or_default();
                link(
                    &body.unwrap_or_else(|| escape_md(file)),
                    &format!("attachment:{file}"),
                )
            }
            "ri:user" => self.mention(target),
            "ri:url" => {
                let url = target.attr("ri:value").unwrap_or_default();
                link(&body.unwrap_or_else(|| escape_md(url)), url)
            }
            other => {
                self.lose(format!("{} link", other.trim_start_matches("ri:")));
                body.unwrap_or_else(|| {
                    escape_md(
                        target
                            .attr("ri:content-title")
                            .or_else(|| target.attr("ri:space-key"))
                            .unwrap_or_default(),
                    )
                })
            }
        }
    }

    fn mention(&mut self, user: &Element) -> String {
        if let Some(id) = user.attr("ri:account-id") {
            let name = self.context.names.get(id).map_or(id, String::as_str);
            return format!("@<{name}>");
        }
        self.lose("user key mention");
        format!("@<{}>", user.attr("ri:userkey").unwrap_or_default())
    }

    fn image(&mut self, image: &Element) -> String {
        if image.child("ac:caption").is_some() {
            self.lose("image caption");
        }
        let alt = escape_md(image.attr("ac:alt").unwrap_or_default());
        let dest = match image.elements().find(|child| child.name.starts_with("ri:")) {
            Some(target) if target.name == "ri:attachment" => {
                if target.elements().next().is_some() {
                    self.lose("image from another page");
                }
                format!(
                    "attachment:{}",
                    target.attr("ri:filename").unwrap_or_default()
                )
            }
            Some(target) if target.name == "ri:url" => {
                target.attr("ri:value").unwrap_or_default().to_owned()
            }
            _ => {
                self.lose("image");
                return alt;
            }
        };
        format!("![{alt}]{}", target(&dest))
    }
}

fn macro_name(element: &Element) -> String {
    element
        .attr("ac:name")
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn page_title(element: &Element) -> Option<String> {
    let link = element.child("ac:link")?;
    let page = link
        .elements()
        .find(|child| child.name.starts_with("ri:"))?;
    page.attr("ri:content-title").map(str::to_owned)
}

fn is_block(element: &Element) -> bool {
    match element.name.as_str() {
        "p" | "ul" | "ol" | "table" | "blockquote" | "pre" | "hr" | "div" | "section"
        | "ac:task-list" | "ac:layout" | "ac:layout-section" | "ac:layout-cell"
        | "ac:adf-extension" | "ac:rich-text-body" => true,
        "ac:structured-macro" => {
            BLOCK_MACROS.contains(&macro_name(element).as_str())
                || element.child("ac:rich-text-body").is_some()
                || element.child("ac:plain-text-body").is_some()
        }
        name => crate::storage::level(name).is_some(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::parse;

    pub(crate) fn md(storage: &str) -> Markdown {
        let names = HashMap::from([("557058:sam".to_owned(), "Sam Lee".to_owned())]);
        to_markdown(
            &parse(storage),
            &Context {
                space: "ENG",
                names: &names,
            },
        )
    }

    #[test]
    fn plain_html_reads_as_markdown() {
        let read = md(
            "<h2>Steps</h2><p>Run <code>make</code>, then <strong>check</strong> the <em>logs</em> &amp; <s>pray</s>.</p><ul><li><p>one</p><ul><li><p>nested</p></li></ul></li><li><p>two</p></li></ul><ol start=\"3\"><li><p>three</p></li></ol><hr /><blockquote><p>quoted</p></blockquote>",
        );
        assert_eq!(
            read.text,
            "## Steps\n\nRun `make`, then **check** the *logs* & ~~pray~~.\n\n- one\n  - nested\n- two\n\n3. three\n\n---\n\n> quoted"
        );
        assert!(read.lossy.is_empty());
    }

    #[test]
    fn macros_read_as_fences_alerts_details_and_markers() {
        let read = md(concat!(
            "<ac:structured-macro ac:name=\"code\"><ac:parameter ac:name=\"language\">bash</ac:parameter><ac:plain-text-body><![CDATA[echo \"a < b\"]]></ac:plain-text-body></ac:structured-macro>",
            "<ac:structured-macro ac:name=\"warning\"><ac:rich-text-body><p>Careful.</p></ac:rich-text-body></ac:structured-macro>",
            "<ac:structured-macro ac:name=\"expand\"><ac:parameter ac:name=\"title\">More</ac:parameter><ac:rich-text-body><p>Hidden.</p></ac:rich-text-body></ac:structured-macro>",
            "<p>Ticket <ac:structured-macro ac:name=\"jira\"><ac:parameter ac:name=\"key\">OPS-12</ac:parameter></ac:structured-macro> is <ac:structured-macro ac:name=\"status\"><ac:parameter ac:name=\"title\">DONE</ac:parameter></ac:structured-macro></p>",
            "<ac:structured-macro ac:name=\"toc\" />",
            "<ac:structured-macro ac:name=\"drawio\"><ac:parameter ac:name=\"diagramName\">arch</ac:parameter></ac:structured-macro>",
            "<ac:structured-macro ac:name=\"jira\"><ac:parameter ac:name=\"key\">OPS-13</ac:parameter></ac:structured-macro>",
        ));
        assert_eq!(
            read.text,
            "```bash\necho \"a < b\"\n```\n\n> [!WARNING]\n> Careful.\n\n<details><summary>More</summary>\n\nHidden.\n\n</details>\n\nTicket \u{27e6}jira OPS-12\u{27e7} is \u{27e6}status DONE\u{27e7}\n\n\u{27e6}toc\u{27e7}\u{27e6}drawio\u{27e7}\u{27e6}jira OPS-13\u{27e7}"
        );
        assert_eq!(
            read.lossy,
            ["jira \u{d7}2", "status", "toc", "macro drawio"]
        );
    }

    #[test]
    fn links_mentions_images_and_tasks_read_as_refs() {
        let read = md(concat!(
            "<p><ac:link><ri:page ri:content-title=\"Runbook: etl_nightly\" /></ac:link> and ",
            "<ac:link><ri:page ri:space-key=\"OPS\" ri:content-title=\"Pager\" /><ac:plain-text-link-body><![CDATA[the pager]]></ac:plain-text-link-body></ac:link>, ",
            "ask <ac:link><ri:user ri:account-id=\"557058:sam\" /></ac:link> or <ac:link><ri:user ri:account-id=\"557058:who\" /></ac:link>, ",
            "<a href=\"https://contoso.example/x\">docs</a> <ac:link><ri:attachment ri:filename=\"changes v2.txt\" /></ac:link></p>",
            "<ac:image ac:alt=\"rollout\"><ri:attachment ri:filename=\"rollout.png\" /></ac:image>",
            "<ac:task-list><ac:task><ac:task-id>1</ac:task-id><ac:task-status>complete</ac:task-status><ac:task-body>Fix the order</ac:task-body></ac:task><ac:task><ac:task-status>incomplete</ac:task-status><ac:task-body>Retry</ac:task-body></ac:task></ac:task-list>",
        ));
        assert_eq!(
            read.text,
            "[Runbook: etl_nightly](<ENG:Runbook: etl_nightly>) and [the pager](OPS:Pager), ask @<Sam Lee> or @<557058:who>, [docs](https://contoso.example/x) [changes v2.txt](<attachment:changes v2.txt>)\n\n![rollout](attachment:rollout.png)\n\n- [x] Fix the order\n- [ ] Retry"
        );
        assert!(read.lossy.is_empty(), "{:?}", read.lossy);
    }

    #[test]
    fn tables_layouts_and_comment_marks_are_named_in_lossy() {
        let read = md(concat!(
            "<table><tbody><tr><th><p>Step</p></th><th><p>Who</p></th></tr><tr><td><p>a|b</p></td><td><p>one</p><p>two</p></td></tr></tbody></table>",
            "<ac:layout><ac:layout-section ac:type=\"two_equal\"><ac:layout-cell><p>left <ac:inline-comment-marker ac:ref=\"r1\">marked</ac:inline-comment-marker></p></ac:layout-cell><ac:layout-cell><p>right</p></ac:layout-cell></ac:layout-section></ac:layout>",
        ));
        assert_eq!(
            read.text,
            "| Step | Who |\n| --- | --- |\n| a\\|b | one<br>two |\n\nleft marked\n\nright"
        );
        assert_eq!(
            read.lossy,
            ["table cell blocks", "layout", "inline comment marks"]
        );
    }

    #[test]
    fn text_that_looks_like_markdown_is_escaped() {
        assert_eq!(
            md("<p># not a heading *or* [link] my_var _x_ a&lt;b &amp;amp; 1. ok</p><p>1. not a list</p>").text,
            "\\# not a heading \\*or\\* \\[link\\] my_var \\_x\\_ a\\<b \\&amp; 1. ok\n\n1\\. not a list"
        );
        assert_eq!(code_span("a`b"), "``a`b``");
        assert_eq!(fence("```\nx", ""), "````\n```\nx\n````");
    }

    #[test]
    fn mentions_are_listed_once() {
        let nodes = parse(
            "<p><ac:link><ri:user ri:account-id=\"a\" /></ac:link><ac:link><ri:user ri:account-id=\"a\" /></ac:link><ac:link><ri:user ri:account-id=\"b\" /></ac:link></p>",
        );
        assert_eq!(mentions(&nodes), ["a", "b"]);
    }
}
