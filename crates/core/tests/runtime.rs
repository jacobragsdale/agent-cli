//! The runtime end to end, in process, against the synthetic registry: what
//! an agent sees for each kind of call, and every safety rule.

mod common;

use std::io::Write;

use agent_cli_core::testing::{
    Answer, FakeTransport, assert_dry_run, assert_read_only_refuses, assert_search_quality, run,
};
use agent_cli_core::{Ctx, Domain, Exit, Failure, Setup, When, check_registry, command, run_with};
use anyhow::Result;
use common::{DOMAINS, LABELED};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::json;

fn fake(answers: Vec<Answer>) -> (Setup, FakeTransport) {
    let transport = FakeTransport::answering(answers);
    (Setup::fake(transport.clone()), transport)
}

fn go(argv: &[&str]) -> agent_cli_core::testing::Outcome {
    run(DOMAINS, argv, fake(Vec::new()).0)
}

#[test]
fn the_synthetic_registry_is_clean() {
    assert_eq!(check_registry(DOMAINS), Vec::<String>::new());
}

#[test]
fn the_overview_is_small_and_counts_domains_not_resources() {
    let outcome = go(&[]);
    assert_eq!(outcome.code, 0);
    assert!(outcome.stdout.len() < 1024, "{}", outcome.stdout.len());
    let lines: Vec<&str> = outcome.stdout.lines().collect();
    assert_eq!(
        lines[0],
        "agent-cli: Tracker, Vault, SQL, Kubernetes for coding agents \u{2014} 32 commands. Output: JSON."
    );
    assert_eq!(
        lines[1],
        "Start here:  agent-cli search <what you want to do>    e.g. agent-cli search \"list tickets matching\""
    );
    assert_eq!(
        lines[5],
        "Config:      tracker not set up \u{b7} db 1 connection    Live check: agent-cli doctor"
    );
    assert!(lines[6].starts_with("Now:         20") && lines[6].ends_with('Z'));
    assert_eq!(
        lines[7],
        "Domains:     tracker(19)  vault(3)  db(4)  cluster(6)"
    );
    for argv in [&["--help"][..], &["-h"], &["help"]] {
        assert_eq!(go(argv).stdout, outcome.stdout);
    }
}

#[test]
fn a_domain_lists_resources_and_a_resource_lists_verbs() {
    let domain = go(&["tracker"]);
    assert_eq!(
        domain.stdout,
        "agent-cli tracker: Tracker \u{2014} 19 commands\n\
         \x20 ticket    list get create update comment delete\n\
         \x20 pr        list get create vote complete abandon\n\
         \x20 build     list get logs run cancel\n\
         \x20 approval  list approve\n\
         Details: agent-cli tracker <resource> [<verb> --help]   Faster: agent-cli search <words>\n"
    );
    let resource = go(&["tracker", "pr", "--help"]);
    assert_eq!(resource.code, 0);
    assert!(resource.stdout.starts_with(
        "agent-cli tracker pr: 6 commands\n  list      List pull requests across repositories\n"
    ));
    assert!(resource.stdout.contains(
        "  complete  Complete a pull request, merging it into its target (destructive)\n"
    ));
}

#[test]
fn help_is_rendered_from_the_args_and_the_return_type() {
    let help = go(&["tracker", "ticket", "list", "--help"]);
    assert_eq!(
        help.stdout,
        "agent-cli tracker ticket list \u{2014} List tickets matching filters\n\
         \x20 --assignee str  name, email or @me\n\
         \x20 --state str[]   e.g. Active, \"In Progress\"\n\
         \x20 --text str      words in the title\n\
         \x20 --limit int     (default 50)\n\
         Returns: [{id,title,state,assignee,tags[]}]\n\
         Read. Globals: --fields --raw --timeout --output\n\
         e.g. agent-cli tracker ticket list --assignee @me --state Active --fields id,title,state\n"
    );
    let vote = go(&["help", "tracker", "pr", "vote"]);
    assert!(
        vote.stdout.contains(
            "\n *<id> int\n *<vote> approve|reject|wait|reset  approve, reject, wait, reset\n"
        ),
        "{}",
        vote.stdout
    );
    assert!(
        vote.stdout
            .contains("Write: --dry-run shows the change without making it. * required.")
    );
    for domain in DOMAINS {
        for command in domain.commands {
            let path = command.path;
            let text = go(&[path[0], path[1], path[2], "-h"]).stdout;
            assert!(text.len() < 2048 && text.contains("Returns: "), "{text}");
        }
    }
}

