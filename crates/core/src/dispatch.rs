//! argv to output: built-ins, the three-word path, the leaf parse, the effect
//! checks, the handler, and the exit code.
//!
//! clap parses only the one command that was invoked, with its help turned
//! off: core renders all help itself. Globals are stripped from anywhere on
//! the line first, so a command never sees them and none can shadow them.

use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use clap::ArgMatches;
use serde_json::json;

use crate::config;
use crate::ctx::{Ctx, Globals, Meter, Setup};
use crate::discover::{self, did_you_mean};
use crate::error::{Exit, Failure, data_of, describe};
pub(crate) use crate::leaf::parse_leaf;
use crate::output::{self, dumps};
use crate::registry::{Command, Domain, Effect, GLOBAL_FLAGS};
use crate::search;
use crate::{doctor, hidden, usage};

/// The whole program: `fn main() -> ExitCode { agent_cli_core::run(DOMAINS) }`.
#[must_use]
pub fn run(domains: &[Domain]) -> ExitCode {
    let argv: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    crate::stop::handle_signals();
    let stdout = std::io::stdout();
    let tty = stdout.is_terminal();
    let code = run_with(
        domains,
        &argv,
        Setup::from_env(),
        &mut stdout.lock(),
        &mut crate::stop::Stderr,
        tty,
    );
    crate::stop::wait_if_stopping();
    ExitCode::from(code)
}

/// [`run`] with its surroundings passed in, so tests run the real thing in
/// process. Returns the exit code, and appends the run to the usage log
/// when the setup keeps one.
pub fn run_with(
    domains: &[Domain],
    argv: &[String],
    setup: Setup,
    out: &mut dyn Write,
    err: &mut dyn Write,
    tty: bool,
) -> u8 {
    let started = Instant::now();
    let off = match &setup.env {
        Some(pairs) => pairs
            .iter()
            .any(|(name, value)| name == "AGENT_CLI_USAGE" && value.trim() == "off"),
        None => std::env::var("AGENT_CLI_USAGE").is_ok_and(|value| value.trim() == "off"),
    };
    let log = usage::log_path(setup.state_dir.as_deref(), off);
    let mut run = Run::default();
    let result = dispatch(domains, argv, setup, out, err, tty, &mut run).and_then(|exit| {
        out.flush()?;
        Ok(exit)
    });
    let code = match result {
        Ok(exit) => exit.code(),
        // The reader (`| head`) has gone, which is what it asked for.
        Err(error) if is_broken_pipe(&error) => 0,
        Err(error) => {
            let (exit, message, hint) = describe(&error);
            let _ = writeln!(err, "error: {message}");
            if let Some(hint) = hint {
                let _ = writeln!(err, "hint: {}", hint.replace('\n', "\n  "));
            }
            exit.code()
        }
    };
    if let Some(log) = log {
        let flags = usage::flag_names(argv);
        let path = run.path.join(" ");
        usage::append(&log, &path, &flags, code, started.elapsed(), &run.meter);
    }
    code
}

/// What the usage log needs from a run: the path as far as it resolved
/// (never a word the registry does not know), and the meter its `Ctx` counts on.
#[derive(Default)]
struct Run {
    path: Vec<&'static str>,
    meter: Arc<Meter>,
}

impl Run {
    fn ctx(&self, globals: Globals, setup: Setup, command_line: String) -> Ctx {
        let mut ctx = Ctx::new(globals, setup, command_line);
        ctx.meter = Arc::clone(&self.meter);
        ctx
    }
}

fn is_broken_pipe(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::BrokenPipe)
    })
}

