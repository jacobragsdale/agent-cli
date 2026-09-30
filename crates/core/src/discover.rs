//! The text an agent reads to find its way: the overview, listings, per-command
//! help and the `Returns:` shape.
//!
//! All of it is rendered from the registry, the handler's clap args and its
//! return schema, so nothing here can describe a flag or a field the command
//! does not have. Sizes are budgets the invariant checker enforces: the
//! overview under 1 KB at any registry size, help under 2 KB, and listings
//! above 40 entries print names only.

use std::any::TypeId;
use std::path::PathBuf;

use clap::{Arg, ArgAction};
use serde_json::{Map, Value};
use time::macros::format_description;

use crate::config::Config;
use crate::registry::{Command, Domain, Effect, STATUS_MAX};
use crate::search::STOP;
use crate::when::{SPAN_FORMS, Span, TIME_FORMS, When};

/// The most the overview's `Config:` line gives the status lines, in bytes:
/// eight domains at [`STATUS_MAX`] fit, and a registry of more still keeps
/// the overview under 1 KB.
const CONFIG_MAX: usize = 210;
/// Above this many entries a listing prints names only.
pub(crate) const BIG_LISTING: usize = 40;
/// How deep `Returns:` shows nested fields before it shows names only.
const SHAPE_DEPTH: usize = 3;
const WRAP: usize = 100;

pub(crate) fn overview(domains: &[Domain], config: &Config) -> String {
    let mut statuses: Vec<String> = domains
        .iter()
        .map(|domain| status_line(&(domain.status)(config)))
        .filter(|status| !status.is_empty())
        .collect();
    if config.problem().is_some() {
        statuses.insert(0, "config file unreadable".to_owned());
    }
    render_overview(domains, statuses)
}

/// The overview with every domain's status line at [`STATUS_MAX`], the
/// longest it can print: what the 1 KB budget must hold.
pub(crate) fn overview_at_most(domains: &[Domain]) -> String {
    let statuses = domains
        .iter()
        .map(|domain| status_line(&format!("{} {}", domain.name, "x".repeat(STATUS_MAX))))
        .collect();
    render_overview(domains, statuses)
}

/// The status lines joined, cut at a separator once they pass
/// [`CONFIG_MAX`] bytes: doctor has the rest.
fn config_line(statuses: &[String]) -> String {
    let mut line = String::new();
    for status in statuses {
        let separator = if line.is_empty() { "" } else { " \u{b7} " };
        if line.len() + separator.len() + status.len() > CONFIG_MAX {
            line.push_str(" \u{b7} \u{2026}");
            break;
        }
        line.push_str(separator);
        line.push_str(status);
    }
    line
}

/// A status line cut to [`STATUS_MAX`] characters.
fn status_line(status: &str) -> String {
    let status = status.trim();
    if status.chars().count() <= STATUS_MAX {
        return status.to_owned();
    }
    let kept: String = status.chars().take(STATUS_MAX - 1).collect();
    format!("{kept}\u{2026}")
}

fn render_overview(domains: &[Domain], mut statuses: Vec<String>) -> String {
    let total: usize = domains.iter().map(|domain| domain.commands.len()).sum();
    let summaries: Vec<&str> = domains.iter().map(|domain| domain.summary).collect();
    let title = match summaries.join(", ") {
        _ if domains.is_empty() => {
            "agent-cli: tools for coding agents \u{2014} no commands yet. Output: JSON.".to_owned()
        }
        joined if joined.len() <= WRAP => {
            format!(
                "agent-cli: {joined} for coding agents \u{2014} {total} commands. Output: JSON."
            )
        }
        _ => format!(
            "agent-cli: {} services for coding agents \u{2014} {total} commands. Output: JSON.",
            domains.len()
        ),
    };
    if statuses.is_empty() {
        statuses.push("nothing to set up".to_owned());
    }
    let now = crate::when::now()
        .format(format_description!("[year]-[month]-[day]T[hour]:[minute]Z"))
        .unwrap_or_default();
    let counts: Vec<String> = domains
        .iter()
        .map(|domain| format!("{}({})", domain.name, domain.commands.len()))
        .collect();
    let mut lines = vec![
        title,
        format!(
            "Start here:  agent-cli search <what you want to do>    e.g. agent-cli search \"{}\"",
            sample_search(domains)
        ),
        "Browse:      agent-cli <domain> [<resource>]    Details: agent-cli <domain> <resource> <verb> --help".to_owned(),
        "Flags:       --fields a,b.c  --raw  --dry-run  --yes  --reveal  --timeout S (60)  --output FILE  --no-cache".to_owned(),
        "Exit:        0 ok \u{b7} 1 failed \u{b7} 2 fix the call \u{b7} 3 needs setup (run doctor) \u{b7} 4 not found \u{b7} 5 conflict \u{b7} 124 timed out".to_owned(),
        format!("Config:      {}    Live check: agent-cli doctor", config_line(&statuses)),
        format!("Now:         {now}"),
    ];
    if counts.is_empty() {
        lines.push("Domains:     none yet".to_owned());
    } else {
        for (index, row) in wrap(&counts).into_iter().enumerate() {
            let label = if index == 0 { "Domains:" } else { "" };
            lines.push(format!("{label:<13}{row}"));
        }
    }
    lines.join("\n")
}