#[test]
fn unknown_words_get_did_you_mean_and_the_closest_commands() {
    let outcome = go(&["tracker", "tikcet", "list"]);
    assert_eq!(outcome.code, 2);
    assert_eq!(outcome.stdout, "");
    let mut lines = outcome.stderr.lines();
    assert_eq!(
        lines.next(),
        Some("error: unknown resource \"tikcet\" in tracker \u{2014} did you mean ticket?")
    );
    assert_eq!(lines.next(), Some("hint: closest commands:"));
    assert!(
        outcome.stderr.contains("\n  agent-cli tracker "),
        "{}",
        outcome.stderr
    );

    let domain = go(&["trackr", "ticket", "list"]);
    assert!(
        domain
            .stderr
            .starts_with("error: unknown domain \"trackr\" \u{2014} did you mean tracker?")
    );
    assert!(
        domain.stderr.contains("agent-cli tracker ticket list"),
        "{}",
        domain.stderr
    );

    let verb = go(&["tracker", "ticket", "lst"]);
    assert!(
        verb.stderr.starts_with(
            "error: unknown verb \"lst\" in tracker ticket \u{2014} did you mean list?"
        )
    );

    let id_first = go(&["tracker", "build", "991", "--raw"]);
    let lines: Vec<&str> = id_first.stderr.lines().collect();
    assert_eq!(
        lines[..3],
        [
            "error: unknown verb \"991\" in tracker build; an id goes after the verb",
            "hint: closest commands:",
            "  agent-cli tracker build get 991  # Show one build's status, result and timing",
        ],
        "{}",
        id_first.stderr
    );
    let effects: Vec<bool> = lines[3..]
        .iter()
        .map(|line| line.ends_with("(destructive)") || line.ends_with("(write)"))
        .collect();
    assert!(
        effects.is_sorted(),
        "reads before changes: {}",
        id_first.stderr
    );

    let synonym = go(&["tracker", "bug", "get"]);
    assert!(
        synonym.stderr.contains("agent-cli tracker ticket get <id>"),
        "{}",
        synonym.stderr
    );

    let flag_first = go(&["--bogus", "tracker"]);
    assert_eq!(flag_first.code, 2);
    assert!(
        flag_first
            .stderr
            .starts_with("error: unknown flag --bogus before the command")
    );
}

#[test]
fn a_bad_leaf_call_is_a_usage_error_that_shows_the_example() {
    let missing = go(&["tracker", "ticket", "get"]);
    assert_eq!(missing.code, 2);
    assert_eq!(
        missing.stderr,
        "error: the following required arguments were not provided: <ID>\n\
         hint: e.g. agent-cli tracker ticket get 42\n\
         \x20 all args: agent-cli tracker ticket get --help\n"
    );
    let bad = go(&["tracker", "ticket", "list", "--limit", "lots"]);
    assert!(
        bad.stderr
            .starts_with("error: invalid value 'lots' for '--limit <LIMIT>'"),
        "{}",
        bad.stderr
    );
    let typo = go(&["tracker", "ticket", "list", "--stat", "x"]);
    assert_eq!(
        typo.stderr.lines().next(),
        Some(
            "error: unknown flag --stat \u{2014} did you mean --state? \
             tracker ticket list takes --assignee --state --text --limit"
        ),
    );
    let stranger = go(&["tracker", "ticket", "list", "--log-id=7"]);
    assert_eq!(
        stranger.stderr.lines().next(),
        Some(
            "error: unknown flag --log-id; tracker ticket list takes --assignee --state --text --limit"
        ),
    );
    let extra = go(&["tracker", "ticket", "get", "42", "43"]);
    assert_eq!(
        extra.stderr.lines().next(),
        Some("error: unexpected argument '43' found")
    );
    assert!(!format!("{typo:?}{stranger:?}{extra:?}").contains("tip:"));
    let enum_value = go(&["tracker", "pr", "vote", "7", "yes"]);
    assert!(
        enum_value
            .stderr
            .contains("possible values: approve, reject, wait, reset"),
        "{}",
        enum_value.stderr
    );
}

