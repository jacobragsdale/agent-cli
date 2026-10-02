//! What an agent writes, made ready to send: a comment's body as it was
//! given, and the mentions in it resolved to the people they name.
//!
//! A mention is `@<Name or address>` or a bare `@name@contoso.com`, outside
//! code. Each is resolved to one person before anything is sent, then written
//! as the anchor a work item's HTML notifies with, or the `@<id>` a pull
//! request's Markdown does.

use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;

use agent_cli_core::{Ctx, Failure};
use anyhow::Result;

use crate::client::{Ado, Person};
use crate::markdown::{backticks, escape, escape_attribute, markdown_to_html};

/// Stands in for the `i`th mention while the Markdown around it is rendered,
/// so escaping and inline markup never touch it: private-use characters.
fn held(i: usize) -> String {
    format!("\u{E000}{i}\u{E001}")
}

/// Markdown as the HTML Azure DevOps stores (work item fields and comments),
/// each mention the anchor that notifies the person.
pub(crate) fn rich_text(ctx: &Ctx, ado: &Ado, markdown: &str) -> Result<String> {
    let markdown = markdown.replace(['\u{E000}', '\u{E001}'], "");
    let people = mentioned(ctx, ado, &markdown)?;
    let mut html = markdown_to_html(&splice(&markdown, &people, |i, _| held(i)));
    for (i, (_, person)) in people.iter().enumerate() {
        let anchor = format!(
            "<a href=\"#\" data-vss-mention=\"version:2.0,{}\">@{}</a>",
            escape_attribute(&person.id),
            escape(&person.name)
        );
        html = html.replacen(&held(i), &anchor, 1);
    }
    Ok(html)
}

/// Markdown as a pull request comment stores it, each mention `@<id>`, the
/// form that notifies the person there.
pub(crate) fn with_mentions(ctx: &Ctx, ado: &Ado, markdown: &str) -> Result<String> {
    let people = mentioned(ctx, ado, markdown)?;
    Ok(splice(markdown, &people, |_, person| {
        format!("@<{}>", person.id)
    }))
}

/// `markdown` with each mention's range replaced by `with` of it.
fn splice(
    markdown: &str,
    people: &[(Range<usize>, Person)],
    with: impl Fn(usize, &Person) -> String,
) -> String {
    let mut out = String::with_capacity(markdown.len());
    let mut from = 0;
    for (i, (range, person)) in people.iter().enumerate() {
        out.push_str(&markdown[from..range.start]);
        out.push_str(&with(i, person));
        from = range.end;
    }
    out.push_str(&markdown[from..]);
    out
}

/// The person each mention in `markdown` names, asked once per name: nobody
/// is exit 4 and several are exit 2, before anything is sent.
fn mentioned(ctx: &Ctx, ado: &Ado, markdown: &str) -> Result<Vec<(Range<usize>, Person)>> {
    let mut asked: HashMap<String, Person> = HashMap::new();
    let mut people = Vec::new();
    for (range, who) in mentions(markdown) {
        let key = who.to_lowercase();
        let person = match asked.get(&key) {
            Some(person) => person.clone(),
            None => {
                let person = ado.person(ctx, &who)?;
                asked.insert(key, person.clone());
                person
            }
        };
        people.push((range, person));
    }
    Ok(people)
}

/// Where `markdown` mentions someone, and who: `@<Name or address>`, or a
/// bare `@name@contoso.com`. Fenced blocks and code spans are code, so an
/// address in a pasted log is not a mention; nor is a plain address
/// (`jane@contoso.com`), nor `@<GUID>`, which already is one in Markdown.
fn mentions(markdown: &str) -> Vec<(Range<usize>, String)> {
    let mut found = Vec::new();
    let mut fence = 0;
    let mut start = 0;
    for line in markdown.split_inclusive('\n') {
        let offset = start;
        start += line.len();
        let run = backticks(line);
        if fence > 0 {
            if run >= fence && line.trim_start()[run..].trim().is_empty() {
                fence = 0;
            }
            continue;
        }
        if run >= 3 {
            fence = run;
            continue;
        }
        let mut at = 0;
        while let Some(index) = line[at..].find(['`', '@']) {
            let index = at + index;
            at = index + 1;
            if line[index..].starts_with('`') {
                if let Some(end) = line[at..].find('`') {
                    at += end + 1;
                }
                continue;
            }
            let glued = line[..index]
                .chars()
                .next_back()
                .is_some_and(|before| before.is_alphanumeric() || "._-+".contains(before));
            if glued {
                continue;
            }
            if let Some((length, who)) = mention_at(&line[index..]) {
                found.push((offset + index..offset + index + length, who));
                at = index + length;
            }
        }
    }
    found
}

