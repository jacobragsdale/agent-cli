//! What reaches stdout, and how much of it.
//!
//! Ported from api-cli's runtime, where trials showed the savings: field
//! selection first (`--fields`), then nulls and empties dropped, compact JSON
//! when piped. A payload over 12 KB is cut structurally so what prints is
//! still valid JSON, and the whole of it is saved to a temp file that stderr
//! names along with the fields worth selecting.

use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::{Map, Value, json};

use crate::ctx::Globals;

pub(crate) const GUARD: usize = 12_000;
/// Fields worth suggesting first for `--fields`.
const PREFERRED: [&str; 9] = [
    "id", "name", "title", "state", "status", "number", "type", "key", "version",
];

/// Prints a command's result the way every command prints: projected,
/// concise, compact when piped, guarded at [`GUARD`] bytes. `keep_tail` cuts
/// a long text payload from the front (logs) instead of the back.
pub(crate) fn emit(
    mut value: Value,
    globals: &Globals,
    keep_tail: bool,
    out: &mut dyn Write,
    err: &mut dyn Write,
    tty: bool,
) -> Result<()> {
    if let Some(fields) = &globals.fields {
        let paths: Vec<Vec<&str>> = fields
            .split(',')
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(|path| path.split('.').collect())
            .collect();
        let projected = project(&value, &paths);
        let kept = concise(projected.clone());
        let nothing = is_blank(&kept)
            || kept
                .as_array()
                .is_some_and(|items| items.iter().all(|item| *item == json!({})));
        if nothing {
            let available = leaf_paths(&value, "", 0);
            writeln!(
                err,
                "[--fields matched nothing. Available: {}]",
                available[..available.len().min(30)].join(",")
            )?;
        }
        value = projected;
    }
    if !globals.raw {
        value = concise(value);
    }
    if let Some(path) = &globals.output {
        return save(&value, path, out);
    }
    let text = dumps(&value, tty);
    if globals.raw || globals.fields.is_some() || tty || text.len() <= GUARD {
        writeln!(out, "{text}")?;
        return Ok(());
    }
    if value.get("text").is_some_and(Value::is_string) {
        return guard_text(value, &text, keep_tail, out, err);
    }
    guard_list(&value, &text, out, err)
}

pub(crate) fn dumps(value: &Value, pretty: bool) -> String {
    if pretty {
        serde_json::to_string_pretty(value).unwrap_or_default()
    } else {
        value.to_string()
    }
}

/// Only the dotted `paths`; lists are mapped through.
fn project(value: &Value, paths: &[Vec<&str>]) -> Value {
    match value {
        Value::Array(items) => {
            Value::Array(items.iter().map(|item| project(item, paths)).collect())
        }
        Value::Object(map) => {
            let mut heads: Vec<(&str, Vec<Vec<&str>>)> = Vec::new();
            for path in paths {
                let Some((head, rest)) = path.split_first() else {
                    continue;
                };
                match heads.iter_mut().find(|(name, _)| name == head) {
                    Some((_, rests)) => rests.push(rest.to_vec()),
                    None => heads.push((head, vec![rest.to_vec()])),
                }
            }
            let mut kept = Map::new();
            for (head, rests) in heads {
                let Some(child) = map.get(head) else {
                    continue;
                };
                let whole = rests.iter().any(Vec::is_empty);
                let child = if whole {
                    child.clone()
                } else {
                    project(child, &rests)
                };
                kept.insert(head.to_owned(), child);
            }
            Value::Object(kept)
        }
        other => other.clone(),
    }
}

/// Nulls and empty strings, lists and objects removed: they cost tokens and
/// carry no answer.
fn concise(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, value)| (key, concise(value)))
                .filter(|(_, value)| !is_blank(value))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(concise).collect()),
        other => other,
    }
}

fn is_blank(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(text) => text.is_empty(),
        Value::Array(items) => items.is_empty(),
        Value::Object(map) => map.is_empty(),
        _ => false,
    }
}

