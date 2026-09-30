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
    }
    let overview = discover::overview(domains, &Config::empty());
    if overview.len() > 1024 {
        problems.push(format!(
            "the overview is {} bytes (max 1024)",
            overview.len()
        ));
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
    let help = discover::command_help(command);
    if help.len() > 2048 {
        problems.push(format!("help is {} bytes (max 2048)", help.len()));
    }
    problems
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
