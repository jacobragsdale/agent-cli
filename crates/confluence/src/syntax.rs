//! Markdown's syntax as both directions write it: fences, code spans,
//! links, list items and quotes, and the escapes that keep text from
//! reading as markup.

/// `- body`, its later lines indented under the marker.
pub(crate) fn item(marker: &str, body: &str) -> String {
    let body = indent(body, &" ".repeat(marker.len()));
    format!("{marker}{}", body.trim_start())
        .trim_end()
        .to_owned()
}

pub(crate) fn indent(text: &str, by: &str) -> String {
    text.lines()
        .map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("{by}{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn quote(text: &str) -> String {
    text.lines()
        .map(|line| {
            if line.is_empty() {
                ">".to_owned()
            } else {
                format!("> {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A fenced block longer than any fence the code holds.
pub(crate) fn fence(code: &str, info: &str) -> String {
    let longest = code
        .lines()
        .map(|line| line.trim_start().chars().take_while(|c| *c == '`').count())
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    format!("{fence}{info}\n{code}\n{fence}")
}

pub(crate) fn code_span(code: &str) -> String {
    let code = collapse_runs(code);
    let mut longest = 0;
    let mut run = 0;
    for c in code.chars() {
        run = if c == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    let ticks = "`".repeat(longest + 1);
    let pad = if code.starts_with('`') || code.ends_with('`') {
        " "
    } else {
        ""
    };
    format!("{ticks}{pad}{code}{pad}{ticks}")
}

/// `**text**` with the text's edge spaces outside the markers, where
/// Markdown needs them.
pub(crate) fn wrap(text: &str, marker: &str) -> String {
    let inner = text.trim();
    if inner.is_empty() {
        return text.to_owned();
    }
    let lead = &text[..text.len() - text.trim_start().len()];
    let trail = &text[text.trim_end().len()..];
    format!("{lead}{marker}{inner}{marker}{trail}")
}

pub(crate) fn link(text: &str, dest: &str) -> String {
    let text = if text.trim().is_empty() {
        escape_md(dest)
    } else {
        text.to_owned()
    };
    format!("[{text}]{}", target(dest))
}

/// `(dest)`, or `(<dest>)` when it holds what a bare destination cannot.
pub(crate) fn target(dest: &str) -> String {
    if !dest.is_empty() && !dest.contains([' ', '(', ')', '<', '>', '\\']) {
        return format!("({dest})");
    }
    let escaped = dest
        .replace('\\', "\\\\")
        .replace('<', "\\<")
        .replace('>', "\\>");
    format!("(<{escaped}>)")
}

/// Runs of ASCII whitespace as one space, as HTML renders text.
pub(crate) fn collapse_runs(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for c in text.chars() {
        if c.is_ascii_whitespace() {
            if !space {
                out.push(' ');
            }
            space = true;
        } else {
            out.push(c);
            space = false;
        }
    }
    out
}

/// Text with what Markdown would read as markup escaped: emphasis, code,
/// links, strikes, HTML-looking `<`, entity-looking `&`, and an `_` that
/// is not inside a word (`snake_case` stays as it is).
pub(crate) fn escape_md(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    for (at, c) in chars.iter().enumerate() {
        let next = chars.get(at + 1).copied();
        let word = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
        let escape = match c {
            '\\' | '`' | '*' | '[' | ']' | '~' => true,
            '_' => !(at > 0 && word(Some(chars[at - 1])) && word(next)),
            '<' => next.is_some_and(|n| n.is_ascii_alphabetic() || matches!(n, '/' | '!' | '?')),
            '&' => entity_like(&chars[at + 1..]),
            _ => false,
        };
        if escape {
            out.push('\\');
        }
        out.push(*c);
    }
    out
}

fn entity_like(rest: &[char]) -> bool {
    let body: String = rest.iter().take(12).take_while(|c| **c != ';').collect();
    rest.get(body.chars().count()) == Some(&';')
        && !body.is_empty()
        && (body.chars().all(|c| c.is_ascii_alphanumeric())
            || body.strip_prefix('#').is_some_and(|n| !n.is_empty()))
}

/// A paragraph whose first characters would open a heading, a quote, a list
/// or a rule, escaped.
pub(crate) fn guard_start(text: &str) -> String {
    if text.starts_with(['#', '>', '-', '+', '=']) {
        return format!("\\{text}");
    }
    let digits = text.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 && text[digits..].starts_with(['.', ')']) {
        return format!("{}\\{}", &text[..digits], &text[digits..]);
    }
    text.to_owned()
}

/// The emoticons storage names that Markdown writes as `:name:`.
const EMOTICONS: &[&str] = &[
    "smile",
    "sad",
    "cheeky",
    "laugh",
    "wink",
    "thumbs-up",
    "thumbs-down",
    "information",
    "tick",
    "cross",
    "warning",
    "plus",
    "minus",
    "question",
    "light-on",
    "light-off",
    "yellow-star",
    "red-star",
    "green-star",
    "blue-star",
    "heart",
    "broken-heart",
];

/// Text as storage holds it between tags: quotes stay as they are.
pub(crate) fn text_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// A CDATA section cannot hold `]]>`; it is split across two.
pub(crate) fn cdata(text: &str) -> String {
    text.replace("]]>", "]]]]><![CDATA[>")
}

pub(crate) fn emoticons(escaped: &str) -> String {
    if !escaped.contains(':') {
        return escaped.to_owned();
    }
    let mut out = escaped.to_owned();
    for name in EMOTICONS {
        out = out.replace(
            &format!(":{name}:"),
            &format!("<ac:emoticon ac:name=\"{name}\" />"),
        );
    }
    out
}