/// A search worth trying, taken from the first list command's summary.
fn sample_search(domains: &[Domain]) -> String {
    let summary = domains
        .iter()
        .flat_map(|domain| domain.commands)
        .find(|command| command.path[2] == "list")
        .map_or("list open work items", |command| command.summary);
    let mut words: Vec<String> = summary
        .to_lowercase()
        .split_whitespace()
        .take(3)
        .map(str::to_owned)
        .collect();
    while words.len() > 1
        && words
            .last()
            .is_some_and(|word| STOP.contains(&word.as_str()))
    {
        words.pop();
    }
    words.join(" ")
}

/// `agent-cli ado`: the domain's resources with their verbs, in registration order.
pub(crate) fn domain_listing(domain: &Domain) -> String {
    let mut resources: Vec<(&str, Vec<&str>)> = Vec::new();
    for command in domain.commands {
        match resources
            .iter_mut()
            .find(|(name, _)| *name == command.path[1])
        {
            Some((_, verbs)) => verbs.push(command.path[2]),
            None => resources.push((command.path[1], vec![command.path[2]])),
        }
    }
    let entries = resources
        .into_iter()
        .map(|(name, verbs)| (name.to_owned(), verbs.join(" ")))
        .collect();
    listing(
        format!(
            "agent-cli {}: {} \u{2014} {} commands",
            domain.name,
            domain.summary,
            domain.commands.len()
        ),
        "resources",
        entries,
        format!(
            "Details: agent-cli {} <resource> [<verb> --help]   Faster: agent-cli search <words>",
            domain.name
        ),
    )
}

/// `agent-cli ado pr`: the resource's verbs with their summaries.
pub(crate) fn resource_listing(domain: &Domain, resource: &str) -> String {
    let entries: Vec<(String, String)> = domain
        .commands
        .iter()
        .filter(|command| command.path[1] == resource)
        .map(|command| {
            (
                command.path[2].to_owned(),
                format!("{}{}", command.summary, command.effect.tag()),
            )
        })
        .collect();
    listing(
        format!(
            "agent-cli {} {}: {} commands",
            domain.name,
            resource,
            entries.len()
        ),
        "commands",
        entries,
        format!(
            "Details: agent-cli {} {resource} <verb> --help   Faster: agent-cli search <words>",
            domain.name
        ),
    )
}

/// A titled listing: name and detail per line, or names only above
/// [`BIG_LISTING`] entries.
pub(crate) fn listing(
    title: String,
    noun: &str,
    entries: Vec<(String, String)>,
    footer: String,
) -> String {
    let mut lines = vec![title];
    if entries.len() > BIG_LISTING {
        let names: Vec<String> = entries.into_iter().map(|(name, _)| name).collect();
        let count = names.len();
        lines.extend(wrap(&names).into_iter().map(|row| format!("  {row}")));
        lines.push(format!(
            "({count} {noun}, names only. Find by intent: agent-cli search <words>)"
        ));
    } else {
        let width = entries
            .iter()
            .map(|(name, _)| name.len())
            .max()
            .unwrap_or(0);
        for (name, detail) in entries {
            lines.push(format!("  {name:<width$}  {detail}").trim_end().to_owned());
        }
    }
    lines.push(footer);
    lines.join("\n")
}

