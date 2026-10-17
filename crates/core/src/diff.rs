//! `diff`, one comparison for every domain: two rows as each domain's `get`
//! builds them, compared key by key, with the noise of the resource dropped,
//! environment names lined up, texts by lines and secrets by equality only.
//!
//! The answer is the same shape everywhere (`left`, `right`, `same`, `as`,
//! `changes`) and exits 0 either way: differences are the answer, not a
//! failure.

mod lines;
mod names;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;

use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Map, Value};

use crate::ctx::Ctx;
use crate::secret::Secret;
use names::{names, swap};

/// The flags every diff command takes, flattened into its args.
#[derive(Clone, Debug, Default, clap::Args)]
pub struct DiffArgs {
    /// Treat a word on the left as one on the right, A=B (repeatable); none
    /// turns off the names inferred from the two ids
    #[arg(long = "as", value_name = "A=B")]
    pub as_: Vec<String>,
    /// Leave out a path as the changes print it, and all under it (repeatable; * is one segment)
    #[arg(long)]
    pub ignore: Vec<String>,
    /// Keep what this resource's diff leaves out as noise (ids, times, counts)
    #[arg(long)]
    pub all: bool,
    /// Most differences to print
    #[arg(long, default_value_t = 50)]
    pub limit: usize,
    /// Texts compare as they are: whitespace, case and comments count
    #[arg(long)]
    pub exact: bool,
}

/// What a resource's diff knows about its rows.
#[derive(Clone, Copy, Debug)]
pub struct Rules {
    /// Lists matched by a field instead of by position: `("containers",
    /// "name")`; the field's value becomes the path segment.
    pub keys: &'static [(&'static str, &'static str)],
    /// Paths that are noise unless `--all`; a path covers what is under it.
    pub noise: &'static [&'static str],
    /// Line comment markers dropped from texts unless `--exact`; `/*` also
    /// drops block comments.
    pub comments: &'static [&'static str],
    /// Texts compare case-insensitively unless `--exact` (SQL).
    pub fold_case: bool,
    /// Infer environment names from the two ids. Off where the ids name two
    /// runs or versions of one thing, whose differing words are no
    /// environment.
    pub infer: bool,
}

impl Rules {
    /// No keys, no noise, plain texts, names inferred: a start for `..`.
    pub const PLAIN: Self = Self {
        keys: &[],
        noise: &[],
        comments: &[],
        fold_case: false,
        infer: true,
    };
}

/// One side of a diff: its id as the command took it, its row, and the
/// secrets the row leaves out, by the path they would have (`None` where
/// the service never returns the value).
pub struct Side {
    pub id: String,
    pub row: Value,
    pub secrets: BTreeMap<String, Option<Secret>>,
}

impl Side {
    #[must_use]
    pub fn new(id: impl Into<String>, row: Value) -> Self {
        Self {
            id: id.into(),
            row,
            secrets: BTreeMap::new(),
        }
    }
}

/// What every diff command returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Diff {
    /// The two sides' ids.
    pub left: String,
    pub right: String,
    /// No difference, masked values aside.
    pub same: bool,
    /// The environment names treated as one, `staging=prod`.
    #[serde(rename = "as")]
    pub as_: Vec<String>,
    pub changes: Vec<Change>,
}

/// One difference. A path on one side only has nothing on the other.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Change {
    /// Dotted, a list's item named by its key: `containers.api.image`.
    pub path: String,
    pub left: Option<Value>,
    pub right: Option<Value>,
    /// A secret's value: only whether the two are equal is ever printed.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub redacted: bool,
    /// For a secret: equal or not; absent where the service masks it.
    pub same: Option<bool>,
    /// A text's unified hunks (`@@ -40,7 +40,7 @@`, ` kept`, `-left`, `+right`).
    pub lines: Vec<String>,
}

/// A side's `SCOPE:` prefix and the rest (`qa:dbo.orders`), for domains whose
/// scope is a flag. Only a prefix without `/`, so a run id's own `:` stays.
#[must_use]
pub fn scoped(raw: &str) -> (Option<&str>, &str) {
    match raw.split_once(':') {
        Some((scope, rest)) if !scope.is_empty() && !scope.contains('/') && !rest.is_empty() => {
            (Some(scope), rest)
        }
        _ => (None, raw),
    }
}

