//! `[core] include`: a team's shared file under a person's own.
//!
//! A team hands everyone the same `team.toml` (its connections, its
//! `read_only` prod entries, its `*_kv` logins) and each person keeps a short
//! `config.toml` that includes it and overrides what differs. Merging is by
//! key, and by `name` for lists of entries, so a person's
//! `[[sql.connection]] name = "prod" read_only = false` changes one key of
//! the team's entry rather than replacing the list.

use std::path::{Path, PathBuf};

use toml::{Table, Value};

/// One file `[core] include` names, and whether it was there.
#[derive(Clone, Debug)]
pub(crate) struct Included {
    pub(crate) path: PathBuf,
    pub(crate) found: bool,
}

/// Why the layered config could not be read: the message, and the next step
/// when it is not the generic "fix the file".
#[derive(Clone, Debug)]
pub(crate) struct Problem {
    pub(crate) message: String,
    pub(crate) hint: Option<String>,
}

impl Problem {
    pub(crate) fn plain(message: String) -> Self {
        Self {
            message,
            hint: None,
        }
    }
}

/// `table` (the file at `path`, as `text`) over the files its `[core]
/// include` names, in order. Every included file is reported, found or not;
/// the first one missing, unparsable or including another is the problem.
pub(crate) fn layered(
    path: &Path,
    text: &str,
    table: Table,
) -> (Result<Table, Problem>, Vec<Included>) {
    let names = match includes(&table) {
        Ok(names) => names,
        Err(why) => {
            return (
                Err(Problem::plain(format!("{} {why}", path.display()))),
                Vec::new(),
            );
        }
    };
    if names.is_empty() {
        return (Ok(table), Vec::new());
    }
    let mut included = Vec::new();
    let mut problem = None;
    let mut merged = Table::new();
    for name in &names {
        let file = resolve(path, name);
        let read = std::fs::read_to_string(&file);
        included.push(Included {
            path: file.clone(),
            found: read.is_ok(),
        });
        if problem.is_some() {
            continue;
        }
        let layer = match read {
            Ok(text) => text.parse::<Table>().map_err(|error| {
                Problem::plain(format!("cannot parse {}: {error}", file.display()))
            }),
            Err(error) => {
                let line = line_of(text, name);
                let what = if error.kind() == std::io::ErrorKind::NotFound {
                    "does not exist".to_owned()
                } else {
                    format!("cannot be read ({error})")
                };
                Err(Problem {
                    message: format!(
                        "{} {what}; {} line {line} includes it ([core] include)",
                        file.display(),
                        path.display()
                    ),
                    hint: Some(format!(
                        "rerun install.sh, which puts the team's file there, or take it out of [core] include in {}",
                        path.display()
                    )),
                })
            }
        };
        match layer {
            Ok(layer) if includes(&layer).is_ok_and(|names| names.is_empty()) => {
                merge(&mut merged, layer);
            }
            Ok(_) => {
                problem = Some(Problem::plain(format!(
                    "{} has a [core] include of its own; only {} may include",
                    file.display(),
                    path.display()
                )));
            }
            Err(why) => problem = Some(why),
        }
    }
    match problem {
        Some(problem) => (Err(problem), included),
        None => {
            merge(&mut merged, table);
            (Ok(merged), included)
        }
    }
}

/// `[core] include` as a list: a string is one file.
fn includes(table: &Table) -> Result<Vec<String>, String> {
    let Some(core) = table.get("core") else {
        return Ok(Vec::new());
    };
    let wrong = || "[core] include is not a path or a list of paths".to_owned();
    match core.get("include") {
        None => Ok(Vec::new()),
        Some(Value::String(one)) => Ok(vec![one.clone()]),
        Some(Value::Array(many)) => many
            .iter()
            .map(|item| item.as_str().map(str::to_owned).ok_or_else(wrong))
            .collect(),
        Some(_) => Err(wrong()),
    }
}

/// `~/` is the home directory; a relative path is beside the including file.
fn resolve(including: &Path, name: &str) -> PathBuf {
    if let Some(rest) = name.strip_prefix("~/")
        && let Some(home) = std::env::home_dir()
    {
        return home.join(rest);
    }
    let path = Path::new(name);
    if path.is_absolute() {
        return path.to_owned();
    }
    including.parent().unwrap_or(Path::new("")).join(path)
}

