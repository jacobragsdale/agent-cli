//! The usage log: one JSON line per run in `usage.jsonl` under the state
//! directory, and `agent-cli usage`, its summary.
//!
//! A team needs to see which commands its agents lean on, which calls they
//! get wrong (exit 2, by flag names) and which cost the most requests,
//! without anyone sending anything anywhere. So the log stays on the
//! machine, holds no value and no positional, only names and counts, and a
//! person pastes the summary where the team tracks it.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::Result;
use serde_json::{Value, json};

use crate::ctx::{Ctx, Meter};
use crate::error::{Exit, Failure};
use crate::output;
use crate::when::{When, now, utc_time};

/// Past this the log keeps its newest half.
const CUT_AT: u64 = 5 * 1024 * 1024;
/// How many entries each "most" list shows.
const TOP: usize = 10;

/// Where this run's line goes, or `None` when the log is off: no state
/// directory (tests, fixtures) or `AGENT_CLI_USAGE=off`.
pub(crate) fn log_path(state_dir: Option<&Path>, off: bool) -> Option<PathBuf> {
    (!off).then(|| state_dir.map(|dir| dir.join("usage.jsonl")))?
}

/// The flag names on the line, before any `--`: never their values, never
/// a positional. `-5` is a value, not a flag.
pub(crate) fn flag_names(argv: &[String]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for arg in argv.iter().take_while(|arg| *arg != "--") {
        let name = arg.split('=').next().unwrap_or_default();
        let bare = name.trim_start_matches('-');
        if name.starts_with('-') && bare.starts_with(|c: char| c.is_ascii_alphabetic()) {
            let name = name.to_owned();
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names
}

/// Appends one run's line; a log that cannot be written is never worth
/// failing the command over.
pub(crate) fn append(
    path: &Path,
    command: &str,
    flags: &[String],
    exit: u8,
    took: Duration,
    meter: &Meter,
) {
    let line = json!({
        "at": utc_time(now()),
        "version": env!("CARGO_PKG_VERSION"),
        "command": command,
        "flags": flags,
        "exit": exit,
        "ms": u64::try_from(took.as_millis()).unwrap_or(u64::MAX),
        "requests": meter.requests.load(Ordering::Relaxed),
        "bytes": meter.bytes.load(Ordering::Relaxed),
        "processes": meter.processes.load(Ordering::Relaxed),
    });
    let _ = path.parent().map(std::fs::create_dir_all);
    let appended = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| writeln!(file, "{line}"));
    if appended.is_ok() && std::fs::metadata(path).is_ok_and(|meta| meta.len() > CUT_AT) {
        cut(path);
    }
}

/// Keeps the newest half of the log, whole lines only.
// ponytail: two runs cutting at once can lose the lines appended between
// the read and the rename; a lock file if a team's numbers ever need them.
fn cut(path: &Path) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let half = text.len() / 2;
    let start = text[..half].rfind('\n').map_or(0, |at| at + 1);
    let Some(dir) = path.parent() else {
        return;
    };
    let _ = tempfile::NamedTempFile::new_in(dir).and_then(|mut file| {
        file.write_all(&text.as_bytes()[start..])?;
        file.persist(path).map(drop).map_err(|error| error.error)
    });
}

/// `agent-cli usage [--since 7d]`: the summary as JSON.
pub(crate) fn builtin(
    args: &[String],
    ctx: &Ctx,
    out: &mut dyn Write,
    err: &mut dyn Write,
    tty: bool,
) -> Result<Exit> {
    let usage = "usage: agent-cli usage [--since 7d]   what this machine's agent-cli runs did since then (default 7d), as JSON: calls, per command (count, failures by exit code, median and slowest ms, requests, bytes), the most frequent exit 2s with their flag names, the slowest, the most requests. Nothing is sent anywhere; AGENT_CLI_USAGE=off stops the log";
    if ctx.globals().help {
        writeln!(out, "{usage}")?;
        return Ok(Exit::Ok);
    }
    let since = match args {
        [] => "7d",
        [flag, value] if flag == "--since" => value.as_str(),
        [flag] if flag.starts_with("--since=") => &flag["--since=".len()..],
        _ => {
            return Err(Failure::usage("unknown arguments to agent-cli usage")
                .hint(usage)
                .into());
        }
    };
    let since: When = since
        .parse()
        .map_err(|why: String| Failure::usage(format!("--since: {why}")))?;
    let path = log_path(ctx.state_dir.as_deref(), false);
    let text = path
        .as_deref()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .unwrap_or_default();
    let entries: Vec<Value> = text
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|entry| {
            entry["at"]
                .as_str()
                .and_then(|at| at.parse::<When>().ok())
                .is_some_and(|at| at >= since)
        })
        .collect();
    output::emit(
        summary(since, &entries),
        ctx.globals(),
        false,
        out,
        err,
        tty,
    )?;
    if entries.is_empty() {
        let at = path.map_or("no state directory".to_owned(), |path| {
            path.display().to_string()
        });
        writeln!(err, "[no runs logged since {} ({at})]", since.utc())?;
    }
    Ok(Exit::Ok)
}