/// Compares two sides under `rules`, noting the names lined up, how many
/// differences there are and where `--limit` cut.
pub fn diff(ctx: &Ctx, args: &DiffArgs, rules: &Rules, left: Side, right: Side) -> Result<Diff> {
    let pairs = names(args, rules, &left.id, &right.id)?;
    if args.as_.is_empty() && !pairs.is_empty() {
        let shown: Vec<String> = pairs.iter().map(|(a, b)| format!("{a} as {b}")).collect();
        ctx.note(format!(
            "[treating {}; --as A=B to choose]",
            shown.join(", ")
        ));
    }
    let (mut changes, masked) = compare(args, rules, &pairs, &left, &right);
    let differences = changes.len() - masked;
    if masked > 0 {
        ctx.note(format!(
            "[{masked} masked by the service: whether they are the same is unknown]"
        ));
    }
    if changes.len() > args.limit {
        // Masked values are changes but no known difference.
        ctx.note(format!(
            "[{} of {} changes; --limit N]",
            args.limit,
            changes.len()
        ));
        changes.truncate(args.limit);
    } else if differences > 0 {
        // Masked values are no known difference: their own note says so.
        let known = changes
            .iter()
            .filter(|c| !(c.redacted && c.same.is_none() && c.left.is_none() && c.right.is_none()));
        let mut named: Vec<&str> = known.map(|c| c.path.as_str()).take(6).collect();
        if named.len() > 5 {
            named[5] = "…";
        }
        let noun = if differences == 1 {
            "difference"
        } else {
            "differences"
        };
        ctx.note(format!("[{differences} {noun}: {}]", named.join(", ")));
    }
    Ok(Diff {
        left: left.id,
        right: right.id,
        same: differences == 0,
        as_: pairs.iter().map(|(a, b)| format!("{a}={b}")).collect(),
        changes,
    })
}

/// Every difference between the two sides, and how many of them are
/// secrets the service masks.
fn compare(
    args: &DiffArgs,
    rules: &Rules,
    pairs: &[(String, String)],
    left: &Side,
    right: &Side,
) -> (Vec<Change>, usize) {
    let walk = Walk { rules, args, pairs };
    let mut changes = Vec::new();
    walk.value(
        &mut Vec::new(),
        Some(&left.row),
        Some(&right.row),
        &mut changes,
    );
    let mut paths: Vec<&String> = left.secrets.keys().chain(right.secrets.keys()).collect();
    paths.sort();
    paths.dedup();
    let mut masked = 0;
    for path in paths {
        let segments: Vec<String> = path.split('.').map(str::to_owned).collect();
        if walk.skipped(&segments) {
            continue;
        }
        let set = || Some(Value::String("(set)".to_owned()));
        let change = |left, right, same| Change {
            path: path.clone(),
            left,
            right,
            redacted: true,
            same,
            lines: Vec::new(),
        };
        match (left.secrets.get(path), right.secrets.get(path)) {
            (Some(Some(a)), Some(Some(b))) if a == b => {}
            (Some(Some(_)), Some(Some(_))) => changes.push(change(None, None, Some(false))),
            (Some(_), Some(_)) => {
                masked += 1;
                changes.push(change(None, None, None));
            }
            (Some(_), None) => changes.push(change(set(), None, None)),
            (None, _) => changes.push(change(None, set(), None)),
        }
    }
    (changes, masked)
}

struct Walk<'a> {
    rules: &'a Rules,
    args: &'a DiffArgs,
    pairs: &'a [(String, String)],
}