/// Words packed into rows of at most [`WRAP`] characters.
fn wrap(words: &[String]) -> Vec<String> {
    let mut rows = Vec::new();
    let mut row = String::new();
    for word in words {
        if !row.is_empty() && row.len() + word.len() + 2 > WRAP {
            rows.push(std::mem::take(&mut row));
        }
        if !row.is_empty() {
            row.push_str("  ");
        }
        row.push_str(word);
    }
    if !row.is_empty() {
        rows.push(row);
    }
    rows
}

/// `agent-cli <path> --help`: summary, args (`*` required), `Returns:`, the
/// effect, and a runnable example. Public for the generated command
/// reference (`docs/reference/<domain>.md`).
#[must_use]
pub fn command_help(command: &Command) -> String {
    let path = command.path.join(" ");
    let mut lines = vec![format!("agent-cli {path} \u{2014} {}", command.summary)];
    let args = (command.args)();
    let mut shown: Vec<&Arg> = args
        .get_arguments()
        .filter(|arg| !arg.is_hide_set())
        .collect();
    shown.sort_by_key(|arg| (!arg.is_positional(), !arg.is_required_set()));
    let lefts: Vec<String> = shown.iter().map(|arg| arg_left(arg)).collect();
    let width = lefts.iter().map(String::len).max().unwrap_or(0).min(28);
    for (arg, left) in shown.iter().zip(&lefts) {
        let required = if arg.is_required_set() { '*' } else { ' ' };
        let help = arg
            .get_help()
            .map(|help| {
                help.to_string()
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_owned()
            })
            .unwrap_or_default();
        let defaults: Vec<String> = arg
            .get_default_values()
            .iter()
            .map(|value| value.to_string_lossy().into_owned())
            .collect();
        let default = if defaults.is_empty() || is_switch(arg) {
            String::new()
        } else {
            format!("(default {})", defaults.join(","))
        };
        let text = [help, default]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        lines.push(
            format!(" {required}{left:<width$}  {text}")
                .trim_end()
                .to_owned(),
        );
    }
    for (kind, forms) in [("time", TIME_FORMS), ("duration", SPAN_FORMS)] {
        if shown.iter().any(|arg| arg_kind(arg) == kind) {
            lines.push(format!("A {kind} is {forms}."));
        }
    }
    lines.push(format!("Returns: {}", returns_shape(&(command.returns)())));
    let required = if shown.iter().any(|arg| arg.is_required_set()) {
        "* required. "
    } else {
        ""
    };
    let timeout = command
        .timeout
        .map_or_else(String::new, |seconds| format!(" (default {seconds}s)"));
    lines.push(format!(
        "{} {required}Globals: --fields --raw --timeout{timeout} --output",
        command.effect.sentence()
    ));
    lines.push(format!("e.g. agent-cli {}", command.example));
    lines.join("\n")
}

fn arg_left(arg: &Arg) -> String {
    let label = type_label(arg);
    let name = if arg.is_positional() {
        format!("<{}>", arg.get_id())
    } else {
        format!("--{}", arg.get_long().unwrap_or(arg.get_id().as_str()))
    };
    if label.is_empty() {
        name
    } else {
        format!("{name} {label}")
    }
}