#[test]
fn globals_work_anywhere_and_fields_project_the_output() {
    let outcome = go(&[
        "--fields", "id", "tracker", "ticket", "list", "--limit", "2",
    ]);
    assert_eq!(
        (outcome.code, outcome.stdout.as_str()),
        (0, "[{\"id\":1},{\"id\":2}]\n")
    );
    let nested = go(&[
        "tracker",
        "pr",
        "list",
        "--limit",
        "1",
        "--fields=id,reviewers.name",
    ]);
    assert_eq!(
        nested.stdout,
        "[{\"id\":1,\"reviewers\":[{\"name\":\"kim\"}]}]\n"
    );
    let big = go(&["tracker", "ticket", "list", "--limit", "500"]);
    assert!(big.stdout.len() <= 12_001, "the guard cut it");
    assert!(
        big.stderr.starts_with("[truncated the list: showing "),
        "{}",
        big.stderr
    );
}

#[test]
fn search_ranks_by_intent_and_says_when_the_best_hit_misses_most_words() {
    let outcome = go(&["search", "merge", "a", "pull", "request"]);
    assert_eq!(
        outcome.stdout.lines().next(),
        Some(
            "agent-cli tracker pr complete <id>  # Complete a pull request, merging it into its target (destructive)"
        )
    );
    assert_eq!(
        outcome.stdout.lines().count(),
        7,
        "six hits and a footer: {}",
        outcome.stdout
    );
    let limited = go(&["search", "--limit", "2", "ticket"]);
    assert_eq!(limited.stdout.lines().count(), 3);
    assert!(
        limited.stdout.ends_with(
            "--limit N for more. Details: agent-cli <domain> <resource> <verb> --help)\n"
        )
    );
    let guess = "(no command matches most of these words; closest:)\n";
    let partial = go(&["search", "pod", "zebra", "yak"]);
    assert!(partial.stdout.starts_with(guess), "{}", partial.stdout);
    let half = go(&["search", "pod", "zebra"]);
    assert!(!half.stdout.starts_with(guess), "{}", half.stdout);
    let none = go(&["search", "zzqx"]);
    assert_eq!(
        none.stdout,
        "no matches for \"zzqx\"; try other words, or browse: agent-cli\n"
    );
    assert_eq!(go(&["search"]).code, 2);
}

#[test]
fn search_meets_the_gates_on_labeled_queries() {
    assert_search_quality(DOMAINS, LABELED);
}

#[test]
fn a_dry_run_runs_the_reads_and_plans_the_first_write_redacted() {
    let project = Answer::json(&json!({"id": "p-1"}));
    let would = assert_dry_run(
        DOMAINS,
        &["tracker", "ticket", "create", "--title", "Login fails"],
        vec![project],
    );
    assert_eq!(
        would,
        [json!({
            "method": "POST",
            "url": "https://tracker.example/tickets",
            "headers": {"Authorization": "***"},
            "body": {"title": "Login fails", "type": "Task", "project": "p-1"},
        })]
    );
    for (argv, answers) in [
        (
            &["tracker", "ticket", "update", "4", "--state", "Done"][..],
            vec![],
        ),
        (&["tracker", "pr", "complete", "7"], vec![]),
        (&["cluster", "pod", "delete", "api-1"], vec![]),
        (
            &["db", "query", "run", "--conn", "local", "drop table t"],
            vec![],
        ),
    ] {
        assert_eq!(assert_dry_run(DOMAINS, argv, answers).len(), 1, "{argv:?}");
    }
    let process = assert_dry_run(DOMAINS, &["cluster", "pod", "delete", "api-1"], vec![]);
    assert_eq!(
        process,
        [json!({"run": ["echo", "pod", "api-1", "deleted"]})]
    );
}