impl Walk<'_> {
    /// Noise (unless `--all`) or `--ignore`d: the path or one above it.
    fn skipped(&self, path: &[String]) -> bool {
        let covers = |pattern: &str| {
            let segments: Vec<&str> = pattern.split('.').collect();
            segments.len() <= path.len()
                && segments
                    .iter()
                    .zip(path)
                    .all(|(want, held)| *want == "*" || want == held)
        };
        (!self.args.all && self.rules.noise.iter().any(|p| covers(p)))
            || self.args.ignore.iter().any(|p| covers(p))
    }

    fn key_of(&self, path: &[String]) -> Option<&'static str> {
        let pattern_matches = |pattern: &str| {
            let segments: Vec<&str> = pattern.split('.').collect();
            segments.len() == path.len()
                && segments
                    .iter()
                    .zip(path)
                    .all(|(want, held)| *want == "*" || want == held)
        };
        self.rules
            .keys
            .iter()
            .find(|(pattern, _)| pattern_matches(pattern))
            .map(|(_, key)| *key)
    }

    /// Two keyed collections' entries paired: each left key with the
    /// right's same key, else with the right's key the names line up with
    /// (`api.staging.contoso.example` with `api.prod.contoso.example`); then
    /// the right's unpaired ones. A pair takes the left's key.
    fn paired<'v>(
        &self,
        a: Vec<(String, &'v Value)>,
        b: Vec<(String, &'v Value)>,
    ) -> Vec<(String, Option<&'v Value>, Option<&'v Value>)> {
        let mut at: BTreeMap<&str, usize> = BTreeMap::new();
        for (index, (key, _)) in b.iter().enumerate().rev() {
            at.insert(key, index);
        }
        let mut used = vec![false; b.len()];
        // The same key first, so `prod_x` on both sides stays one pair
        // however a left `qa_x` would line up.
        let mut found: Vec<Option<usize>> = Vec::with_capacity(a.len());
        for (key, _) in &a {
            let index = at.get(key.as_str()).copied().filter(|index| !used[*index]);
            if let Some(index) = index {
                used[index] = true;
            }
            found.push(index);
        }
        for ((key, _), held) in a.iter().zip(&mut found) {
            if held.is_none()
                && let Some(index) = at.get(swap(key, self.pairs).as_str()).copied()
                && !used[index]
            {
                used[index] = true;
                *held = Some(index);
            }
        }
        let mut out = Vec::with_capacity(a.len() + b.len());
        for ((key, left), held) in a.iter().zip(found) {
            out.push((key.clone(), Some(*left), held.map(|index| b[index].1)));
        }
        for (index, (key, right)) in b.iter().enumerate() {
            if !used[index] {
                out.push((key.clone(), None, Some(*right)));
            }
        }
        out
    }

    fn value<'v>(
        &self,
        path: &mut Vec<String>,
        l: Option<&'v Value>,
        r: Option<&'v Value>,
        out: &mut Vec<Change>,
    ) {
        if self.skipped(path) {
            return;
        }
        let (l, r) = (l.filter(|v| !blank(v)), r.filter(|v| !blank(v)));
        match (l, r) {
            (None, None) => {}
            (Some(Value::Object(a)), Some(Value::Object(b))) => {
                let entries = |map: &'v Map<String, Value>| -> Vec<(String, &'v Value)> {
                    map.iter()
                        .map(|(key, value)| (key.clone(), value))
                        .collect()
                };
                for (key, l, r) in self.paired(entries(a), entries(b)) {
                    path.push(key);
                    self.value(path, l, r, out);
                    path.pop();
                }
            }
            (Some(Value::Array(a)), Some(Value::Array(b))) => match self.key_of(path) {
                Some(field) => {
                    let named = |items: &'v [Value]| -> Vec<(String, &'v Value)> {
                        items
                            .iter()
                            .enumerate()
                            .map(|(at, item)| {
                                // A nested field (`metadata.name`) by its path.
                                let held = field.split('.').fold(item, |value, part| &value[part]);
                                let key = match held {
                                    Value::String(text) => text.clone(),
                                    Value::Null => at.to_string(),
                                    other => other.to_string(),
                                };
                                (key, item)
                            })
                            .collect()
                    };
                    for (key, l, r) in self.paired(named(a), named(b)) {
                        path.push(key);
                        if l.is_some() && r.is_some() {
                            self.value(path, l, r, out);
                        } else {
                            // On one side only, the path names it: its key
                            // field again says nothing.
                            let item = l.or(r).map(|item| without(item, field));
                            let (l, r) = if l.is_some() {
                                (item.as_ref(), None)
                            } else {
                                (None, item.as_ref())
                            };
                            self.value(path, l, r, out);
                        }
                        path.pop();
                    }
                }
                // Plain values (tags, a command's arguments, an index's
                // columns) read as one list: `tags.1` against nothing would
                // say less than the two lists.
                None if a.iter().chain(b).all(plain) => {
                    let same = a.len() == b.len()
                        && a.iter().zip(b).all(|pair| match pair {
                            (Value::String(x), Value::String(y)) => swap(x, self.pairs) == *y,
                            (x, y) => x == y,
                        });
                    if !same {
                        out.push(Change {
                            path: path.join("."),
                            left: l.cloned(),
                            right: r.cloned(),
                            redacted: false,
                            same: None,
                            lines: Vec::new(),
                        });
                    }
                }
                None => {
                    for at in 0..a.len().max(b.len()) {
                        path.push(at.to_string());
                        self.value(path, a.get(at), b.get(at), out);
                        path.pop();
                    }
                }
            },
            (Some(Value::String(a)), Some(Value::String(b))) => self.text(path, a, b, out),
            (Some(a), Some(b)) if a == b => {}
            (l, r) => out.push(Change {
                path: path.join("."),
                left: l.cloned(),
                right: r.cloned(),
                redacted: false,
                same: None,
                lines: Vec::new(),
            }),
        }
    }

    /// Two strings: as JSON when both hold an object or a list, by lines
    /// when either has several, else as they are with the names lined up.
    fn text(&self, path: &mut Vec<String>, a: &str, b: &str, out: &mut Vec<Change>) {
        let json = |text: &str| {
            let trimmed = text.trim_start();
            (trimmed.starts_with('{') || trimmed.starts_with('['))
                .then(|| serde_json::from_str::<Value>(text).ok())
                .flatten()
        };
        if let (Some(x), Some(y)) = (json(a), json(b)) {
            return self.value(path, Some(&x), Some(&y), out);
        }
        let a_swapped = swap(a, self.pairs);
        if a_swapped == b {
            return;
        }
        let change = |left, right, lines| Change {
            path: path.join("."),
            left,
            right,
            redacted: false,
            same: None,
            lines,
        };
        if !a.contains('\n') && !b.contains('\n') {
            out.push(change(
                Some(Value::from(a)),
                Some(Value::from(b)),
                Vec::new(),
            ));
            return;
        }
        let exact = self.args.exact;
        let key = |line: &str| {
            if exact {
                return line.to_owned();
            }
            let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
            if self.rules.fold_case {
                line.to_lowercase()
            } else {
                line
            }
        };
        // Compared with comments blanked (their lines kept, so the two line
        // up) and the names lined up; printed as written.
        let keys = |text: &str| -> Vec<String> {
            blanked(text, self.rules, exact).lines().map(key).collect()
        };
        let (ka, kb) = (keys(&a_swapped), keys(b));
        let significant = |keys: &[String]| -> Vec<String> {
            keys.iter()
                .filter(|l| exact || !l.is_empty())
                .cloned()
                .collect()
        };
        if significant(&ka) == significant(&kb) {
            return;
        }
        let (left, right): (Vec<&str>, Vec<&str>) = (a.lines().collect(), b.lines().collect());
        out.push(change(None, None, lines::unified(&left, &right, ka, kb)));
    }
}