fn dispatch(
    domains: &[Domain],
    argv: &[String],
    setup: Setup,
    out: &mut dyn Write,
    err: &mut dyn Write,
    tty: bool,
    run: &mut Run,
) -> Result<Exit> {
    let (mut globals, words) = split_globals(argv)?;
    let command_line = std::iter::once("agent-cli".to_owned())
        .chain(argv.iter().map(|arg| shell_quote(arg)))
        .collect::<Vec<_>>()
        .join(" ");
    let mut words = &words[..];
    if words.first().is_some_and(|word| word == "help") {
        globals.help = true;
        words = &words[1..];
    }
    let Some(first) = words.first() else {
        let hidden = hidden::hidden(domains, &setup.config, setup.state_dir.as_deref());
        let overview = hidden::overview(domains, &setup.config, &hidden);
        writeln!(out, "{overview}")?;
        return Ok(Exit::Ok);
    };
    match first.as_str() {
        "search" => {
            run.path = vec!["search"];
            // Taken quietly, `search --fields id` would look like it worked.
            let ignored = argv.iter().take_while(|arg| *arg != "--").find_map(|arg| {
                let name = arg.split('=').next()?.strip_prefix("--")?;
                (name != "help" && GLOBAL_FLAGS.contains(&name)).then_some(name)
            });
            if let Some(flag) = ignored {
                let refusal = Failure::usage(format!("search does not take --{flag}"));
                return Err(refusal
                    .hint("search takes words, --limit N and --all")
                    .into());
            }
            let hidden = hidden::hidden(domains, &setup.config, setup.state_dir.as_deref());
            return search_builtin(domains, &hidden, &words[1..], globals.help, out, err);
        }
        "doctor" => {
            run.path = vec!["doctor"];
            let ctx = run.ctx(globals, setup, command_line);
            return doctor::builtin(domains, &words[1..], &ctx, out, err, tty);
        }
        "config" => {
            run.path = match words.get(1).map(String::as_str) {
                Some("pull") => vec!["config", "pull"],
                Some("example") => vec!["config", "example"],
                _ => vec!["config"],
            };
            let ctx = run.ctx(globals, setup, command_line);
            return config::builtin(&words[1..], &ctx, out, err, tty);
        }
        "usage" => {
            run.path = vec!["usage"];
            let ctx = run.ctx(globals, setup, command_line);
            return usage::builtin(&words[1..], &ctx, out, err, tty);
        }
        flag if flag.starts_with('-') => {
            return Err(Failure::usage(format!("unknown flag {flag} before the command"))
                .hint("flags go after the command: agent-cli <domain> <resource> <verb> --flag value; `agent-cli` lists the domains")
                .into());
        }
        _ => {}
    }
    let Some(domain) = domains.iter().find(|domain| domain.name == first) else {
        if let Some(error) = search::health_question(domains, words) {
            return Err(error);
        }
        let names = domains
            .iter()
            .map(|domain| domain.name)
            .chain(["search", "doctor", "config", "usage"]);
        return Err(unknown("domain", first, "", names, domains, words, None));
    };
    if let Some(flag) = words[1..words.len().min(3)]
        .iter()
        .find(|word| word.starts_with('-'))
    {
        return Err(
            Failure::usage(format!("{flag} came before the verb; flags go after it"))
                .hint(format!(
                    "agent-cli {}  (lists the resources and their verbs)",
                    domain.name
                ))
                .into(),
        );
    }
    run.path = vec![domain.name];
    let Some(resource) = words.get(1) else {
        writeln!(out, "{}", discover::domain_listing(domain))?;
        return Ok(Exit::Ok);
    };
    let members: Vec<&Command> = domain
        .commands
        .iter()
        .filter(|command| command.path[1] == resource)
        .collect();
    if members.is_empty() {
        let mut resources: Vec<&str> = domain
            .commands
            .iter()
            .map(|command| command.path[1])
            .collect();
        resources.dedup();
        let scope = format!(" in {}", domain.name);
        return Err(unknown(
            "resource", resource, &scope, resources, domains, words, None,
        ));
    }
    run.path.push(members[0].path[1]);
    let Some(verb) = words.get(2) else {
        writeln!(out, "{}", discover::resource_listing(domain, resource))?;
        return Ok(Exit::Ok);
    };
    let Some(command) = members.iter().find(|command| command.path[2] == verb) else {
        let scope = format!(" in {} {resource}", domain.name);
        let verbs = members.iter().map(|command| command.path[2]);
        // `ado run 8809`: the id came before the verb, and `get` is the verb
        // it was meant for.
        let get = members
            .iter()
            .find(|command| command.path[2] == "get")
            .filter(|_| looks_like_id(verb))
            .and_then(|get| Some((*get, get_line(get, verb)?)));
        return Err(unknown("verb", verb, &scope, verbs, domains, words, get));
    };
    run.path = command.path.to_vec();
    if globals.help {
        writeln!(out, "{}", discover::command_help(command))?;
        return Ok(Exit::Ok);
    }
    let matches: ArgMatches = parse_leaf(command, &words[3..])?;
    precheck(command, &globals, &command_line)?;
    if let Some(path) = &globals.output {
        output::check_output(path)?;
    }
    globals.timeout = globals.timeout.or(command.timeout.map(Duration::from_secs));
    let mut ctx = run.ctx(globals, setup, command_line);
    ctx.command = Some(command.path);
    let result = (command.run)(&ctx, &matches);
    let plans = ctx.take_plans();
    let keep_tail = command.path[2] == "logs";
    let printing = ctx.printing();
    let emitted = if plans.is_empty() {
        match result {
            Ok(value) => output::emit(value, &printing, keep_tail, out, err, tty),
            // A failure with an answer (a wait that ended badly) prints it
            // as a success would, then exits with its own code.
            Err(error) => match data_of(&error) {
                // The reader (`| true`) may be gone; the failure still stands.
                Some(data) => match output::emit(data, &printing, keep_tail, out, err, tty) {
                    Err(emitted) if !is_broken_pipe(&emitted) => Err(emitted),
                    _ => Err(error),
                },
                None => Err(error),
            },
        }
    } else if ctx.globals().output.is_some() {
        // The plan is small, and a file would read as the command's answer.
        let refusal = Failure::usage("--dry-run prints its plan; --output has nothing to save");
        Err(refusal.hint("run it again without --output").into())
    } else {
        // The handler stopped at its first change; whatever it made of that
        // error, what it would have done is the answer.
        let plan = json!({"dry_run": true, "would": plans});
        writeln!(out, "{}", dumps(&plan, tty)).map_err(Into::into)
    };
    for note in ctx.take_notes() {
        writeln!(err, "{note}")?;
    }
    emitted.map(|()| Exit::Ok)
}

