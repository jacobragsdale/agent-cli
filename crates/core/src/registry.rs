//! A command is static data, and the invariant checker keeps 1,000 of them
//! consistent.
//!
//! Registration is explicit: each domain crate exports `pub const DOMAIN:
//! Domain` whose `commands` slice lists its [`Command`]s, each made by
//! [`command!`](crate::command). The macro takes the handler's own arg and
//! return types, so parsing, help and `Returns:` cannot drift from the code.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use clap::{ArgMatches, FromArgMatches};
use schemars::{JsonSchema, Schema};
use serde::Serialize;
use serde_json::Value;

use crate::config::Config;
use crate::ctx::Ctx;
use crate::discover::{self, BIG_LISTING};
use crate::dispatch::{parse_leaf, shell_words, split_globals};
use crate::error::Failure;

/// The closed verb vocabulary. A new verb is a deliberate one-line edit here,
/// so the same action never gets two names.
pub const VERBS: &[&str] = &[
    "list", "get", "create", "update", "delete", "run", "wait", "cancel", "retry", "logs", "vote",
    "complete", "abandon", "link", "comment", "approve", "reject", "connect", "restart", "scale",
    "bench",
];

/// Words core owns; no domain may take one.
pub const BUILTINS: &[&str] = &["search", "doctor", "help"];

/// Flags core strips from anywhere on the line; no command may declare one.
pub const GLOBAL_FLAGS: &[&str] = &[
    "fields", "raw", "dry-run", "yes", "reveal", "timeout", "output", "no-cache", "help",
];

/// Flag names agents reach for that mean a canonical one. Declaring one is
/// refused, naming the canonical flag, so each idea keeps one name across
/// every domain (and a spec generator must rename `from`/`to`, `$top` …).
pub const SYNONYM_FLAGS: &[(&str, &str)] = &[
    ("from", "since"),
    ("after", "since"),
    ("start", "since"),
    ("to", "until"),
    ("before", "until"),
    ("end", "until"),
    ("count", "limit"),
    ("top", "limit"),
    ("max", "limit"),
    ("lines", "tail"),
    ("ns", "namespace"),
    ("context", "cluster"),
    ("connection", "conn"),
];

/// Flags that name a person; their help must say `@me` works.
const IDENTITY_FLAGS: &[&str] = &["assignee", "author", "reviewer", "creator", "owner"];

/// Words that are one domain's synonym and another's resource, each with
/// labeled queries for both readings in `search.toml`. Anything else that
/// collides is refused: search would weigh the two readings the same.
/// `task`: ado's Task work item and airflow's task instance.
pub const SHARED_WORDS: &[&str] = &["task"];

/// The longest status line the overview shows for a domain; longer ones are
/// cut. What keeps the overview under 1 KB with every domain configured.
pub(crate) const STATUS_MAX: usize = 22;

/// What running a command can do. Help and search show it, and core checks
/// it before the handler runs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Effect {
    Read,
    Write,
    /// Needs `--yes`: deletes, completes, approves, restarts.
    Destructive,
    /// Prints a secret value: needs `--reveal`, or `--output FILE`.
    Reveal,
    /// Read or write depending on the input, like SQL `query run`. The handler
    /// classifies each call and sends writes through [`Ctx::write`].
    Varies,
}

#[derive(Clone, Copy)]
pub struct Command {
    /// Domain, resource, verb.
    pub path: [&'static str; 3],
    /// Imperative, at most 80 characters, no trailing period.
    pub summary: &'static str,
    pub effect: Effect,
    /// Extra search terms: the words an agent might use instead of the path.
    pub keywords: &'static [&'static str],
    /// A runnable line starting with the path; a test parses it.
    pub example: &'static str,
    /// Seconds the command gets when `--timeout` is not given; `None` is
    /// core's 60. For waits, which should use most of an agent shell's two
    /// minutes.
    pub timeout: Option<u64>,
    pub args: fn() -> clap::Command,
    pub returns: fn() -> Schema,
    pub run: fn(&Ctx, &ArgMatches) -> Result<Value>,
}