/// Dotted paths to every leaf, through the first item of lists, three levels deep.
fn leaf_paths(value: &Value, prefix: &str, depth: usize) -> Vec<String> {
    match value {
        Value::Array(items) => items
            .first()
            .map(|first| leaf_paths(first, prefix, depth))
            .unwrap_or_default(),
        Value::Object(map) if depth <= 2 => map
            .iter()
            .flat_map(|(key, child)| {
                let nested = matches!(child, Value::Object(m) if !m.is_empty())
                    || matches!(child, Value::Array(a) if !a.is_empty());
                if nested {
                    leaf_paths(child, &format!("{prefix}{key}."), depth + 1)
                } else {
                    vec![format!("{prefix}{key}")]
                }
            })
            .collect(),
        _ if prefix.is_empty() => Vec::new(),
        _ => vec![prefix.trim_end_matches('.').to_owned()],
    }
}

/// The largest list in the top two levels: an envelope's payload.
fn biggest_list<'a>(value: &'a Value, path: &[String]) -> Option<(Vec<String>, &'a Vec<Value>)> {
    match value {
        Value::Array(items) => Some((path.to_vec(), items)),
        Value::Object(map) if path.len() < 2 => map
            .iter()
            .filter_map(|(key, child)| {
                let mut deeper = path.to_vec();
                deeper.push(key.clone());
                biggest_list(child, &deeper)
            })
            .max_by_key(|(_, items)| Value::Array((*items).clone()).to_string().len()),
        _ => None,
    }
}

fn save_temp(content: &str, suffix: &str) -> Result<String> {
    let file = tempfile::Builder::new()
        .prefix("agent-cli-")
        .suffix(suffix)
        .tempfile()
        .context("cannot create a temp file for the full output")?;
    let (mut file, path) = file.keep().context("cannot keep the temp file")?;
    file.write_all(content.as_bytes())?;
    Ok(path.display().to_string())
}

/// Keeps a prefix of the largest list, prints valid JSON, and says where the
/// rest is.
fn guard_list(value: &Value, text: &str, out: &mut dyn Write, err: &mut dyn Write) -> Result<()> {
    let saved = save_temp(text, ".json")?;
    let found = biggest_list(value, &[]).filter(|(_, items)| !items.is_empty());
    let Some((at, items)) = found else {
        let fields = value
            .as_object()
            .map(|map| map.keys().cloned().collect::<Vec<_>>().join(","))
            .unwrap_or_default();
        writeln!(
            out,
            "{}",
            json!({"truncated": true, "bytes": text.len(), "top_level_fields": fields})
        )?;
        writeln!(
            err,
            "[output is {} KB; full JSON: {saved}. Narrow with --fields, or --raw for everything]",
            text.len() / 1024
        )?;
        return Ok(());
    };
    let mut size = text.len() - Value::Array(items.clone()).to_string().len();
    let mut kept = Vec::new();
    for item in items {
        size += item.to_string().len() + 1;
        if size > GUARD && !kept.is_empty() {
            break;
        }
        kept.push(item.clone());
    }
    let shown = if at.is_empty() {
        Value::Array(kept.clone())
    } else {
        let mut shown = value.clone();
        let mut node = &mut shown;
        for key in &at[..at.len() - 1] {
            node = &mut node[key.as_str()];
        }
        node[at[at.len() - 1].as_str()] = Value::Array(kept.clone());
        shown
    };
    writeln!(out, "{shown}")?;
    let prefix = if at.is_empty() {
        String::new()
    } else {
        format!("{}.", at.join("."))
    };
    let paths: Vec<String> = leaf_paths(&items[0], "", 0)
        .into_iter()
        .map(|path| format!("{prefix}{path}"))
        .take(30)
        .collect();
    let mut best: Vec<&String> = paths
        .iter()
        .filter(|path| PREFERRED.contains(&path.rsplit('.').next().unwrap_or_default()))
        .collect();
    best.sort_by_key(|path| {
        PREFERRED
            .iter()
            .position(|p| *p == path.rsplit('.').next().unwrap_or_default())
    });
    let suggested: Vec<&String> = if best.is_empty() {
        paths.iter().collect()
    } else {
        best
    };
    let suggested: Vec<&str> = suggested.iter().take(3).map(|path| path.as_str()).collect();
    writeln!(
        err,
        "[truncated {}: showing {} of {} items. Full JSON ({} KB): {saved}\n narrow with --fields, e.g. --fields {}\n available: {}\n --raw prints everything]",
        if at.is_empty() {
            "the list".to_owned()
        } else {
            at.join(".")
        },
        kept.len(),
        items.len(),
        text.len() / 1024,
        suggested.join(","),
        paths.join(","),
    )?;
    Ok(())
}

