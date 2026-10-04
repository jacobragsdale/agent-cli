//! A command's own args: parsed by clap, and refused with a message that
//! names the flags the command has when the call is wrong.

use clap::ArgMatches;

use crate::discover::did_you_mean;
use crate::error::Failure;
use crate::registry::Command;

/// The invoked command's own args, parsed by clap with its help off. A clap
/// error becomes a usage error that shows the example.
pub(crate) fn parse_leaf(command: &Command, args: &[String]) -> Result<ArgMatches, Failure> {
    let usage = |message: String| {
        Failure::usage(message).hint(format!(
            "e.g. agent-cli {}\nall args: agent-cli {} --help",
            command.example,
            command.path.join(" ")
        ))
    };
    let matches = (command.args)()
        .no_binary_name(true)
        .disable_help_flag(true)
        .disable_version_flag(true)
        .color(clap::ColorChoice::Never)
        .try_get_matches_from(args)
        .map_err(|error| {
            usage(unknown_flag(command, &error, args).unwrap_or_else(|| clap_message(&error)))
        })?;
    match flag_taken_as_text(command, &matches) {
        Some(message) => Err(usage(message)),
        None => Ok(matches),
    }
}

/// An arg that takes leading hyphens (Markdown with `- ` bullets, SQL with
/// `--` comments) also takes a flag typed in the wrong place:
/// `comment 42 --text=hi` would post "--text=hi", and `--comment --if-rev=7`
/// would drop the rev test. A whole value that is one `--name` or
/// `--name=value` is never meant as text, so it is refused.
fn flag_taken_as_text(command: &Command, matches: &ArgMatches) -> Option<String> {
    let args = (command.args)();
    for arg in args
        .get_arguments()
        .filter(|arg| arg.is_allow_hyphen_values_set())
    {
        let values = matches.get_raw(arg.get_id().as_str()).into_iter().flatten();
        for value in values {
            let value = value.to_string_lossy();
            let Some(name) = flag_shaped(&value) else {
                continue;
            };
            let own = args
                .get_arguments()
                .any(|other| other.get_long() == Some(name));
            if !own {
                return Some(unknown_flag_named(command, &format!("--{name}")));
            }
            let what = arg
                .get_long()
                .map_or_else(|| format!("<{}>", arg.get_id()), |long| format!("--{long}"));
            return Some(format!(
                "{what} has no value of its own: {value} came next, and would be sent as its \
                 text; give {what} a value, or leave it out"
            ));
        }
    }
    None
}

/// The name in `--name` or `--name=value`, when that is the whole value.
fn flag_shaped(value: &str) -> Option<&str> {
    let rest = value.strip_prefix("--")?;
    let name = rest.split_once('=').map_or(rest, |(name, _)| name);
    let mut chars = name.chars();
    (chars.next()?.is_ascii_alphabetic()
        && chars.all(|char| char.is_ascii_alphanumeric() || char == '-'))
    .then_some(name)
}

/// A flag the command does not have: the close ones, and every one it has.
/// clap's own tip ("to pass '--log-id' as a value, use '-- --log-id'")
/// sends an agent the wrong way.
fn unknown_flag(command: &Command, error: &clap::Error, args: &[String]) -> Option<String> {
    use clap::error::{ContextKind, ContextValue, ErrorKind};
    if error.kind() != ErrorKind::UnknownArgument {
        return None;
    }
    let Some(ContextValue::String(flag)) = error.get(ContextKind::InvalidArg) else {
        return None;
    };
    if flag.starts_with('-') {
        return Some(unknown_flag_named(command, flag));
    }
    // `--sqlfile q.sql`: a positional that takes hyphens took the unknown
    // flag as its value, so clap names `q.sql`. The flag is the news.
    let own = (command.args)();
    let unknown = args
        .iter()
        .take_while(|arg| *arg != "--")
        .filter_map(|arg| flag_shaped(arg))
        .find(|name| !own.get_arguments().any(|arg| arg.get_long() == Some(*name)))?;
    Some(unknown_flag_named(command, &format!("--{unknown}")))
}

fn unknown_flag_named(command: &Command, flag: &str) -> String {
    let args = (command.args)();
    let flags: Vec<&str> = args
        .get_arguments()
        .filter(|arg| !arg.is_positional() && !arg.is_hide_set())
        .filter_map(clap::Arg::get_long)
        .collect();
    let wanted = flag.trim_start_matches('-');
    let synonym = crate::registry::SYNONYM_FLAGS
        .iter()
        .find(|(name, canonical)| *name == wanted && flags.contains(canonical))
        .map(|(_, canonical)| format!("--{canonical}"));
    let close: Vec<String> = match synonym {
        Some(canonical) => vec![canonical],
        None => did_you_mean(wanted, flags.iter().copied())
            .into_iter()
            .map(|name| format!("--{name}"))
            .collect(),
    };
    let mut message = format!("unknown flag {flag}");
    if !close.is_empty() {
        message.push_str(&format!(" \u{2014} did you mean {}?", close.join(", ")));
    }
    message.push_str(if close.is_empty() { "; " } else { " " });
    let path = command.path.join(" ");
    if flags.is_empty() {
        message.push_str(&format!("{path} takes no flags"));
    } else {
        let all: Vec<String> = flags.iter().map(|name| format!("--{name}")).collect();
        message.push_str(&format!("{path} takes {}", all.join(" ")));
    }
    message
}

/// clap's message without its usage block and tips, on one line.
fn clap_message(error: &clap::Error) -> String {
    let text = error.to_string();
    let text = text.split("\nUsage:").next().unwrap_or_default();
    let text = text
        .split("\nFor more information")
        .next()
        .unwrap_or_default();
    let mut message = String::new();
    for line in text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("tip:"))
    {
        if !message.is_empty() {
            message.push_str(if message.ends_with(':') { " " } else { "; " });
        }
        message.push_str(line.strip_prefix("error: ").unwrap_or(line));
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_whole_flag_shaped_value_reads_as_a_flag() {
        for (value, name) in [
            ("--text=hello", Some("text")),
            ("--json", Some("json")),
            ("--if-rev=7", Some("if-rev")),
            ("--message=LGTM, ship it", Some("message")),
            ("- a bullet", None),
            ("-- a SQL comment\nselect 1", None),
            ("--", None),
            ("---", None),
            ("--1", None),
            ("--verbose output is broken", None),
        ] {
            assert_eq!(flag_shaped(value), name, "{value:?}");
        }
    }
}