pub struct Domain {
    pub name: &'static str,
    /// A few words for the overview: "Azure DevOps".
    pub summary: &'static str,
    pub commands: &'static [Command],
    /// Words agents use for this domain's resources: `("ticket", &["workitem"])`.
    /// A key may be a phrase (`"pull request"`).
    pub synonyms: &'static [(&'static str, &'static [&'static str])],
    /// The overview's config line for this domain, such as "sql 3
    /// connections". Cheap: config only, no network, no processes.
    pub status: fn(&Config) -> String,
    /// Live checks for `agent-cli doctor`.
    pub doctor: fn(&Ctx) -> Vec<Check>,
}

/// One line of `agent-cli doctor`.
#[derive(Clone, Debug, Serialize)]
pub struct Check {
    pub check: String,
    pub ok: bool,
    pub detail: String,
    pub hint: Option<String>,
}

impl Check {
    #[must_use]
    pub fn ok(check: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            check: check.into(),
            ok: true,
            detail: detail.into(),
            hint: None,
        }
    }

    #[must_use]
    pub fn failed(
        check: impl Into<String>,
        detail: impl Into<String>,
        hint: impl Into<String>,
    ) -> Self {
        Self {
            check: check.into(),
            ok: false,
            detail: detail.into(),
            hint: Some(hint.into()),
        }
    }
}

/// Makes a [`Command`] constant from a typed handler.
///
/// ```ignore
/// command! {
///     pub LIST = ["ado", "workitem", "list"], Read,
///     "List work items matching filters",
///     keywords: ["ticket", "bug", "backlog"],
///     example: "ado workitem list --state Active --fields id,title",
///     timeout: 100, // optional: seconds when --timeout is not given
///     run: list,
/// }
/// fn list(ctx: &Ctx, args: ListArgs) -> anyhow::Result<Vec<WorkItemRow>> { … }
/// ```
///
/// `ListArgs` derives `clap::Args`, `WorkItemRow` derives `Serialize` and
/// `JsonSchema`; both are read off `list`'s signature.
#[macro_export]
macro_rules! command {
    (
        $vis:vis $name:ident = [$domain:literal, $resource:literal, $verb:literal], $effect:ident,
        $summary:literal,
        keywords: [$($keyword:literal),* $(,)?],
        example: $example:literal,
        $(timeout: $timeout:literal,)?
        run: $handler:path $(,)?
    ) => {
        $vis const $name: $crate::Command = {
            fn args() -> $crate::clap::Command {
                $crate::args_of($handler)
            }
            fn returns() -> $crate::schemars::Schema {
                $crate::returns_of($handler)
            }
            fn run(
                ctx: &$crate::Ctx,
                matches: &$crate::clap::ArgMatches,
            ) -> $crate::anyhow::Result<$crate::serde_json::Value> {
                $crate::invoke(ctx, matches, $handler)
            }
            $crate::Command {
                path: [$domain, $resource, $verb],
                summary: $summary,
                effect: $crate::Effect::$effect,
                keywords: &[$($keyword),*],
                example: $example,
                timeout: {
                    #[allow(unused_mut, unused_assignments)]
                    let mut timeout: Option<u64> = None;
                    $(timeout = Some($timeout);)?
                    timeout
                },
                args,
                returns,
                run,
            }
        };
    };
}

#[doc(hidden)]
pub fn args_of<A: clap::Args, R>(_: fn(&Ctx, A) -> Result<R>) -> clap::Command {
    A::augment_args(clap::Command::new("agent-cli"))
}

#[doc(hidden)]
pub fn returns_of<A, R: JsonSchema>(_: fn(&Ctx, A) -> Result<R>) -> Schema {
    schemars::schema_for!(R)
}

#[doc(hidden)]
pub fn invoke<A: FromArgMatches, R: Serialize>(
    ctx: &Ctx,
    matches: &ArgMatches,
    handler: fn(&Ctx, A) -> Result<R>,
) -> Result<Value> {
    let args = A::from_arg_matches(matches).map_err(|error| Failure::usage(error.to_string()))?;
    Ok(serde_json::to_value(handler(ctx, args)?)?)
}

