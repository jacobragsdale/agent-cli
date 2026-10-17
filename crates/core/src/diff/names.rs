//! The environment names a diff lines up: from `--as`, or inferred from the
//! two ids, and swapped into the left side's texts and keys.

use anyhow::Result;

use super::{DiffArgs, Rules};
use crate::error::Failure;

/// The words treated as one: `--as` pairs, `none`, or those inferred from
/// the ids (tokens split at non-alphanumerics, differing in place).
pub(super) fn names(
    args: &DiffArgs,
    rules: &Rules,
    left: &str,
    right: &str,
) -> Result<Vec<(String, String)>> {
    if args.as_.iter().any(|held| held == "none") {
        return Ok(Vec::new());
    }
    if !args.as_.is_empty() {
        return args
            .as_
            .iter()
            .map(|held| match held.split_once('=') {
                Some((a, b)) if !a.is_empty() && !b.is_empty() => Ok((a.to_owned(), b.to_owned())),
                _ => Err(Failure::usage(format!("--as {held:?} is not A=B (or none)")).into()),
            })
            .collect();
    }
    if !rules.infer {
        return Ok(Vec::new());
    }
    let tokens = |id: &str| -> Vec<String> {
        id.split(|c: char| !c.is_alphanumeric())
            .filter(|token| !token.is_empty())
            .map(str::to_owned)
            .collect()
    };
    let (a, b) = (tokens(left), tokens(right));
    if a.len() != b.len() {
        return Ok(Vec::new());
    }
    let numeric = |token: &str| token.chars().all(|c| c.is_ascii_digit());
    let mut pairs: Vec<(String, String)> = Vec::new();
    for (x, y) in a.into_iter().zip(b) {
        // `qa` and Oracle's `QA` are one name: `swap` matches either case.
        if !x.eq_ignore_ascii_case(&y)
            && !(numeric(&x) && numeric(&y))
            && !pairs.iter().any(|(held, _)| held.eq_ignore_ascii_case(&x))
        {
            pairs.push((x, y));
        }
    }
    Ok(pairs)
}

/// `text` with each pair's left word replaced by its right where it stands
/// alone: `-qa.`, `_qa`, `/qa/`, never inside `quality`. The word matches in
/// any ASCII case, and an upper-case one (`SHOP_QA`) becomes `SHOP_PROD`.
pub(super) fn swap(text: &str, pairs: &[(String, String)]) -> String {
    let mut text = text.to_owned();
    for (from, to) in pairs {
        let needle = from.to_ascii_lowercase();
        let mut out = String::with_capacity(text.len());
        let mut rest = text.as_str();
        let mut before: Option<char> = None;
        // ASCII lower-casing keeps every byte offset, so `at` indexes `rest`.
        while let Some(at) = rest.to_ascii_lowercase().find(&needle) {
            let found = &rest[at..at + from.len()];
            let prior = rest[..at].chars().next_back().or(before);
            let after = rest[at + from.len()..].chars().next();
            let alone = |c: Option<char>| c.is_none_or(|c| !c.is_alphanumeric());
            out.push_str(&rest[..at]);
            if !alone(prior) || !alone(after) {
                out.push_str(found);
            } else if found != from && !found.chars().any(char::is_lowercase) {
                out.push_str(&to.to_uppercase());
            } else {
                out.push_str(to);
            }
            before = found.chars().next_back();
            rest = &rest[at + from.len()..];
        }
        out.push_str(rest);
        text = out;
    }
    text
}
