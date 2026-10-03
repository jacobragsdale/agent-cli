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
use crate::error::Failure;

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
        // An empty answer has nothing to match: it prints as itself, `[]`.
        let available = if nothing {
            leaf_paths(&value, "", 0)
        } else {
            Vec::new()
        };
        let asked: Vec<String> = paths.iter().map(|path| path.join(".")).collect();
        if !available.is_empty() && asked.iter().all(|path| available.contains(path)) {
            // Fields the answer has, all empty here: not a misspelling.
            writeln!(err, "[--fields {}: empty]", asked.join(","))?;
        } else if !available.is_empty() {
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
    // --reveal asked for the value on stdout; a spill file would leave it on disk.
    if globals.raw || globals.reveal || globals.fields.is_some() || tty || text.len() <= GUARD {
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

/// One step down a JSON value: a key, or an index into a list.
#[derive(Clone)]
enum Step {
    Key(String),
    Index(usize),
}

/// The largest list a few levels down, through objects and into a list's
/// items, so `results[0].rows` is found and not only `results`.
fn biggest_list<'a>(value: &'a Value, path: &[Step]) -> Option<(Vec<Step>, &'a Vec<Value>)> {
    let deeper = |step: Step, child: &'a Value| {
        let mut path = path.to_vec();
        path.push(step);
        biggest_list(child, &path)
    };
    let mut found: Vec<(Vec<Step>, &Vec<Value>)> = Vec::new();
    match value {
        Value::Array(items) => {
            // One item cannot be cut short; what it holds can.
            if items.len() != 1 {
                found.push((path.to_vec(), items));
            }
            // An envelope (results[0]) holds the payload; a long list is it.
            if path.len() < 4 && items.len() <= 10 {
                found.extend(
                    items
                        .iter()
                        .enumerate()
                        .filter_map(|(at, item)| deeper(Step::Index(at), item)),
                );
            }
        }
        Value::Object(map) if path.len() < 4 => {
            found.extend(
                map.iter()
                    .filter_map(|(key, child)| deeper(Step::Key(key.clone()), child)),
            );
        }
        _ => {}
    }
    found
        .into_iter()
        .max_by_key(|(_, items)| serde_json::to_string(items).map_or(0, |text| text.len()))
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
    // The answer is worth more than the copy: print it cut either way.
    let saved = save_temp(text, ".json").unwrap_or_else(|error| format!("not saved ({error:#})"));
    let found = biggest_list(value, &[]).filter(|(_, items)| !items.is_empty());
    let Some((at, items)) = found else {
        let mut shown = value.clone();
        let cut = cut_strings(&mut shown);
        if !cut.is_empty() && shown.to_string().len() <= GUARD {
            writeln!(out, "{shown}")?;
            writeln!(
                err,
                "[cut short: {}. Full JSON ({} KB): {saved}\n --raw prints everything]",
                cut.join(", "),
                text.len() / 1024
            )?;
            return Ok(());
        }
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
    let mut size = text.len() - serde_json::to_string(items).map_or(0, |list| list.len());
    let mut kept = Vec::new();
    for item in items {
        size += item.to_string().len() + 1;
        if size > GUARD && !kept.is_empty() {
            break;
        }
        kept.push(item.clone());
    }
    let mut shown = value.clone();
    let mut node = &mut shown;
    for step in &at {
        node = match step {
            Step::Key(key) => &mut node[key.as_str()],
            Step::Index(index) => &mut node[*index],
        };
    }
    *node = Value::Array(kept.clone());
    // A long text beside the list (a description) can hold the answer over.
    let cut = cut_strings(&mut shown);
    writeln!(out, "{shown}")?;
    // A --fields path maps through lists, so only the keys name it.
    let keys: Vec<&str> = at
        .iter()
        .filter_map(|step| match step {
            Step::Key(key) => Some(key.as_str()),
            Step::Index(_) => None,
        })
        .collect();
    let prefix = if keys.is_empty() {
        String::new()
    } else {
        format!("{}.", keys.join("."))
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
    // Rows that are plain lists (SQL's) have no field paths to narrow by.
    let narrow = if paths.is_empty() {
        String::new()
    } else {
        format!(
            "\n narrow with --fields, e.g. --fields {}\n available: {}",
            suggested.join(","),
            paths.join(",")
        )
    };
    let also = if cut.is_empty() {
        String::new()
    } else {
        format!(", and cut short: {}", cut.join(", "))
    };
    writeln!(
        err,
        "[truncated {}: showing {} of {} items{also}. Full JSON ({} KB): {saved}{narrow}\n --raw prints everything]",
        if keys.is_empty() {
            "the list".to_owned()
        } else {
            keys.join(".")
        },
        kept.len(),
        items.len(),
        text.len() / 1024,
    )?;
    Ok(())
}

/// Cuts the longest strings in `value`, each to what still fits and an
/// ellipsis, until it prints within [`GUARD`]; the dotted names of those cut.
fn cut_strings(value: &mut Value) -> Vec<String> {
    fn longest(value: &Value, at: &mut Vec<Step>, best: &mut Option<(Vec<Step>, usize)>) {
        match value {
            Value::String(text) if best.as_ref().is_none_or(|(_, len)| text.len() > *len) => {
                *best = Some((at.clone(), text.len()));
            }
            Value::Object(map) => {
                for (key, child) in map {
                    at.push(Step::Key(key.clone()));
                    longest(child, at, best);
                    at.pop();
                }
            }
            Value::Array(items) => {
                for (index, child) in items.iter().enumerate() {
                    at.push(Step::Index(index));
                    longest(child, at, best);
                    at.pop();
                }
            }
            _ => {}
        }
    }
    let mut cut = Vec::new();
    loop {
        let size = value.to_string().len();
        let mut best = None;
        longest(value, &mut Vec::new(), &mut best);
        // A short string is not where the bytes are.
        let Some((at, len)) = best.filter(|(_, len)| size > GUARD && *len > 200) else {
            return cut;
        };
        let mut node = &mut *value;
        for step in &at {
            node = match step {
                Step::Key(key) => &mut node[key.as_str()],
                Step::Index(index) => &mut node[*index],
            };
        }
        let Value::String(text) = node else {
            return cut;
        };
        let mut end = len.saturating_sub(size - GUARD + 64).max(200).min(len - 1);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        // The ellipsis is three bytes: a cut that saves none ends the cutting.
        if end + 3 >= len {
            return cut;
        }
        text.truncate(end);
        text.push('…');
        let name: Vec<&str> = (at.iter())
            .filter_map(|step| match step {
                Step::Key(key) => Some(key.as_str()),
                Step::Index(_) => None,
            })
            .collect();
        let name = name.join(".");
        if !cut.contains(&name) {
            cut.push(name);
        }
    }
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
    let saved = save_temp(&full, ".txt").unwrap_or_else(|error| format!("not saved ({error:#})"));
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
/// `--output` must name a file in a directory that exists, checked before
/// the command runs: a change whose answer cannot be saved is still made,
/// and its answer (the new id) would be lost.
pub(crate) fn check_output(path: &Path) -> Result<(), Failure> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let problem = if path.is_dir() {
        "is a directory"
    } else if !parent.is_dir() {
        "is in a directory that does not exist"
    } else {
        return Ok(());
    };
    Err(
        Failure::usage(format!("--output {} {problem}", path.display()))
            .hint("give --output a file path in a directory that exists"),
    )
}

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
    // A file is replaced whole, so an interrupted write leaves the old one;
    // a device, a pipe or a link is written in place. ponytail: a kill
    // during the write leaves its .tmp file beside the target.
    let in_place = std::fs::symlink_metadata(path).is_ok_and(|meta| !meta.is_file());
    if !in_place {
        let dir = path.parent().filter(|dir| !dir.as_os_str().is_empty());
        let mut file = tempfile::NamedTempFile::new_in(dir.unwrap_or(Path::new(".")))
            .with_context(|| format!("cannot write {}", path.display()))?;
        file.write_all(bytes)?;
        file.persist(path)
            .with_context(|| format!("cannot write {}", path.display()))?;
        return Ok(());
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options
        .open(path)
        .with_context(|| format!("cannot write {}", path.display()))?;
    // An existing file keeps its old mode through `open`; tighten it before
    // anything is written into it. Only a file: /dev/null is everyone's.
    #[cfg(unix)]
    if file.metadata().is_ok_and(|meta| meta.is_file()) {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("cannot make {} private", path.display()))?;
    }
    file.write_all(bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_must_be_a_file_in_a_directory_that_exists() {
        let dir = tempfile::tempdir().unwrap();
        assert!(check_output(&dir.path().join("row.json")).is_ok());
        assert!(
            check_output(Path::new("row.json")).is_ok(),
            "the working directory"
        );
        for (path, why) in [
            (dir.path().to_path_buf(), "is a directory"),
            (dir.path().join("nodir/row.json"), "does not exist"),
        ] {
            let failure = check_output(&path).unwrap_err();
            assert_eq!(failure.exit, crate::error::Exit::Usage);
            assert!(failure.message.contains(why), "{}", failure.message);
        }
    }

    #[test]
    fn a_list_nested_in_an_envelope_is_the_one_cut() {
        let rows: Vec<Value> = (0..2000).map(|n| json!([n, "x".repeat(20)])).collect();
        let value =
            json!({"results": [{"columns": ["id", "name"], "rows": rows}], "elapsed_ms": 7});
        let (out, err) = run(value, &Globals::default(), false);
        assert!(out.len() <= GUARD + 1, "{}", out.len());
        let shown: Value = serde_json::from_str(&out).unwrap();
        let kept = shown["results"][0]["rows"].as_array().unwrap().len();
        assert!(kept > 100 && kept < 2000, "{kept}");
        assert!(err.contains("[truncated results.rows: showing"), "{err}");
    }

    #[test]
    fn a_revealed_value_prints_whole_and_never_lands_in_a_spill_file() {
        let value = json!({"secret": "big", "key": "v", "value": "x".repeat(20_000)});
        let globals = Globals {
            reveal: true,
            ..Globals::default()
        };
        let (out, err) = run(value, &globals, false);
        assert!(out.len() > 20_000, "{}", out.len());
        assert!(err.is_empty(), "no spill note: {err}");
    }

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
    fn fields_the_answer_has_but_empty_say_so() {
        let (out, err) = run(
            json!({"name": "web", "default_branch": null, "branches": []}),
            &fields("default_branch,branches"),
            false,
        );
        assert_eq!(out.trim(), "{}");
        assert_eq!(err.trim(), "[--fields default_branch,branches: empty]");
    }

    #[test]
    fn an_empty_answer_with_fields_prints_empty_without_a_note() {
        let (out, err) = run(json!([]), &fields("id"), false);
        assert_eq!(out.trim(), "[]");
        assert!(err.is_empty(), "{err}");
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

    #[test]
    fn a_long_text_beside_the_list_is_cut_short_too() {
        let value = json!({"id": 903, "title": "big", "description": "word ".repeat(6000),
            "comments": [{"id": 1, "text": "a"}, {"id": 2, "text": "b"}]});
        let (out, err) = run(value, &Globals::default(), false);
        assert!(out.len() <= GUARD + 1, "{} bytes", out.len());
        let shown: Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(
            (shown["id"].clone(), shown["title"].clone()),
            (json!(903), json!("big"))
        );
        assert!(shown["description"].as_str().unwrap().ends_with('…'));
        assert!(err.contains("cut short: description"), "{err}");

        let (out, err) = run(
            json!({"id": 1, "description": "é".repeat(20000)}),
            &Globals::default(),
            false,
        );
        assert!(out.len() <= GUARD + 1, "{} bytes", out.len());
        assert!(err.starts_with("[cut short: description."), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn output_to_a_device_writes_without_touching_its_mode() {
        use std::os::unix::fs::PermissionsExt;
        let null = Path::new("/dev/null");
        let mode = std::fs::metadata(null).unwrap().permissions().mode();
        write_private(null, b"discarded").unwrap();
        assert_eq!(std::fs::metadata(null).unwrap().permissions().mode(), mode);
    }

    #[cfg(unix)]
    #[test]
    fn an_existing_output_file_is_replaced_whole_and_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.bin");
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_private(&path, b"new bytes").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new bytes");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            1,
            "no temp file left"
        );
    }
}