/// What `search` shows after a command's path: its positionals, and its
/// required flags each with a placeholder value (`--conn CONN`), so the line
/// reads as a call to fill in.
pub(crate) fn required_args(command: &Command) -> String {
    let args = (command.args)();
    args.get_arguments()
        .filter(|arg| arg.is_required_set() && !arg.is_hide_set())
        .map(|arg| match arg.get_long() {
            Some(long) if !arg.is_positional() && is_switch(arg) => format!("--{long}"),
            Some(long) if !arg.is_positional() => {
                format!("--{long} {}", long.to_ascii_uppercase().replace('-', "_"))
            }
            _ => format!("<{}>", arg.get_id()),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_switch(arg: &Arg) -> bool {
    matches!(arg.get_action(), ArgAction::SetTrue | ArgAction::SetFalse)
}

fn is_many(arg: &Arg) -> bool {
    matches!(arg.get_action(), ArgAction::Append | ArgAction::Count)
}

/// The kind of value an arg takes: `str`, `int`, `num`, `path`, `bool`,
/// `time` ([`When`]), `duration` ([`Span`]), `enum`, or `value` for anything
/// else; empty for a switch.
pub(crate) fn arg_kind(arg: &Arg) -> &'static str {
    if is_switch(arg) {
        return "";
    }
    let id = arg.get_value_parser().type_id();
    let is = |candidates: &[TypeId]| candidates.iter().any(|candidate| id == *candidate);
    if is(&[TypeId::of::<String>()]) {
        "str"
    } else if is(&[TypeId::of::<When>()]) {
        "time"
    } else if is(&[TypeId::of::<Span>()]) {
        "duration"
    } else if is(&[
        TypeId::of::<i64>(),
        TypeId::of::<i32>(),
        TypeId::of::<i16>(),
        TypeId::of::<i8>(),
        TypeId::of::<u64>(),
        TypeId::of::<u32>(),
        TypeId::of::<u16>(),
        TypeId::of::<u8>(),
        TypeId::of::<usize>(),
        TypeId::of::<isize>(),
    ]) {
        "int"
    } else if is(&[TypeId::of::<f64>(), TypeId::of::<f32>()]) {
        "num"
    } else if is(&[TypeId::of::<PathBuf>()]) {
        "path"
    } else if is(&[TypeId::of::<bool>()]) {
        "bool"
    } else if arg.get_possible_values().is_empty() {
        "value"
    } else {
        "enum"
    }
}

/// What the same flag name must agree on across every command: the kind of
/// value. How many it takes may differ, since `--state` filters by several on
/// a list and sets one on an update.
pub(crate) fn arg_signature(arg: &Arg) -> &'static str {
    if is_switch(arg) {
        "switch"
    } else {
        arg_kind(arg)
    }
}

/// How help shows an arg's value: its possible values, or its kind.
pub(crate) fn type_label(arg: &Arg) -> String {
    let many = if is_many(arg) { "[]" } else { "" };
    let values: Vec<String> = arg
        .get_possible_values()
        .iter()
        .filter(|value| !value.is_hide_set())
        .map(|value| value.get_name().to_owned())
        .collect();
    if !values.is_empty() && !is_switch(arg) {
        let more = if values.len() > 12 { "|\u{2026}" } else { "" };
        return format!("{}{more}{many}", values[..values.len().min(12)].join("|"));
    }
    format!("{}{many}", arg_kind(arg))
}

impl Effect {
    /// After a summary in listings and search.
    pub(crate) const fn tag(self) -> &'static str {
        match self {
            Self::Read => "",
            Self::Write => " (write)",
            Self::Destructive => " (destructive)",
            Self::Reveal => " (reveals a secret)",
            Self::Varies => " (read or write)",
        }
    }

    const fn sentence(self) -> &'static str {
        match self {
            Self::Read => "Read.",
            Self::Write => "Write: --dry-run shows the change without making it.",
            Self::Destructive => {
                "Destructive: needs --yes; --dry-run shows the change without making it."
            }
            Self::Reveal => {
                "Reveals a secret: --reveal prints it; --output FILE saves it (0600) and prints only the path."
            }
            Self::Varies => {
                "Read or write, decided by the input: a write honours --dry-run and may need --yes."
            }
        }
    }
}

/// A return schema as a compact shape: `[{id,title,tags[],reviewers[{name,vote}]}]`.
pub(crate) fn returns_shape(schema: &schemars::Schema) -> String {
    let root = schema.as_value();
    render(root, root, 0)
}

/// True when the command returns a list of objects, which is when its
/// example should show `--fields`.
pub(crate) fn returns_list_of_objects(schema: &schemars::Schema) -> bool {
    let root = schema.as_value();
    let resolved = resolve(root, root);
    resolved["type"] == "array" && !properties(&resolve(root, &resolved["items"])).is_empty()
}