#[test]
fn a_destructive_command_needs_yes_and_says_how_to_run_it_again() {
    let (setup, transport) = fake(vec![Answer::ok("{}")]);
    let refused = run(DOMAINS, &["tracker", "pr", "complete", "7"], setup);
    assert_eq!(refused.code, 2);
    assert_eq!(
        refused.stderr,
        "error: `tracker pr complete` is destructive; confirm it with --yes\n\
         hint: agent-cli tracker pr complete 7 --yes   (or --dry-run to see it first)\n"
    );
    assert!(transport.sent().is_empty());

    let (setup, transport) = fake(vec![Answer::ok("{}")]);
    let done = run(DOMAINS, &["tracker", "pr", "complete", "7", "--yes"], setup);
    assert_eq!((done.code, done.stdout.as_str()), (0, "{\"id\":7}\n"));
    assert_eq!(transport.sent()[0].method, agent_cli_core::Method::Patch);

    let deleted = go(&["cluster", "pod", "delete", "api-1", "--yes"]);
    assert_eq!(
        deleted.stdout,
        "{\"deleted\":\"api-1\",\"said\":\"pod api-1 deleted\"}\n"
    );
}

#[test]
fn a_command_whose_effect_varies_is_checked_at_the_write() {
    let read = go(&["db", "query", "run", "--conn", "local", "select 1"]);
    assert_eq!(read.code, 0, "{read:?}");
    let refused = go(&["db", "query", "run", "--conn", "local", "delete from t"]);
    assert_eq!(refused.code, 2);
    assert!(
        refused
            .stderr
            .contains("hint: agent-cli db query run --conn local 'delete from t' --yes"),
        "{}",
        refused.stderr
    );
    let done = go(&[
        "db",
        "query",
        "run",
        "--conn",
        "local",
        "delete from t",
        "--yes",
    ]);
    assert_eq!(done.json()["rows_affected"], 3);

    let read_only = |argv: &[&str]| run(DOMAINS, argv, fake(Vec::new()).0.read_only());
    assert_eq!(
        read_only(&["db", "query", "run", "--conn", "local", "select 1"]).code,
        0
    );
    let refused = read_only(&[
        "db",
        "query",
        "run",
        "--conn",
        "local",
        "delete from t",
        "--yes",
    ]);
    assert_eq!(refused.code, 2);
    assert!(
        refused
            .stderr
            .starts_with("error: AGENT_CLI_READ_ONLY is set, so this change was refused")
    );
}

#[test]
fn read_only_refuses_every_non_read_command_before_it_runs() {
    assert_read_only_refuses(DOMAINS);
}

#[test]
fn a_secret_needs_reveal_or_output_and_output_keeps_it_off_stdout() {
    let answer = || vec![Answer::json(&json!({"value": "hunter2-real-value"}))];
    let refused = run(DOMAINS, &["vault", "secret", "get", "db"], fake(answer()).0);
    assert_eq!(refused.code, 2);
    assert!(refused.stderr.contains("--reveal") && refused.stderr.contains("--output FILE"));

    let shown = run(
        DOMAINS,
        &["vault", "secret", "get", "db", "--reveal"],
        fake(answer()).0,
    );
    assert_eq!(shown.json()["value"], "hunter2-real-value");

    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("db.txt");
    let file_arg = file.display().to_string();
    let saved = run(
        DOMAINS,
        &[
            "vault", "secret", "get", "db", "--output", &file_arg, "--fields", "value",
        ],
        fake(answer()).0,
    );
    assert_eq!(saved.json(), json!({"saved": file_arg, "bytes": 18}));
    assert!(!saved.stdout.contains("hunter2"));
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "hunter2-real-value"
    );
}

#[test]
fn logs_keep_their_tail_when_the_guard_cuts_them() {
    let outcome = go(&["cluster", "pod", "logs", "api-1"]);
    let printed = outcome.json();
    assert!(
        printed["text"]
            .as_str()
            .unwrap()
            .ends_with("api-1 request 1999 served in 3ms\n")
    );
    assert_eq!(printed["truncated"], true);
    assert!(
        outcome.stderr.contains("showing the last"),
        "{}",
        outcome.stderr
    );
    let saved = outcome
        .stderr
        .split("Full text: ")
        .nth(1)
        .unwrap()
        .split(". --raw")
        .next()
        .unwrap();
    std::fs::remove_file(saved).unwrap();
}

