//! 1,000 commands: discovery stays fast and small, and the registry stays
//! clean at that size. Its own test binary, so no other test competes for
//! the CPU while it is timed.

mod common;

use std::time::{Duration, Instant};

use agent_cli_core::testing::{FakeTransport, Outcome, run};
use agent_cli_core::{Command, Domain, Setup, VERBS, check_registry};

/// Best of five on the development machine, then about 3x. Sub-millisecond
/// numbers get a 1 ms floor so scheduler noise cannot fail a run; the floor
/// still catches the regression that matters, touching every command.
///
/// Measured debug: overview 10 µs, search 65 ms, help 48 µs.
/// Measured release: overview 2 µs, search 7.7 ms, help 6.5 µs.
const GATES: Gates = if cfg!(debug_assertions) {
    Gates {
        overview: Duration::from_millis(1),
        search: Duration::from_millis(200),
        help: Duration::from_millis(1),
    }
} else {
    Gates {
        overview: Duration::from_millis(1),
        search: Duration::from_millis(25),
        help: Duration::from_millis(1),
    }
};

struct Gates {
    overview: Duration,
    search: Duration,
    help: Duration,
}

const DOMAINS: [(&str, &str); 12] = [
    ("alpha", "Alpha"),
    ("bravo", "Bravo"),
    ("charlie", "Charlie"),
    ("delta", "Delta"),
    ("echo", "Echo"),
    ("foxtrot", "Foxtrot"),
    ("golf", "Golf"),
    ("hotel", "Hotel"),
    ("india", "India"),
    ("juliet", "Juliet"),
    ("kilo", "Kilo"),
    ("lima", "Lima"),
];

const NOUNS: [&str; 64] = [
    "account",
    "alert",
    "artifact",
    "backup",
    "bucket",
    "certificate",
    "channel",
    "commit",
    "config",
    "credential",
    "dashboard",
    "database",
    "deployment",
    "disk",
    "endpoint",
    "environment",
    "event",
    "feature",
    "feed",
    "firewall",
    "function",
    "gateway",
    "group",
    "identity",
    "image",
    "incident",
    "index",
    "instance",
    "invoice",
    "job",
    "label",
    "license",
    "listener",
    "member",
    "metric",
    "migration",
    "milestone",
    "monitor",
    "namespace",
    "network",
    "node",
    "notebook",
    "package",
    "partition",
    "permission",
    "policy",
    "pool",
    "queue",
    "quota",
    "record",
    "region",
    "release",
    "replica",
    "report",
    "role",
    "route",
    "rule",
    "schedule",
    "snapshot",
    "sprint",
    "subnet",
    "template",
    "tenant",
    "volume",
];

const QUALITIES: [&str; 8] = [
    "status and owner",
    "size and region",
    "tags and labels",
    "age and version",
    "health and errors",
    "limits and usage",
    "state and history",
    "members and roles",
];