/// The 1-based line of `text` that names `name`, else the `include` line.
fn line_of(text: &str, name: &str) -> usize {
    let quoted = [format!("\"{name}\""), format!("'{name}'")];
    let at = text
        .lines()
        .position(|line| quoted.iter().any(|quoted| line.contains(quoted.as_str())))
        .or_else(|| {
            text.lines()
                .position(|line| line.trim_start().starts_with("include"))
        });
    at.map_or(1, |at| at + 1)
}

/// `over` onto `base`: scalars and other lists replace, tables merge key by
/// key, and lists of entries that all have a `name` merge entry by entry.
pub(crate) fn merge(base: &mut Table, over: Table) {
    for (key, value) in over {
        match (base.get_mut(&key), value) {
            (Some(Value::Table(old)), Value::Table(new)) => merge(old, new),
            (Some(Value::Array(old)), Value::Array(new)) if named(old) && named(&new) => {
                for entry in new {
                    let name = entry.get("name").cloned();
                    let same = old
                        .iter_mut()
                        .find(|item| item.get("name") == name.as_ref());
                    match (same, entry) {
                        (Some(Value::Table(old)), Value::Table(new)) => merge(old, new),
                        (_, entry) => old.push(entry),
                    }
                }
            }
            (_, value) => {
                base.insert(key, value);
            }
        }
    }
}

fn named(items: &[Value]) -> bool {
    !items.is_empty()
        && items
            .iter()
            .all(|item| item.get("name").is_some_and(Value::is_str))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(text: &str) -> Table {
        text.parse().unwrap()
    }

    #[test]
    fn a_later_file_wins_key_by_key_and_entries_merge_by_name() {
        let mut base = table(
            "[sql]\ntimeout = 5\nlist = [1, 2]\n\n[[sql.connection]]\nname = \"prod\"\nhost = \"db\"\nread_only = true\n\n[[sql.connection]]\nname = \"qa\"\nhost = \"qa-db\"\n",
        );
        merge(
            &mut base,
            table(
                "[sql]\nlist = [3]\n\n[[sql.connection]]\nname = \"prod\"\nread_only = false\n\n[[sql.connection]]\nname = \"mine\"\nhost = \"local\"\n",
            ),
        );
        let sql = &base["sql"];
        assert_eq!(sql["timeout"].as_integer(), Some(5));
        assert_eq!(
            sql["list"].as_array().unwrap().len(),
            1,
            "a plain list is replaced"
        );
        let entries = sql["connection"].as_array().unwrap();
        let names: Vec<&str> = entries
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["prod", "qa", "mine"]);
        assert_eq!(entries[0]["host"].as_str(), Some("db"));
        assert_eq!(entries[0]["read_only"].as_bool(), Some(false));
    }

    #[test]
    fn includes_merge_in_order_under_the_including_file_and_report_each() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("team.toml"),
            "[ado]\norg = \"contoso\"\nproject = \"web\"\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("more.toml"), "[ado]\nproject = \"data\"\n").unwrap();
        let path = dir.path().join("config.toml");
        let text = "[core]\ninclude = [\"team.toml\", \"more.toml\"]\n\n[ado]\nteam = \"mine\"\n";
        let (merged, included) = layered(&path, text, table(text));
        let merged = merged.unwrap();
        assert_eq!(merged["ado"]["org"].as_str(), Some("contoso"));
        assert_eq!(merged["ado"]["project"].as_str(), Some("data"));
        assert_eq!(merged["ado"]["team"].as_str(), Some("mine"));
        assert!(included.iter().all(|file| file.found) && included.len() == 2);
    }

    #[test]
    fn a_missing_include_names_the_file_and_the_line_and_an_include_that_includes_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let text = "# mine\n[core]\ninclude = \"gone.toml\"\n";
        let (merged, included) = layered(&path, text, table(text));
        let problem = merged.unwrap_err();
        assert!(
            problem.message.contains("gone.toml does not exist"),
            "{problem:?}"
        );
        assert!(
            problem.message.contains("config.toml line 3 includes it"),
            "{problem:?}"
        );
        assert!(problem.hint.unwrap().starts_with("rerun install.sh"));
        assert!(!included[0].found);

        std::fs::write(
            dir.path().join("team.toml"),
            "[core]\ninclude = \"other.toml\"\n",
        )
        .unwrap();
        let text = "[core]\ninclude = \"team.toml\"\n";
        let (merged, _) = layered(&path, text, table(text));
        assert!(merged.unwrap_err().message.contains("only"));
    }
}