fn summary(since: When, entries: &[Value]) -> Value {
    let number = |entry: &Value, key: &str| entry[key].as_u64().unwrap_or_default();
    let name = |entry: &Value| match entry["command"].as_str() {
        Some("") | None => "agent-cli".to_owned(),
        Some(command) => command.to_owned(),
    };
    let mut by: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    for entry in entries {
        by.entry(name(entry)).or_default().push(entry);
    }
    let mut commands: Vec<Value> = by
        .iter()
        .map(|(command, runs)| {
            let mut ms: Vec<u64> = runs.iter().map(|run| number(run, "ms")).collect();
            ms.sort_unstable();
            let mut failures: BTreeMap<String, u64> = BTreeMap::new();
            for run in runs.iter().filter(|run| number(run, "exit") != 0) {
                *failures.entry(number(run, "exit").to_string()).or_default() += 1;
            }
            json!({
                "command": command,
                "count": runs.len(),
                "failures": failures,
                "median_ms": ms[ms.len() / 2],
                "slowest_ms": ms.last().copied().unwrap_or_default(),
                "requests": runs.iter().map(|run| number(run, "requests")).sum::<u64>(),
                "bytes": runs.iter().map(|run| number(run, "bytes")).sum::<u64>(),
            })
        })
        .collect();
    commands.sort_by_key(|row| std::cmp::Reverse(row["count"].as_u64()));
    let mut usage_errors: BTreeMap<(String, String), u64> = BTreeMap::new();
    for entry in entries.iter().filter(|entry| number(entry, "exit") == 2) {
        let flags: Vec<&str> = entry["flags"]
            .as_array()
            .map(|flags| flags.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        *usage_errors
            .entry((name(entry), flags.join(" ")))
            .or_default() += 1;
    }
    let mut exit_2: Vec<Value> = usage_errors
        .into_iter()
        .map(
            |((command, flags), count)| json!({"command": command, "flags": flags, "count": count}),
        )
        .collect();
    exit_2.sort_by_key(|row| std::cmp::Reverse(row["count"].as_u64()));
    exit_2.truncate(TOP);
    let most = |key: &str| {
        let mut runs: Vec<&Value> = entries.iter().collect();
        runs.sort_by_key(|run| std::cmp::Reverse(number(run, key)));
        runs.iter()
            .take(TOP)
            .filter(|run| number(run, key) > 0)
            .map(|run| json!({"command": name(run), key: number(run, key), "at": run["at"], "exit": run["exit"]}))
            .collect::<Vec<_>>()
    };
    json!({
        "since": since.utc(),
        "calls": entries.len(),
        "commands": commands,
        "exit_2": exit_2,
        "slowest": most("ms"),
        "most_requests": most("requests"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_owned()).collect()
    }

    #[test]
    fn only_flag_names_are_kept_never_values_or_positionals() {
        let argv = strings(&[
            "sql",
            "query",
            "run",
            "select 1",
            "--conn=prod",
            "--limit",
            "5",
            "-n",
            "-5",
            "--",
            "--raw",
        ]);
        assert_eq!(flag_names(&argv), ["--conn", "--limit", "-n"]);
    }

    #[test]
    fn the_log_is_off_without_a_state_dir_or_when_asked() {
        assert!(log_path(None, false).is_none());
        assert!(log_path(Some(Path::new("/s")), true).is_none());
        assert_eq!(
            log_path(Some(Path::new("/s")), false).unwrap(),
            Path::new("/s/usage.jsonl")
        );
    }

    #[test]
    fn a_log_past_its_cap_keeps_its_newest_half_in_whole_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.jsonl");
        let lines: String = (0..10).map(|n| format!("{{\"n\":{n}}}\n")).collect();
        std::fs::write(&path, &lines).unwrap();
        cut(&path);
        let kept = std::fs::read_to_string(&path).unwrap();
        assert!(
            kept.starts_with("{\"n\":5}") && kept.ends_with("{\"n\":9}\n"),
            "{kept}"
        );
    }

    #[test]
    fn the_summary_counts_by_command_with_failures_exit_2s_and_the_costliest() {
        let entry = |command: &str, exit: u64, ms: u64, requests: u64, flags: &[&str]| {
            json!({"at": "2026-10-20T10:00:00Z", "command": command, "exit": exit, "ms": ms,
                   "requests": requests, "bytes": 100, "flags": flags})
        };
        let entries = [
            entry("ado workitem list", 0, 300, 2, &["--limit"]),
            entry("ado workitem list", 2, 10, 0, &["--count"]),
            entry("ado workitem list", 2, 12, 0, &["--count"]),
            entry("sql query run", 0, 900, 0, &["--conn"]),
        ];
        let since = When(time::macros::datetime!(2026-10-01 00:00 UTC));
        let summary = summary(since, &entries);
        assert_eq!(summary["calls"], 4);
        let list = &summary["commands"][0];
        assert_eq!(list["command"], "ado workitem list");
        assert_eq!(
            (list["count"].as_u64(), list["failures"]["2"].as_u64()),
            (Some(3), Some(2))
        );
        assert_eq!(
            (list["median_ms"].as_u64(), list["slowest_ms"].as_u64()),
            (Some(12), Some(300))
        );
        assert_eq!(
            summary["exit_2"][0],
            json!({"command": "ado workitem list", "flags": "--count", "count": 2})
        );
        assert_eq!(summary["slowest"][0]["command"], "sql query run");
        assert_eq!(summary["most_requests"].as_array().unwrap().len(), 1);
    }
}