/// `line` with `flag` added where it is still a flag: before any `--`.
fn with_flag(line: &str, flag: &str) -> String {
    match line.split_once(" -- ") {
        Some((before, after)) => format!("{before} {flag} -- {after}"),
        None => format!("{line} {flag}"),
    }
}

/// Splits the globals out of `argv`, from anywhere on the line up to `--`.
pub(crate) fn split_globals(argv: &[String]) -> Result<(Globals, Vec<String>), Failure> {
    let mut globals = Globals::default();
    let mut rest = Vec::new();
    let mut args = argv.iter();
    while let Some(arg) = args.next() {
        if arg == "--" {
            rest.push(arg.clone());
            rest.extend(args.by_ref().cloned());
            break;
        }
        let (name, inline) = match arg.split_once('=') {
            Some((name, value)) if name.starts_with("--") => (name, Some(value.to_owned())),
            _ => (arg.as_str(), None),
        };
        let switch = match name {
            "--raw" => &mut globals.raw,
            "--dry-run" => &mut globals.dry_run,
            "--yes" => &mut globals.yes,
            "--reveal" => &mut globals.reveal,
            "--no-cache" => &mut globals.no_cache,
            "-h" | "--help" => &mut globals.help,
            "--fields" | "--timeout" | "--output" => {
                let value = inline
                    .or_else(|| args.next().cloned())
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| Failure::usage(format!("{name} needs a value")))?;
                match name {
                    "--fields" => globals.fields = Some(value),
                    "--output" if value == "-" => {
                        return Err(Failure::usage(
                            "--output - would write a file named \"-\"; leave --output out to print the answer",
                        ));
                    }
                    "--output" => globals.output = Some(PathBuf::from(value)),
                    _ => {
                        // A day bounds any one call, and keeps the deadline an Instant holds.
                        let seconds = value
                            .parse::<u64>()
                            .ok()
                            .filter(|seconds| (1..=86_400).contains(seconds));
                        let seconds = seconds.ok_or_else(|| {
                            Failure::usage(format!(
                                "--timeout needs a whole number of seconds from 1 to 86400 (a day), not {value:?}"
                            ))
                        })?;
                        globals.timeout = Some(Duration::from_secs(seconds));
                    }
                }
                continue;
            }
            _ => {
                rest.push(arg.clone());
                continue;
            }
        };
        if inline.is_some() {
            return Err(Failure::usage(format!(
                "{name} is a switch and takes no value"
            )));
        }
        *switch = true;
    }
    Ok((globals, rest))
}