/// Every field name down to [`SHAPE_DEPTH`], for search.
pub(crate) fn return_fields(schema: &schemars::Schema) -> Vec<String> {
    fn walk(root: &Value, schema: &Value, depth: usize, out: &mut Vec<String>) {
        let schema = resolve(root, schema);
        if schema["type"] == "array" {
            return walk(root, &schema["items"], depth, out);
        }
        if depth >= SHAPE_DEPTH {
            return;
        }
        for (name, child) in properties(&schema) {
            out.push(name.clone());
            walk(root, child, depth + 1, out);
        }
    }
    let root = schema.as_value();
    let mut out = Vec::new();
    walk(root, root, 0, &mut out);
    out
}

fn render(root: &Value, schema: &Value, depth: usize) -> String {
    let schema = resolve(root, schema);
    if schema["type"] == "array" {
        return format!("[{}]", render(root, &schema["items"], depth));
    }
    let fields = properties(&schema);
    if !fields.is_empty() {
        if depth >= SHAPE_DEPTH {
            return "{\u{2026}}".to_owned();
        }
        let inner: Vec<String> = fields
            .iter()
            .map(|(name, child)| field(root, name, child, depth + 1))
            .collect();
        return format!("{{{}}}", inner.join(","));
    }
    match schema.get("type").and_then(Value::as_str) {
        Some("string") => "str",
        Some("integer") => "int",
        Some("number") => "num",
        Some("boolean") => "bool",
        Some("null") => "nothing",
        Some("object") => "{\u{2026}}",
        _ => "any",
    }
    .to_owned()
}

fn field(root: &Value, name: &str, schema: &Value, depth: usize) -> String {
    let schema = resolve(root, schema);
    if schema["type"] == "array" {
        let items = resolve(root, &schema["items"]);
        return if depth < SHAPE_DEPTH && !properties(&items).is_empty() {
            format!("{name}[{}]", render(root, &items, depth))
        } else {
            format!("{name}[]")
        };
    }
    if depth < SHAPE_DEPTH && !properties(&schema).is_empty() {
        return format!("{name}{}", render(root, &schema, depth));
    }
    name.to_owned()
}

/// A schema with its `$ref` followed, `Option`'s null alternative dropped, and
/// `type: ["string", "null"]` read as `"string"`. Several object alternatives
/// (a tagged enum) or an `allOf` merge into one object.
fn resolve(root: &Value, schema: &Value) -> Value {
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        let name = reference.rsplit('/').next().unwrap_or_default();
        let target = root
            .get("$defs")
            .or_else(|| root.get("definitions"))
            .and_then(|defs| defs.get(name));
        return target.map_or(Value::Bool(true), |target| resolve(root, target));
    }
    if let Some(Value::Array(types)) = schema.get("type") {
        let kept: Vec<&Value> = types.iter().filter(|kind| *kind != "null").collect();
        if let [only] = kept[..] {
            let mut schema = schema.clone();
            schema["type"] = only.clone();
            return schema;
        }
    }
    for key in ["anyOf", "oneOf", "allOf"] {
        let Some(Value::Array(options)) = schema.get(key) else {
            continue;
        };
        let options: Vec<Value> = options
            .iter()
            .map(|option| resolve(root, option))
            .filter(|option| option["type"] != "null")
            .collect();
        if let [only] = &options[..] {
            return only.clone();
        }
        let mut merged = Map::new();
        for option in &options {
            for (name, child) in properties(option) {
                merged.entry(name.clone()).or_insert_with(|| child.clone());
            }
        }
        if merged.is_empty() {
            return Value::Bool(true);
        }
        return serde_json::json!({"type": "object", "properties": merged});
    }
    schema.clone()
}

fn properties(schema: &Value) -> Vec<(&String, &Value)> {
    schema
        .get("properties")
        .and_then(Value::as_object)
        .map(|map| map.iter().collect())
        .unwrap_or_default()
}

/// Up to three names close to `word`, best first.
pub(crate) fn did_you_mean<'a>(
    word: &str,
    candidates: impl IntoIterator<Item = &'a str>,
) -> Vec<&'a str> {
    let mut scored: Vec<(f64, &str)> = candidates
        .into_iter()
        .map(|candidate| (strsim::jaro_winkler(word, candidate), candidate))
        .filter(|(score, _)| *score >= 0.8)
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    scored.dedup_by_key(|(_, candidate)| *candidate);
    scored
        .into_iter()
        .take(3)
        .map(|(_, candidate)| candidate)
        .collect()
}