/// The mention `rest` (which starts at an `@`) begins with: its length and
/// who it names.
fn mention_at(rest: &str) -> Option<(usize, String)> {
    let after = &rest[1..];
    if let Some(inner) = after.strip_prefix('<') {
        let end = inner.find(['>', '<', '\n'])?;
        let who = inner[..end].trim();
        let guid = who.len() == 36 && who.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
        if !inner[end..].starts_with('>') || who.is_empty() || guid {
            return None;
        }
        return Some((end + 3, who.to_owned()));
    }
    let length = after
        .find(|c: char| !(c.is_ascii_alphanumeric() || "._%+-@".contains(c)))
        .unwrap_or(after.len());
    // A sentence's full stop is not part of the address it ends on.
    let address = after[..length].trim_end_matches(['.', '-']);
    let (local, domain) = address.split_once('@')?;
    let valid = !local.is_empty()
        && !domain.contains('@')
        && domain.contains('.')
        && !domain.starts_with('.')
        && domain.ends_with(|c: char| c.is_ascii_alphabetic());
    valid.then(|| (address.len() + 1, address.to_owned()))
}

/// The most one comment may carry. Past this it is a log, and the part worth
/// reading is at one end of it rather than spread over the whole.
pub(crate) const COMMENT_LIMIT: usize = 64 * 1024;

/// What one comment says, and whether it came down a pipe.
///
/// Piped text is program output (a test tail, a log), so it is posted as a
/// fenced block, which is what keeps its columns lined up. Text typed as the
/// argument or read from `--text-file` is Markdown, as written.
pub(crate) struct CommentBody {
    text: String,
    fenced: bool,
}

impl CommentBody {
    /// The argument as typed, stdin when it is `-`, or `--text-file`.
    pub(crate) fn read(ctx: &Ctx, text: Option<&str>, file: Option<&Path>) -> Result<Self> {
        let body = ctx
            .long_text("text", text, file, Some(COMMENT_LIMIT))?
            .filter(|body| !body.text.trim().is_empty())
            .ok_or_else(|| Failure::usage("a comment cannot be empty"))?;
        Ok(Self {
            text: body.text,
            fenced: body.piped,
        })
    }

    /// As Markdown, before its mentions are resolved. A fence is one backtick
    /// longer than any run inside, so a log that quotes a code block cannot
    /// close it early.
    pub(crate) fn markdown(&self) -> String {
        if !self.fenced {
            return self.text.clone();
        }
        let longest = self
            .text
            .split(|held| held != '`')
            .map(str::len)
            .max()
            .unwrap_or(0);
        let fence = "`".repeat(longest.max(2) + 1);
        format!("{fence}\n{}\n{fence}", self.text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_piped_comment_is_a_fence_longer_than_any_run_inside() {
        let piped = |text: &str| CommentBody {
            text: text.to_owned(),
            fenced: true,
        };
        let body = piped("ok 1\n```inner```");
        assert_eq!(body.markdown(), "````\nok 1\n```inner```\n````");
        assert_eq!(
            markdown_to_html(&body.markdown()),
            "<pre>ok 1\n```inner```</pre>"
        );
        let typed = CommentBody {
            fenced: false,
            ..piped("Fixed in **!17**")
        };
        assert_eq!(typed.markdown(), "Fixed in **!17**");
    }

    #[test]
    fn mentions_are_found_outside_code_and_never_in_a_plain_address_or_a_guid() {
        let text = "Hi @<Sam Lee>, @sam@contoso.com. Not jane@contoso.com, `@<x>`, @<6a2b1c3d-0000-0000-0000-00000000abcd> or\n```\n@<Log Line> ops@contoso.com\n```\n@<Ann> again";
        let found: Vec<(&str, String)> = mentions(text)
            .into_iter()
            .map(|(range, who)| (&text[range], who))
            .collect();
        assert_eq!(
            found,
            [
                ("@<Sam Lee>", "Sam Lee".to_owned()),
                ("@sam@contoso.com", "sam@contoso.com".to_owned()),
                ("@<Ann>", "Ann".to_owned()),
            ]
        );
    }
}