/// The effect checks that need no handler: `--yes` for a destructive
/// command, `--reveal` or `--output` for a secret.
fn precheck(command: &Command, globals: &Globals, command_line: &str) -> Result<(), Failure> {
    let path = command.path.join(" ");
    match command.effect {
        Effect::Destructive if !globals.yes && !globals.dry_run => {
            Err(Failure::usage(format!("`{path}` is destructive; confirm it with --yes"))
                .hint(format!("{}   (or --dry-run to see it first)", with_flag(command_line, "--yes"))))
        }
        Effect::Reveal if !globals.reveal && globals.output.is_none() => Err(Failure::usage(format!(
            "`{path}` prints a secret value, and whatever it prints lands in your transcript"
        ))
        .hint(format!(
            "add --reveal to print it anyway, or --output FILE to write it to a 0600 file and print only the path: {}", with_flag(command_line, "--output FILE")
        ))),
        _ => Ok(()),
    }
}

/// A word no verb has: a digit, a slash, a dot, a colon, `#`, `@` or a
/// capital, as ids have.
fn looks_like_id(word: &str) -> bool {
    !word.is_empty()
        && !word
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'-')
}

/// `get`'s search line with `id` as its first positional.
fn get_line(get: &Command, id: &str) -> Option<String> {
    let args = (get.args)();
    let first = args.get_positionals().next()?;
    Some(search::hit_line(get).replacen(&format!("<{}>", first.get_id()), &shell_quote(id), 1))
}

/// An unknown word: the close names, and the closest commands by search,
/// reads before changes (a failed lookup is rarely a request to delete
/// something). `first`, when given, leads the list.
pub(crate) fn unknown<'a>(
    kind: &str,
    word: &str,
    scope: &str,
    candidates: impl IntoIterator<Item = &'a str>,
    domains: &[Domain],
    words: &[String],
    first: Option<(&Command, String)>,
) -> anyhow::Error {
    let close = did_you_mean(word, candidates);
    let mut message = format!("unknown {kind} {word:?}{scope}");
    if !close.is_empty() {
        message.push_str(&format!(" \u{2014} did you mean {}?", close.join(", ")));
    } else if first.is_some() {
        message.push_str("; an id goes after the verb");
    }
    let query: Vec<&str> = words
        .iter()
        .take(3)
        .map(String::as_str)
        .filter(|word| !word.starts_with('-'))
        .collect();
    let lead = first.as_ref().map(|(command, _)| command.path);
    let mut ranked: Vec<&Command> = search::rank(domains, &query.join(" "))
        .iter()
        .map(|hit| hit.command)
        .filter(|command| Some(command.path) != lead)
        .take(if first.is_some() { 4 } else { 5 })
        .collect();
    ranked.sort_by_key(|command| match command.effect {
        Effect::Read => 0,
        Effect::Reveal | Effect::Varies | Effect::Write => 1,
        Effect::Destructive => 2,
    });
    let hits: Vec<String> = first
        .map(|(_, line)| line)
        .into_iter()
        .chain(ranked.into_iter().map(search::hit_line))
        .collect();
    let hint = if hits.is_empty() {
        format!(
            "`agent-cli{}` lists what there is",
            scope.replacen(" in", "", 1)
        )
    } else {
        format!("closest commands:\n{}", hits.join("\n"))
    };
    Failure::usage(message).hint(hint).into()
}