/// Every way the registry breaks the rules that keep it usable at 1,000
/// commands, one line each. Empty means clean; tests assert that.
#[must_use]
pub fn check_registry(domains: &[Domain]) -> Vec<String> {
    let mut problems = Vec::new();
    let mut names = HashSet::new();
    let mut paths = HashSet::new();
    // Flag name -> (signature, the command that set it).
    let mut flags: HashMap<String, (&str, String)> = HashMap::new();
    for domain in domains {
        if !is_kebab(domain.name) {
            problems.push(format!(
                "domain {:?} is not a lowercase kebab word",
                domain.name
            ));
        }
        if BUILTINS.contains(&domain.name) {
            problems.push(format!(
                "domain {} shadows the built-in `agent-cli {0}`",
                domain.name
            ));
        }
        if !names.insert(domain.name) {
            problems.push(format!("domain {} is registered twice", domain.name));
        }
        for command in domain.commands {
            let path = command.path.join(" ");
            if !paths.insert(path.clone()) {
                problems.push(format!("{path}: registered twice"));
            }
            problems.extend(
                check_command(domain, command)
                    .into_iter()
                    .map(|problem| format!("{path}: {problem}")),
            );
            for arg in (command.args)().get_arguments() {
                let Some(long) = arg.get_long().filter(|_| !arg.is_positional()) else {
                    continue;
                };
                let signature = discover::arg_signature(arg);
                match flags.get(long) {
                    Some((held, first)) if *held != signature => problems.push(format!(
                        "{path}: --{long} takes {signature} here but {held} in {first}"
                    )),
                    Some(_) => {}
                    None => {
                        flags.insert(long.to_owned(), (signature, path.clone()));
                    }
                }
            }
        }
        problems.extend(check_listings(domain));
        problems.extend(check_synonyms(domain, domains));
    }
    let overview = discover::overview(domains, &Config::empty());
    let crowded = discover::overview_at_most(domains);
    for (text, when) in [
        (overview, "with no config"),
        (crowded, "with every domain configured"),
    ] {
        if text.len() > 1024 {
            problems.push(format!(
                "the overview is {} bytes {when} (max 1024)",
                text.len()
            ));
        }
    }
    problems
}

/// A domain synonym that is another domain's resource ties the two in
/// search, unless it is a [`SHARED_WORDS`] word with queries for both.
fn check_synonyms(domain: &Domain, domains: &[Domain]) -> Vec<String> {
    let mut problems = Vec::new();
    for (key, _) in domain.synonyms {
        for other in domains.iter().filter(|other| other.name != domain.name) {
            if other.commands.iter().any(|command| command.path[1] == *key)
                && !SHARED_WORDS.contains(key)
            {
                problems.push(format!(
                    "{}'s synonym {key:?} is a {} resource; add it to SHARED_WORDS with labeled queries for both readings, or drop it",
                    domain.name, other.name
                ));
            }
        }
    }
    problems
}

fn check_command(domain: &Domain, command: &Command) -> Vec<String> {
    let mut problems = Vec::new();
    if command.path[0] != domain.name {
        problems.push(format!("listed under domain {}", domain.name));
    }
    for word in command.path {
        if !is_kebab(word) {
            problems.push(format!("{word:?} is not a lowercase kebab word"));
        }
    }
    if !VERBS.contains(&command.path[2]) {
        problems.push(format!(
            "verb {:?} is not in VERBS (adding one is a deliberate edit in registry.rs)",
            command.path[2]
        ));
    }
    let length = command.summary.chars().count();
    if length == 0 || length > 80 {
        problems.push(format!("summary is {length} chars (1 to 80)"));
    }
    if command.summary.ends_with('.') {
        problems.push("summary ends with a period".to_owned());
    }
    match example_problem(command) {
        Some(problem) => problems.push(problem),
        None => {
            let uses_fields = command
                .example
                .split_whitespace()
                .any(|word| word == "--fields" || word.starts_with("--fields="));
            if !uses_fields && discover::returns_list_of_objects(&(command.returns)()) {
                problems.push(
                    "returns a list of objects, so its example should use --fields".to_owned(),
                );
            }
        }
    }
    for arg in (command.args)().get_arguments() {
        if let Some(long) = arg.get_long()
            && GLOBAL_FLAGS.contains(&long)
        {
            problems.push(format!("--{long} shadows the core global"));
        }
        if arg.get_short() == Some('h') {
            problems.push("-h shadows --help".to_owned());
        }
    }
    problems.extend(check_conventions(command));
    let help = discover::command_help(command);
    if help.len() > 2048 {
        problems.push(format!("help is {} bytes (max 2048)", help.len()));
    }
    problems
}