#[cfg(test)]
mod tests {
    use schemars::JsonSchema;
    use serde::Serialize;

    use super::*;

    #[derive(Serialize, JsonSchema)]
    struct Reviewer {
        name: String,
        vote: i32,
    }

    #[derive(Serialize, JsonSchema)]
    struct Pr {
        id: u64,
        title: String,
        tags: Vec<String>,
        reviewers: Vec<Reviewer>,
        author: Option<Reviewer>,
        merged: Option<bool>,
    }

    #[derive(Serialize, JsonSchema)]
    struct Deep {
        a: Level1,
    }
    #[derive(Serialize, JsonSchema)]
    struct Level1 {
        b: Level2,
    }
    #[derive(Serialize, JsonSchema)]
    struct Level2 {
        c: Level3,
    }
    #[derive(Serialize, JsonSchema)]
    struct Level3 {
        d: String,
    }

    #[test]
    fn a_return_schema_renders_as_a_compact_depth_limited_shape() {
        let list = schemars::schema_for!(Vec<Pr>);
        assert_eq!(
            returns_shape(&list),
            "[{id,title,tags[],reviewers[{name,vote}],author{name,vote},merged}]"
        );
        assert!(returns_list_of_objects(&list));
        assert_eq!(
            return_fields(&list),
            [
                "id",
                "title",
                "tags",
                "reviewers",
                "name",
                "vote",
                "author",
                "name",
                "vote",
                "merged"
            ]
        );
        assert_eq!(returns_shape(&schemars::schema_for!(Deep)), "{a{b{c}}}");
        assert_eq!(returns_shape(&schemars::schema_for!(Vec<String>)), "[str]");
        assert_eq!(
            returns_shape(&schemars::schema_for!(serde_json::Value)),
            "any"
        );
        assert_eq!(
            returns_shape(&schemars::schema_for!(std::collections::BTreeMap<String, u32>)),
            "{\u{2026}}"
        );
        assert!(!returns_list_of_objects(&schemars::schema_for!(
            Vec<String>
        )));
        assert!(!returns_list_of_objects(&schemars::schema_for!(Pr)));
    }

    #[test]
    fn a_tagged_enum_merges_its_variants_fields() {
        #[derive(Serialize, JsonSchema)]
        #[serde(tag = "kind")]
        #[allow(dead_code)]
        enum Event {
            Pushed { branch: String },
            Voted { reviewer: String, vote: i32 },
        }
        assert_eq!(
            returns_shape(&schemars::schema_for!(Vec<Event>)),
            "[{branch,kind,reviewer,vote}]"
        );
    }

    #[test]
    fn a_long_listing_prints_names_only() {
        let few: Vec<(String, String)> = (0..3)
            .map(|i| (format!("n{i}"), "detail".to_owned()))
            .collect();
        let text = listing("T".into(), "commands", few, "F".into());
        assert_eq!(text, "T\n  n0  detail\n  n1  detail\n  n2  detail\nF");
        let many: Vec<(String, String)> = (0..41)
            .map(|i| (format!("n{i}"), "detail".to_owned()))
            .collect();
        let text = listing("T".into(), "commands", many, "F".into());
        assert!(!text.contains("detail"));
        assert!(text.contains("(41 commands, names only."));
        assert!(text.lines().all(|line| line.len() <= WRAP + 2), "{text}");
    }

    #[test]
    fn did_you_mean_finds_typos_and_prefixes_but_not_strangers() {
        let names = ["workitem", "pr", "pipeline", "run", "repo"];
        assert_eq!(did_you_mean("workitems", names), ["workitem"]);
        assert_eq!(did_you_mean("pipelin", names), ["pipeline"]);
        assert_eq!(did_you_mean("repos", names), ["repo"]);
        assert!(did_you_mean("ticket", names).is_empty());
    }

    #[test]
    fn the_empty_overview_is_small_and_names_the_basics() {
        let text = overview(&[], &Config::empty());
        assert!(text.len() < 1024, "{}", text.len());
        assert!(text.starts_with("agent-cli: tools for coding agents"));
        assert!(text.contains("Domains:     none yet"));
        assert!(text.contains("Config:      nothing to set up"));
    }
}