/// `text` with comments blanked to spaces (newlines kept, so its lines line
/// up with the original's), unless `exact`.
fn blanked(text: &str, rules: &Rules, exact: bool) -> String {
    if exact || rules.comments.is_empty() {
        return text.to_owned();
    }
    let block = rules.comments.contains(&"/*");
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    'outer: while !rest.is_empty() {
        if block && rest.starts_with("/*") {
            let end = rest[2..].find("*/").map_or(rest.len(), |at| at + 4);
            out.extend(
                rest[..end]
                    .chars()
                    .map(|c| if c == '\n' { '\n' } else { ' ' }),
            );
            rest = &rest[end..];
            continue;
        }
        for marker in rules.comments.iter().filter(|m| **m != "/*") {
            if rest.starts_with(marker) {
                let end = rest.find('\n').unwrap_or(rest.len());
                rest = &rest[end..];
                continue 'outer;
            }
        }
        let c = rest.chars().next().unwrap_or_default();
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// `item` without its top-level `field`.
fn without(item: &Value, field: &str) -> Value {
    let mut item = item.clone();
    if let Some(map) = item.as_object_mut() {
        map.shift_remove(field);
    }
    item
}

/// Neither an object nor a list.
fn plain(value: &Value) -> bool {
    !matches!(value, Value::Object(_) | Value::Array(_))
}

/// Null, `""`, `[]` and `{}` are left out of every printed row, so a diff
/// treats them as absent: a defaulted empty list is no difference, and an
/// object of nothing but those would print as a change with no sides.
fn blank(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(text) => text.is_empty(),
        Value::Array(items) => items.is_empty(),
        Value::Object(map) => map.values().all(blank),
        _ => false,
    }
}