fn leak(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

fn summary(verb: &str, noun: &str, quality: &str) -> String {
    let text = match verb {
        "list" => format!("List {noun}s in a scope with their {quality}"),
        "get" => format!("Show one {noun} with its {quality}"),
        "create" => format!("Create a {noun} from a name and its {quality}"),
        "update" => format!("Change a {noun}'s {quality}"),
        "delete" => format!("Delete a {noun} and everything under it"),
        "logs" => format!("Show recent log lines for a {noun}"),
        "wait" => format!("Wait for a {noun} to finish, bounded by --timeout"),
        other => format!(
            "{}{} a {noun} by name",
            other[..1].to_uppercase(),
            &other[1..]
        ),
    };
    text.chars().take(80).collect()
}

/// 1,000 commands over 12 domains. The first domain has 45 resources, so its
/// listing is names-only; the rest have 11 resources of up to 8 verbs. Each
/// command reuses a synthetic template's args and return type, which is how
/// real domains share arg structs.
fn registry() -> Vec<Domain> {
    let templates: Vec<Command> = common::templates();
    let mut made = 0;
    let mut domains = Vec::new();
    for (index, (name, title)) in DOMAINS.iter().enumerate() {
        let mut commands = Vec::new();
        let (resources, verbs) = if index == 0 { (45, 1) } else { (11, 8) };
        for r in 0..resources {
            let noun = NOUNS[(r + index * 5) % NOUNS.len()];
            let resource = if index == 0 {
                format!("{noun}{r}")
            } else {
                noun.to_owned()
            };
            for v in 0..verbs {
                if made == 1000 {
                    break;
                }
                let verb = VERBS[(v + r) % VERBS.len()];
                // A template of the same verb where there is one, so lists
                // keep --limit and logs keep --tail, as check_registry wants.
                let same: Vec<&Command> = templates.iter().filter(|t| t.path[2] == verb).collect();
                let others: Vec<&Command> = templates
                    .iter()
                    .filter(|t| !matches!(t.path[2], "list" | "logs"))
                    .collect();
                let template = if same.is_empty() {
                    *others[made % others.len()]
                } else {
                    *same[made % same.len()]
                };
                let rest = template.example.splitn(4, ' ').nth(3).unwrap_or_default();
                let quality = QUALITIES[(made * 7) % QUALITIES.len()];
                commands.push(Command {
                    path: [name, leak(resource.clone()), verb],
                    summary: leak(summary(verb, noun, quality)),
                    keywords: Box::leak(
                        vec![noun, quality.split(' ').next().unwrap_or_default(), *title]
                            .into_boxed_slice(),
                    ),
                    example: leak(
                        format!("{name} {resource} {verb} {rest}")
                            .trim_end()
                            .to_owned(),
                    ),
                    ..template
                });
                made += 1;
            }
        }
        domains.push(Domain {
            name,
            summary: title,
            commands: Box::leak(commands.into_boxed_slice()),
            synonyms: &[("vm", &["instance"]), ("cert", &["certificate"])],
            status: |_| String::new(),
            doctor: |_| Vec::new(),
        });
    }
    assert_eq!(made, 1000);
    domains
}

/// The best of five runs, and the last run's output.
fn best(domains: &[Domain], argv: &[&str]) -> (Duration, Outcome) {
    let mut fastest = Duration::MAX;
    let mut last = None;
    for _ in 0..5 {
        let started = Instant::now();
        let outcome = run(domains, argv, Setup::fake(FakeTransport::default()));
        fastest = fastest.min(started.elapsed());
        last = Some(outcome);
    }
    (fastest, last.unwrap_or_else(|| unreachable!()))
}

#[test]
fn discovery_is_fast_and_small_at_1000_commands() {
    let domains = registry();

    let started = Instant::now();
    let problems = check_registry(&domains);
    let checked = started.elapsed();
    assert_eq!(problems, Vec::<String>::new());

    let (overview, outcome) = best(&domains, &[]);
    assert_eq!(outcome.code, 0);
    assert!(
        outcome.stdout.len() < 1024,
        "overview is {} bytes",
        outcome.stdout.len()
    );

    let (search, outcome) = best(
        &domains,
        &["search", "restart", "a", "deployment", "replica"],
    );
    assert_eq!(outcome.code, 0);
    assert!(outcome.stdout.lines().count() >= 6, "{}", outcome.stdout);

    let [domain, resource, verb] = domains[1].commands[0].path;
    let (help, outcome) = best(&domains, &[domain, resource, verb, "--help"]);
    assert_eq!(outcome.code, 0, "{outcome:?}");

    let listing = run(&domains, &["alpha"], Setup::fake(FakeTransport::default()));
    assert!(
        listing.stdout.contains("(45 resources, names only."),
        "{}",
        listing.stdout
    );

    eprintln!(
        "1,000 commands ({}): overview {overview:.2?}, search {search:.2?}, help {help:.2?}, check_registry {checked:.2?}",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );
    assert!(overview <= GATES.overview, "overview took {overview:?}");
    assert!(search <= GATES.search, "search took {search:?}");
    assert!(help <= GATES.help, "help took {help:?}");
}