fn search_builtin(
    domains: &[Domain],
    hidden: &[(&str, hidden::Why)],
    args: &[String],
    help: bool,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<Exit> {
    let usage = "usage: agent-cli search <words> [--limit N] [--all]   e.g. agent-cli search \"list my work items\"; --all includes domains not set up here or denied";
    let mut limit = 6;
    let mut all = false;
    let mut words = Vec::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let value = match arg.split_once('=') {
            Some(("--limit", value)) => Some(value.to_owned()),
            _ if arg == "--limit" => Some(args.next().cloned().unwrap_or_default()),
            _ => None,
        };
        match value {
            Some(value) => {
                limit = value
                    .parse::<usize>()
                    .ok()
                    .filter(|limit| *limit > 0)
                    .ok_or_else(|| Failure::usage("--limit needs a whole number").hint(usage))?;
            }
            None if arg == "--all" => all = true,
            None => words.push(arg.as_str()),
        }
    }
    if help {
        writeln!(out, "{usage}")?;
        return Ok(Exit::Ok);
    }
    if words.is_empty() {
        return Err(Failure::usage("search needs some words").hint(usage).into());
    }
    let query = words.join(" ");
    let left_out: Vec<&str> = if all {
        Vec::new()
    } else {
        hidden.iter().map(|(name, _)| *name).collect()
    };
    let (lines, missed) = search::search_lines(domains, &query, limit, &left_out);
    if lines.is_empty() {
        writeln!(
            out,
            "no matches for {query:?}; try other words, or browse: agent-cli"
        )?;
    } else {
        writeln!(out, "{}", lines.join("\n"))?;
    }
    if !missed.is_empty() {
        let mut names: Vec<&str> = Vec::new();
        for command in &missed {
            if !names.contains(&command.path[0]) {
                names.push(command.path[0]);
            }
        }
        let why = if hidden
            .iter()
            .filter(|(name, _)| names.contains(name))
            .all(|(_, why)| *why == hidden::Why::NotSetUp)
        {
            "not set up here"
        } else {
            "not set up here or denied"
        };
        writeln!(
            err,
            "[{} more in domains {why}: {}; agent-cli search --all {}]",
            missed.len(),
            names.join(", "),
            shell_quote(&query)
        )?;
    }
    Ok(Exit::Ok)
}

/// `word` as a shell would need it typed, for "run it again" hints.
fn shell_quote(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_./:=,@%+".contains(&byte));
    if plain {
        word.to_owned()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

/// An example line split the way a shell would: whitespace, with single or
/// double quotes grouping. No escapes; examples do not need them.
pub(crate) fn shell_words(line: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    for c in line.chars() {
        match quote {
            Some(open) if c == open => quote = None,
            Some(_) => word.push(c),
            None if c == '\'' || c == '"' => {
                quote = Some(c);
                in_word = true;
            }
            None if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            None => {
                word.push(c);
                in_word = true;
            }
        }
    }
    if quote.is_some() {
        return Err("an unclosed quote".to_owned());
    }
    if in_word {
        words.push(word);
    }
    Ok(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_owned()).collect()
    }

    #[test]
    fn globals_are_taken_from_anywhere_until_a_double_dash() {
        let argv = strings(&[
            "--dry-run",
            "ado",
            "pr",
            "--fields=id,title",
            "get",
            "42",
            "--timeout",
            "9",
            "--output",
            "out.json",
            "--yes",
            "--",
            "--raw",
        ]);
        let (globals, rest) = split_globals(&argv).unwrap();
        assert!(globals.dry_run && globals.yes && !globals.raw);
        assert_eq!(globals.fields.as_deref(), Some("id,title"));
        assert_eq!(globals.timeout, Some(Duration::from_secs(9)));
        assert_eq!(globals.output, Some(PathBuf::from("out.json")));
        assert_eq!(rest, strings(&["ado", "pr", "get", "42", "--", "--raw"]));
    }

    #[test]
    fn a_bad_global_is_a_usage_error() {
        for (argv, want) in [
            (
                &["--timeout", "soon"][..],
                "--timeout needs a whole number of seconds from 1 to 86400 (a day), not \"soon\"",
            ),
            (
                &["--timeout", "0"][..],
                "--timeout needs a whole number of seconds from 1 to 86400 (a day), not \"0\"",
            ),
            (
                &["--timeout", "18446744073709551615"][..],
                "--timeout needs a whole number of seconds from 1 to 86400 (a day), not \"18446744073709551615\"",
            ),
            (&["--fields"][..], "--fields needs a value"),
            (&["--raw=yes"][..], "--raw is a switch and takes no value"),
        ] {
            let failure = split_globals(&strings(argv)).unwrap_err();
            assert_eq!(
                (failure.exit, failure.message.as_str()),
                (Exit::Usage, want)
            );
        }
    }

    #[test]
    fn shell_words_group_quotes_and_shell_quote_round_trips() {
        assert_eq!(
            shell_words(r#"ado workitem list --state "In Progress" --text 'a "b"'"#).unwrap(),
            strings(&[
                "ado",
                "workitem",
                "list",
                "--state",
                "In Progress",
                "--text",
                r#"a "b""#
            ])
        );
        assert!(shell_words("x 'open").is_err());
        assert_eq!(shell_quote("--fields=id,title"), "--fields=id,title");
        assert_eq!(shell_quote("it's here"), r"'it'\''s here'");
        assert_eq!(shell_quote(""), "''");
    }
}