#[test]
fn http_failures_map_to_exit_codes_and_their_text_is_redacted() {
    let body = r#"{"message":"TF401: token Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.c2lnbmF0dXJl rejected"}"#;
    let (setup, _) = fake(vec![Answer::status(404, body)]);
    let outcome = run(DOMAINS, &["tracker", "ticket", "get", "9"], setup);
    assert_eq!(outcome.code, 4);
    assert_eq!(
        outcome.stderr,
        "error: GET https://tracker.example/tickets/9 answered 404: TF401: token Bearer *** rejected\n"
    );

    let (setup, _) = fake(vec![
        Answer::status(429, "{}").with_header("Retry-After", "30"),
    ]);
    let started = std::time::Instant::now();
    let throttled = run(
        DOMAINS,
        &["tracker", "ticket", "get", "9", "--timeout", "2"],
        setup,
    );
    assert_eq!(throttled.code, 124);
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
}

#[test]
fn doctor_reports_every_check_as_json_and_a_broken_section_stays_local() {
    let outcome = go(&["doctor"]);
    assert_eq!(outcome.code, 1, "the db check fails");
    let rows = outcome.json();
    assert_eq!(rows[0]["domain"], "core");
    assert_eq!(
        rows[1],
        json!({"domain": "tracker", "check": "org", "ok": false, "detail": "no org configured", "hint": "set [tracker] org in the config file"})
    );
    assert_eq!(rows.as_array().unwrap().len(), 3);

    let configured = fake(Vec::new())
        .0
        .with_config("[tracker]\norg = \"contoso\"\n");
    let tracker = run(DOMAINS, &["doctor", "tracker"], configured);
    assert_eq!(tracker.code, 0);
    assert_eq!(tracker.json()[1]["detail"], "contoso");

    let broken = || fake(Vec::new()).0.with_config("[tracker]\norg = 5\n");
    let tracker = run(DOMAINS, &["doctor", "tracker"], broken());
    assert_eq!(tracker.code, 1);
    assert!(
        tracker.json()[1]["detail"]
            .as_str()
            .unwrap()
            .starts_with("[tracker] in config.toml: invalid type")
    );
    assert_eq!(
        run(DOMAINS, &["db", "connection", "list"], broken()).code,
        0
    );
    assert_eq!(go(&["doctor", "nope"]).code, 2);
}