/// The cross-domain flag conventions: canonical names, times only as
/// `--since`/`--until`, `@me` for people, `--limit` on lists and `--tail` on
/// logs.
fn check_conventions(command: &Command) -> Vec<String> {
    let mut problems = Vec::new();
    let args = (command.args)();
    let flag = |name: &str| {
        args.get_arguments()
            .find(|arg| !arg.is_positional() && arg.get_long() == Some(name))
    };
    for arg in args.get_arguments().filter(|arg| !arg.is_positional()) {
        let Some(long) = arg.get_long() else { continue };
        if let Some((_, canonical)) = SYNONYM_FLAGS.iter().find(|(name, _)| *name == long) {
            problems.push(format!("--{long} is a synonym: name it --{canonical}"));
        }
        let time = discover::arg_kind(arg) == "time";
        let named = matches!(long, "since" | "until");
        if time && !named {
            problems.push(format!(
                "--{long} takes a time, which only --since and --until do"
            ));
        }
        if named && !time {
            problems.push(format!("--{long} must take a time (core's When)"));
        }
        let help = arg.get_help().map(ToString::to_string).unwrap_or_default();
        if IDENTITY_FLAGS.contains(&long) && !help.contains("@me") {
            problems.push(format!(
                "--{long} names a person, so its help must say @me works"
            ));
        }
    }
    // A list with no arguments at all reads config, not a service.
    let bounded = args.get_arguments().next().is_none()
        || flag("limit").is_some_and(|limit| {
            discover::arg_kind(limit) == "int"
                && limit.get_default_values().first().and_then(|v| v.to_str()) == Some("50")
        });
    if command.path[2] == "list" && !bounded {
        problems.push("a list takes --limit (int, default 50)".to_owned());
    }
    if command.path[2] == "logs" {
        if flag("tail").is_none() {
            problems.push("logs take --tail".to_owned());
        }
        for endless in ["follow", "watch"] {
            if flag(endless).is_some() {
                problems.push(format!("logs never --{endless}: a command must end"));
            }
        }
    }
    problems
}

/// The command lines after each `agent-cli ` in `text` (a hint, a note, an
/// example), each cut where the prose resumes: a backtick, a bracket, `;`,
/// `, `, two spaces, ` (`, or the end of the line, outside quotes (a quoted
/// argument, such as SQL, may hold any of them).
pub(crate) fn printed_commands(text: &str) -> Vec<String> {
    const STOPS: [&str; 8] = ["\n", "`", ")", "]", ";", ", ", "  ", " ("];
    text.match_indices("agent-cli ")
        .filter_map(|(at, marker)| {
            let rest = &text[at + marker.len()..];
            let mut quote: Option<char> = None;
            let mut end = rest.len();
            for (index, c) in rest.char_indices() {
                match quote {
                    Some(open) if c == open => quote = None,
                    Some(_) => {}
                    None if c == '\'' || c == '"' => quote = Some(c),
                    None if STOPS.iter().any(|stop| rest[index..].starts_with(stop)) => {
                        end = index;
                        break;
                    }
                    None => {}
                }
            }
            let line = rest[..end].trim().trim_end_matches(['.', ',']);
            (!line.is_empty()).then(|| line.to_owned())
        })
        .collect()
}