/// Cuts a `{"text": …}` payload to fit, from the back for logs (the tail is
/// the news) and from the front otherwise, preferring a line boundary.
fn guard_text(
    mut value: Value,
    text: &str,
    keep_tail: bool,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<()> {
    let full = value["text"].as_str().unwrap_or_default().to_owned();
    let saved = save_temp(&full, ".txt")?;
    let overhead =
        text.len() - Value::String(full.clone()).to_string().len() + r#","truncated":true"#.len();
    let mut budget = GUARD.saturating_sub(overhead);
    let cut = loop {
        let cut = cut_text(&full, budget, keep_tail);
        let size = Value::String(cut.to_owned()).to_string().len();
        if overhead + size <= GUARD || budget == 0 {
            break cut;
        }
        budget = budget.saturating_sub(overhead + size - GUARD + 16);
    };
    let shown = cut.len();
    value["text"] = Value::String(cut.to_owned());
    value["truncated"] = Value::Bool(true);
    writeln!(out, "{value}")?;
    writeln!(
        err,
        "[text is {} KB; showing the {} {} KB. Full text: {saved}. --raw prints everything]",
        full.len() / 1024,
        if keep_tail { "last" } else { "first" },
        shown / 1024
    )?;
    Ok(())
}

/// At most `budget` bytes of `text` from its head or tail, on a char boundary,
/// and on a line boundary when one is near.
fn cut_text(text: &str, budget: usize, tail: bool) -> &str {
    if text.len() <= budget {
        return text;
    }
    if tail {
        let mut start = text.len() - budget;
        while !text.is_char_boundary(start) {
            start += 1;
        }
        let cut = &text[start..];
        match cut.find('\n') {
            Some(newline) if newline < cut.len() / 2 => &cut[newline + 1..],
            _ => cut,
        }
    } else {
        let mut end = budget;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let cut = &text[..end];
        match cut.rfind('\n') {
            Some(newline) if newline > cut.len() / 2 => &cut[..=newline],
            _ => cut,
        }
    }
}

/// `--output FILE`: the payload goes to a 0600 file and stdout says where.
/// A lone string (or an object holding only one) is written as itself, so
/// `--fields value --output FILE` leaves exactly the secret in the file.
fn save(value: &Value, path: &Path, out: &mut dyn Write) -> Result<()> {
    let lone = match value {
        Value::String(text) => Some(text.as_str()),
        Value::Object(map) if map.len() == 1 => map.values().next().and_then(Value::as_str),
        _ => None,
    };
    let content = lone.map_or_else(|| value.to_string(), str::to_owned);
    write_private(path, content.as_bytes())?;
    writeln!(
        out,
        "{}",
        json!({"saved": path.display().to_string(), "bytes": content.len()})
    )?;
    Ok(())
}

pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options
        .open(path)
        .with_context(|| format!("cannot write {}", path.display()))?;
    // An existing file keeps its old mode through `open`; tighten it before
    // anything is written into it.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(value: Value, globals: &Globals, keep_tail: bool) -> (String, String) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        emit(value, globals, keep_tail, &mut out, &mut err, false).unwrap();
        (
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    fn fields(list: &str) -> Globals {
        Globals {
            fields: Some(list.to_owned()),
            ..Globals::default()
        }
    }

    fn saved_path(err: &str) -> String {
        let at = err.find("/").unwrap();
        err[at..]
            .split(|c: char| c.is_whitespace() || c == ']')
            .next()
            .unwrap()
            .trim_end_matches('.')
            .to_owned()
    }

    #[test]
    fn fields_project_through_lists_and_nested_objects() {
        let value = json!([
            {"id": 1, "title": "a", "author": {"name": "kim", "mail": "k@x"}, "tags": [{"n": "x", "c": 1}]},
            {"id": 2, "title": "b", "author": null}
        ]);
        let (out, err) = run(value, &fields("id,author.name,tags.n"), false);
        assert_eq!(
            out.trim(),
            r#"[{"id":1,"author":{"name":"kim"},"tags":[{"n":"x"}]},{"id":2}]"#
        );
        assert!(err.is_empty());
    }

    #[test]
    fn fields_that_match_nothing_list_what_is_there() {
        let (out, err) = run(
            json!([{"id": 1, "author": {"name": "kim"}}]),
            &fields("nope"),
            false,
        );
        assert_eq!(out.trim(), "[{}]");
        assert_eq!(
            err.trim(),
            "[--fields matched nothing. Available: id,author.name]"
        );
    }

    #[test]
    fn nulls_and_empties_are_dropped_unless_raw() {
        let value = json!({"a": null, "b": "", "c": [], "d": {}, "e": 0, "f": false, "g": [{"h": null}], "i": "x"});
        let (out, _) = run(value.clone(), &Globals::default(), false);
        assert_eq!(out.trim(), r#"{"e":0,"f":false,"g":[{}],"i":"x"}"#);
        let raw = Globals {
            raw: true,
            ..Globals::default()
        };
        assert_eq!(run(value.clone(), &raw, false).0.trim(), value.to_string());
    }

    #[test]
    fn a_big_list_is_cut_to_valid_json_and_saved_whole() {
        let items: Vec<Value> = (0..400)
            .map(|i| json!({"id": i, "title": format!("item number {i} with some words"), "state": "Active"}))
            .collect();
        let (out, err) = run(
            json!({"count": 400, "value": items}),
            &Globals::default(),
            false,
        );
        assert!(out.len() <= GUARD + 1, "{}", out.len());
        let shown: Value = serde_json::from_str(&out).unwrap();
        let kept = shown["value"].as_array().unwrap().len();
        assert!(kept > 50 && kept < 400, "{kept}");
        assert_eq!(shown["count"], 400);
        assert!(
            err.starts_with(&format!("[truncated value: showing {kept} of 400 items.")),
            "{err}"
        );
        assert!(
            err.contains("e.g. --fields value.id,value.title,value.state"),
            "{err}"
        );
        let saved = saved_path(&err);
        let full: Value = serde_json::from_str(&std::fs::read_to_string(&saved).unwrap()).unwrap();
        assert_eq!(full["value"].as_array().unwrap().len(), 400);
        std::fs::remove_file(saved).unwrap();

        let raw = Globals {
            raw: true,
            ..Globals::default()
        };
        let items: Vec<Value> = (0..3000).map(|i| json!({"id": i})).collect();
        assert!(
            run(Value::Array(items), &raw, false).0.len() > GUARD,
            "--raw bypasses the guard"
        );
    }

    #[test]
    fn long_text_keeps_the_head_or_for_logs_the_tail() {
        let text: String = (0..3000).map(|i| format!("line {i}\n")).collect();
        let value = json!({"text": text, "lines": 3000});
        let (head, err) = run(value.clone(), &Globals::default(), false);
        let head: Value = serde_json::from_str(&head).unwrap();
        let kept = head["text"].as_str().unwrap();
        assert!(kept.starts_with("line 0\n") && kept.ends_with('\n'));
        assert_eq!(
            (head["truncated"].clone(), head["lines"].clone()),
            (json!(true), json!(3000))
        );
        assert!(err.contains("showing the first"), "{err}");
        let saved = saved_path(&err);
        assert_eq!(std::fs::read_to_string(&saved).unwrap(), text);
        std::fs::remove_file(saved).unwrap();

        let (tail, err) = run(value, &Globals::default(), true);
        assert!(tail.len() <= GUARD + 1);
        let tail: Value = serde_json::from_str(&tail).unwrap();
        let kept = tail["text"].as_str().unwrap();
        assert!(
            kept.ends_with("line 2999\n") && kept.starts_with("line "),
            "{}",
            &kept[..20]
        );
        assert!(err.contains("showing the last"), "{err}");
        std::fs::remove_file(saved_path(&err)).unwrap();
    }

    #[test]
    fn output_writes_a_private_file_and_prints_only_where() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret.txt");
        std::fs::write(&path, "old").unwrap();
        let globals = Globals {
            output: Some(path.clone()),
            fields: Some("value".into()),
            ..Globals::default()
        };
        let (out, _) = run(json!({"name": "db", "value": "p@ss w0rd"}), &globals, false);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "p@ss w0rd");
        assert_eq!(
            out.trim(),
            format!(r#"{{"saved":"{}","bytes":9}}"#, path.display())
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let whole = Globals {
            output: Some(path.clone()),
            ..Globals::default()
        };
        run(json!({"a": 1, "b": "x"}), &whole, false);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            r#"{"a":1,"b":"x"}"#
        );
    }
}