#[test]
fn a_closed_pipe_is_success() {
    struct Closed;
    impl Write for Closed {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let argv = vec!["tracker".to_owned(), "ticket".to_owned(), "list".to_owned()];
    let mut err = Vec::new();
    let code = run_with(
        DOMAINS,
        &argv,
        fake(Vec::new()).0,
        &mut Closed,
        &mut err,
        false,
    );
    assert_eq!((code, err.len()), (0, 0));
}

// ---------- the checker finds each kind of violation ----------

#[derive(clap::Args)]
struct Shadowing {
    #[arg(long)]
    output: Option<String>,
    #[arg(short = 'h', long)]
    host: Option<String>,
    #[arg(long)]
    limit: Option<String>,
}

#[derive(clap::Args)]
struct Plain {
    #[arg(long)]
    limit: Option<u32>,
}

#[derive(Serialize, JsonSchema)]
struct Row {
    id: u64,
}

fn shadowing(_: &Ctx, _: Shadowing) -> Result<Vec<Row>> {
    Ok(Vec::new())
}

fn plain(_: &Ctx, _: Plain) -> Result<Vec<Row>> {
    Ok(Vec::new())
}

command! {
    SHADOW = ["search", "Thing", "list"], Read,
    "List things.",
    keywords: [],
    example: "search Thing list --fields id",
    run: shadowing,
}

command! {
    BAD_VERB = ["search", "thing", "frobnicate"], Read,
    "A summary that goes on and on well past the eighty characters every summary is allowed",
    keywords: [],
    example: "other thing frobnicate",
    run: plain,
}

command! {
    NO_FIELDS = ["search", "thing", "get"], Read,
    "Get things",
    keywords: [],
    example: "search thing get --limit many",
    run: plain,
}

command! {
    NO_FIELDS_OK_PARSE = ["search", "thing", "run"], Read,
    "Run things",
    keywords: [],
    example: "search thing run --limit 5",
    run: plain,
}

#[test]
fn the_checker_names_each_violation() {
    const BROKEN: &[Domain] = &[Domain {
        name: "search",
        summary: "Broken",
        commands: &[
            SHADOW,
            BAD_VERB,
            NO_FIELDS,
            NO_FIELDS_OK_PARSE,
            NO_FIELDS_OK_PARSE,
        ],
        synonyms: &[],
        status: |_| String::new(),
        doctor: |_| Vec::new(),
    }];
    let problems = check_registry(BROKEN);
    let expected = [
        "domain search shadows the built-in `agent-cli search`",
        "search Thing list: \"Thing\" is not a lowercase kebab word",
        "search Thing list: summary ends with a period",
        "search Thing list: --output shadows the core global",
        "search Thing list: -h shadows --help",
        "search thing frobnicate: verb \"frobnicate\" is not in VERBS",
        "search thing frobnicate: summary is 86 chars (1 to 80)",
        "search thing frobnicate: example must start with `search thing frobnicate`",
        "search thing frobnicate: --limit takes int here but str in search Thing list",
        "search thing get: example does not parse: invalid value 'many' for '--limit <LIMIT>'",
        "search thing run: returns a list of objects, so its example should use --fields",
        "search thing run: registered twice",
    ];
    for want in expected {
        assert!(
            problems.iter().any(|problem| problem.starts_with(want)),
            "missing {want:?} in {problems:#?}"
        );
    }
}

// ---------- the conventions every domain keeps ----------

#[derive(clap::Args)]
struct Loose {
    /// When it began
    #[arg(long)]
    from: Option<String>,
    #[arg(long)]
    ns: Option<String>,
    /// Changed after this
    #[arg(long)]
    changed: Option<When>,
    /// A day, as text
    #[arg(long)]
    since: Option<String>,
    /// Who it is for
    #[arg(long)]
    assignee: Option<String>,
}

#[derive(clap::Args)]
struct Endless {
    #[arg(long)]
    follow: bool,
}

fn loose(_: &Ctx, _: Loose) -> Result<Vec<Row>> {
    Ok(Vec::new())
}

fn endless(_: &Ctx, _: Endless) -> Result<Row> {
    Ok(Row { id: 1 })
}

command! {
    LOOSE = ["conv", "item", "list"], Read,
    "List items",
    keywords: [],
    example: "conv item list --fields id",
    run: loose,
}

command! {
    ENDLESS = ["conv", "item", "logs"], Read,
    "Follow an item's log",
    keywords: [],
    example: "conv item logs",
    run: endless,
}

#[test]
fn the_checker_holds_every_domain_to_the_shared_flag_conventions() {
    const CONV: &[Domain] = &[
        Domain {
            name: "conv",
            summary: "Conventions",
            commands: &[LOOSE, ENDLESS],
            synonyms: &[("ticket", &["item"])],
            status: |_| String::new(),
            doctor: |_| Vec::new(),
        },
        Domain {
            name: "other",
            summary: "Other",
            commands: &[NO_FIELDS_OK_PARSE_OTHER],
            synonyms: &[],
            status: |_| String::new(),
            doctor: |_| Vec::new(),
        },
    ];
    let problems = check_registry(CONV);
    let expected = [
        "conv item list: --from is a synonym: name it --since",
        "conv item list: --ns is a synonym: name it --namespace",
        "conv item list: --changed takes a time, which only --since and --until do",
        "conv item list: --since must take a time (core's When)",
        "conv item list: --assignee names a person, so its help must say @me works",
        "conv item list: a list takes --limit (int, default 50)",
        "conv item logs: logs take --tail",
        "conv item logs: logs never --follow: a command must end",
        "conv's synonym \"ticket\" is a other resource",
    ];
    for want in expected {
        assert!(
            problems.iter().any(|problem| problem.starts_with(want)),
            "missing {want:?} in {problems:#?}"
        );
    }
    assert_eq!(problems.len(), expected.len(), "{problems:#?}");
}

command! {
    NO_FIELDS_OK_PARSE_OTHER = ["other", "ticket", "run"], Read,
    "Run tickets",
    keywords: [],
    example: "other ticket run --limit 5 --fields id",
    run: plain,
}

// ---------- an answer alongside a failing exit, and a command's own deadline ----------

#[derive(clap::Args)]
struct JobId {
    id: u64,
}

#[derive(Serialize, JsonSchema)]
struct Job {
    id: u64,
    state: String,
    /// Whole seconds of deadline the command was given.
    budget: u64,
}

fn job_wait(ctx: &Ctx, args: JobId) -> Result<Job> {
    let job = Job {
        id: args.id,
        state: "failed".into(),
        budget: ctx.remaining()?.as_secs_f64().round() as u64,
    };
    Err(
        Failure::new(Exit::Failed, format!("job {} failed", args.id))
            .hint(format!("agent-cli jobs job get {}", args.id))
            .with_data(job)
            .into(),
    )
}

fn job_get(_: &Ctx, args: JobId) -> Result<Job> {
    Ok(Job {
        id: args.id,
        state: "failed".into(),
        budget: 0,
    })
}

command! {
    JOB_WAIT = ["jobs", "job", "wait"], Read,
    "Wait for a job to finish",
    keywords: [],
    example: "jobs job wait 7",
    timeout: 100,
    run: job_wait,
}

command! {
    JOB_GET = ["jobs", "job", "get"], Read,
    "Show a job",
    keywords: [],
    example: "jobs job get 7",
    run: job_get,
}

const JOBS: &[Domain] = &[Domain {
    name: "jobs",
    summary: "Jobs",
    commands: &[JOB_WAIT, JOB_GET],
    synonyms: &[],
    status: |_| String::new(),
    doctor: |_| Vec::new(),
}];

#[test]
fn a_failure_can_carry_its_answer_and_a_command_its_own_default_timeout() {
    assert_eq!(check_registry(JOBS), Vec::<String>::new());
    let outcome = run(JOBS, &["jobs", "job", "wait", "7"], fake(Vec::new()).0);
    assert_eq!(outcome.code, 1, "{outcome:?}");
    assert_eq!(
        outcome.json(),
        json!({"id": 7, "state": "failed", "budget": 100})
    );
    assert_eq!(
        outcome.stderr,
        "error: job 7 failed\nhint: agent-cli jobs job get 7\n"
    );

    let projected = run(
        JOBS,
        &[
            "jobs",
            "job",
            "wait",
            "7",
            "--timeout",
            "5",
            "--fields",
            "budget",
        ],
        fake(Vec::new()).0,
    );
    assert_eq!(projected.code, 1);
    assert_eq!(
        projected.json(),
        json!({"budget": 5}),
        "--timeout wins, --fields applies"
    );

    let help = run(JOBS, &["jobs", "job", "wait", "--help"], fake(Vec::new()).0);
    assert!(
        help.stdout
            .contains("Globals: --fields --raw --timeout (default 100s) --output"),
        "{}",
        help.stdout
    );
}

#[test]
fn printed_command_lines_must_parse_and_printed_times_must_be_utc() {
    use agent_cli_core::testing::{non_utc_times, printed_command_problems};
    let text = "hint: agent-cli tracker ticket get ID --fields id, or agent-cli tracker ticket get --bogus\n\
                hint: agent-cli tracker tikcet list; agent-cli <domain> <resource>; agent-cli doctor tracker\n\
                hint: agent-cli elsewhere thing get 1";
    assert_eq!(
        printed_command_problems(DOMAINS, text),
        [
            "`agent-cli tracker ticket get --bogus`: unknown flag --bogus; tracker ticket get takes no flags",
            "`agent-cli tracker tikcet list`: tracker has no resource \"tikcet\"",
        ]
    );
    let printed = json!({"at": "2026-09-29T14:00:00Z", "rows": [{"t": "2026-09-29T16:00:00+02:00"}],
        "text": "2026-09-29T16:00:00+02:00 is inside prose", "naive": "2026-09-29T14:00:00"});
    assert_eq!(non_utc_times(&printed), ["2026-09-29T16:00:00+02:00"]);
}