/// Why a printed `agent-cli …` line would not run, if it would not. An
/// ALL-CAPS word (`ID`, `NAME`, `N`) is a placeholder and stands in for a
/// value; a usage pattern (`<domain>`, `[<verb>]`) is not a call. A domain
/// `domains` does not hold cannot be checked here and passes.
pub(crate) fn printed_command_problem(domains: &[Domain], line: &str) -> Option<String> {
    let words = match shell_words(line) {
        Ok(words) => words,
        Err(error) => return Some(error),
    };
    if words
        .iter()
        .any(|word| word.contains(['<', '[', '\u{2026}']) || word == "...")
    {
        return None;
    }
    let words: Vec<String> = words
        .into_iter()
        .map(|word| {
            let placeholder = word.starts_with(|c: char| c.is_ascii_uppercase())
                && word
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
            if placeholder { "1".to_owned() } else { word }
        })
        .collect();
    let (globals, words) = match split_globals(&words) {
        Ok(split) => split,
        Err(failure) => return Some(failure.message),
    };
    let first = words.first()?;
    if BUILTINS.contains(&first.as_str()) {
        return None;
    }
    let domain = domains.iter().find(|domain| domain.name == first)?;
    let resource = words.get(1)?;
    if !domain
        .commands
        .iter()
        .any(|command| command.path[1] == resource)
    {
        return Some(format!("{} has no resource {resource:?}", domain.name));
    }
    let verb = words.get(2)?;
    let Some(command) = domain
        .commands
        .iter()
        .find(|command| command.path[1] == resource && command.path[2] == verb)
    else {
        return Some(format!("{} {resource} has no verb {verb:?}", domain.name));
    };
    if globals.help {
        return None;
    }
    parse_leaf(command, &words[3..])
        .err()
        .map(|failure| failure.message)
}

/// Why the example does not run as written, if it does not.
fn example_problem(command: &Command) -> Option<String> {
    let words = match shell_words(command.example) {
        Ok(words) => words,
        Err(error) => return Some(format!("example: {error}")),
    };
    if words.len() < 3 || words[..3] != command.path {
        return Some(format!(
            "example must start with `{}`",
            command.path.join(" ")
        ));
    }
    let parsed =
        split_globals(&words[3..]).and_then(|(_, rest)| parse_leaf(command, &rest).map(drop));
    parsed
        .err()
        .map(|failure: Failure| format!("example does not parse: {}", failure.message))
}

/// Listings above [`BIG_LISTING`] entries must print names only.
fn check_listings(domain: &Domain) -> Vec<String> {
    let mut problems = Vec::new();
    let mut resources: Vec<&str> = Vec::new();
    for command in domain.commands {
        if !resources.contains(&command.path[1]) {
            resources.push(command.path[1]);
        }
    }
    if resources.len() > BIG_LISTING {
        let text = discover::domain_listing(domain);
        if !text.contains("names only") {
            problems.push(format!("the {} listing is not names-only", domain.name));
        }
    }
    for resource in resources {
        let commands: Vec<&Command> = domain
            .commands
            .iter()
            .filter(|command| command.path[1] == resource)
            .collect();
        if commands.len() > BIG_LISTING {
            let text = discover::resource_listing(domain, resource);
            if commands
                .iter()
                .any(|command| text.contains(command.summary))
            {
                problems.push(format!(
                    "the {} {resource} listing is not names-only",
                    domain.name
                ));
            }
        }
    }
    problems
}

fn is_kebab(word: &str) -> bool {
    !word.is_empty()
        && word.starts_with(|c: char| c.is_ascii_lowercase())
        && !word.ends_with('-')
        && !word.contains("--")
        && word
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printed_command_lines_are_cut_where_the_prose_resumes() {
        let text = "error: x\nhint: agent-cli ado run get 991 --fields failed, then agent-cli ado run logs 991\n\
                    hint: agent-cli k8s pod delete p --yes   (or --dry-run to see it first)\n\
                    hint: re-read it (agent-cli ado workitem get 42 --fields rev), or `agent-cli doctor ado` \n\
                    [first 50; agent-cli <domain> <resource>]";
        assert_eq!(
            printed_commands(text),
            [
                "ado run get 991 --fields failed",
                "ado run logs 991",
                "k8s pod delete p --yes",
                "ado workitem get 42 --fields rev",
                "doctor ado",
                "<domain> <resource>"
            ]
        );
    }

    #[test]
    fn kebab_words_are_lowercase_letters_digits_and_single_hyphens() {
        for good in ["ado", "work-item", "k8s", "v2-api"] {
            assert!(is_kebab(good), "{good}");
        }
        for bad in [
            "",
            "Ado",
            "work_item",
            "-x",
            "x-",
            "a--b",
            "2fa",
            "work item",
        ] {
            assert!(!is_kebab(bad), "{bad}");
        }
    }
}
