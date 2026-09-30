//! The k8s domain: a thin, bounded, JSON-emitting `kubectl` wrapper, not a
//! kubectl replacement. Ported from az-tui's `kube.rs` (the types, the
//! `Kubectl` source, owner resolution and `kubectl_error`; not the watcher).
//!
//! Every call is `kubectl --context C --request-timeout=10s …`, run through
//! `ctx.read` or `ctx.write` and so bounded by the command's deadline, with
//! the whole process group killed at it: a kubelogin waiting on a device-code
//! prompt cannot hang the command. Where a call goes comes from
//! `[[k8s.scope]]`: `--cluster` picks a scope and `--namespace` one of its
//! namespaces, and each defaults only when there is exactly one to pick.

mod objects;
mod pod;

use std::process::Command;

use agent_cli_core::{Check, Config, Ctx, Domain, Effect, Exit, Failure, Output, command, pick};
use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

pub const K8S: Domain = Domain {
    name: "k8s",
    summary: "Kubernetes",
    commands: &[
        pod::POD_LIST,
        pod::POD_GET,
        pod::POD_LOGS,
        pod::POD_DELETE,
        objects::EVENT_LIST,
        objects::DEPLOYMENT_LIST,
        objects::DEPLOYMENT_RESTART,
        objects::DEPLOYMENT_SCALE,
        objects::CONFIGMAP_LIST,
        objects::CONFIGMAP_GET,
        objects::SECRET_LIST,
        objects::SECRET_GET,
        CONTEXT_LIST,
    ],
    synonyms: &[
        ("container", &["pod"]),
        ("containers", &["pod"]),
        ("kubernetes", &["k8s"]),
        ("kubectl", &["k8s"]),
        // Not "restarts": its stem is the restart verb, and a crash loop is
        // a question for pod list, not a reason to restart anything.
        ("crashloop", &["pod", "status", "ready"]),
        ("crashlooping", &["pod", "status", "ready"]),
        ("crash loop", &["pod", "status", "ready"]),
        ("crash looping", &["pod", "status", "ready"]),
        ("crashing", &["pod", "status", "ready"]),
        ("crash", &["pod", "previous"]),
        ("oomkilled", &["pod", "status"]),
        ("rollout", &["deployment", "restart"]),
        ("deployed", &["deployment", "images"]),
        ("running in", &["deployment", "images"]),
        ("redeploy", &["deployment", "restart"]),
        ("bounce", &["restart"]),
        ("replicas", &["scale"]),
        ("config map", &["configmap"]),
        ("env vars", &["configmap"]),
        ("k8s secret", &["k8s", "secret"]),
        ("kubernetes secret", &["k8s", "secret"]),
        ("log", &["logs"]),
        ("output", &["logs"]),
        ("warnings", &["event"]),
    ],
    status,
    doctor,
};

/// The bound on one request to the API server. The command's deadline bounds
/// the whole call, credential plugin included.
const REQUEST_TIMEOUT: &str = "--request-timeout=10s";

/// The `[k8s]` section.
///
/// ```toml
/// [[k8s.scope]]
/// name = "dev"                   # what --cluster takes
/// context = "aks-contoso-dev"    # the kubeconfig context; the name when left out
/// namespaces = ["web", "jobs"]   # or one string; left out: every namespace
/// ```
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct K8s {
    scope: Vec<Scope>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Scope {
    name: String,
    #[serde(default)]
    context: Option<String>,
    #[serde(default, deserialize_with = "one_or_many")]
    namespaces: Vec<String>,
}

impl Scope {
    fn context(&self) -> &str {
        self.context.as_deref().unwrap_or(&self.name)
    }
}

fn one_or_many<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }
    Ok(match OneOrMany::deserialize(deserializer)? {
        OneOrMany::One(one) => vec![one],
        OneOrMany::Many(many) => many,
    })
}

fn scopes(config: &Config) -> Result<Vec<Scope>> {
    let section: K8s = config.section("k8s")?;
    if let Some(blank) = section
        .scope
        .iter()
        .find(|scope| scope.name.trim().is_empty())
    {
        return Err(Failure::setup(format!(
            "a [[k8s.scope]] in {} has a blank name (context {:?})",
            config.path().display(),
            blank.context()
        ))
        .hint("give every [[k8s.scope]] a name")
        .into());
    }
    Ok(section.scope)
}

/// Where every k8s command reads or changes something.
#[derive(Clone, clap::Args)]
pub struct At {
    /// The [[k8s.scope]] name (or its kube context); defaults to the only one
    #[arg(long)]
    cluster: Option<String>,
    /// Defaults to the scope's only namespace
    #[arg(long)]
    namespace: Option<String>,
}

/// A scope and namespace picked, ready to run kubectl against.
struct Target {
    scope: String,
    context: String,
    /// `None` is every namespace, for a listing in a scope that names none.
    namespace: Option<String>,
}

impl At {
    /// For a listing: a scope that names no namespaces lists all of them.
    fn listing(&self, ctx: &Ctx) -> Result<Target> {
        self.resolve(ctx, false)
    }

    /// For one object, which lives in exactly one namespace.
    fn one(&self, ctx: &Ctx) -> Result<Target> {
        self.resolve(ctx, true)
    }

    /// One object as an agent was handed it: its id (`cluster/namespace/name`,
    /// as every row prints it), `namespace/name`, or a name, with `--cluster`
    /// and `--namespace` filling in what it leaves out. A ref and a flag that
    /// disagree is exit 2: the CLI never picks one.
    fn named(&self, ctx: &Ctx, raw: &str) -> Result<(Target, String)> {
        let parts: Vec<&str> = raw.trim().split('/').collect();
        let (cluster, namespace, name) = match parts.as_slice() {
            [name] => (None, None, *name),
            [namespace, name] => (None, Some(*namespace), *name),
            [cluster, namespace, name] => (Some(*cluster), Some(*namespace), *name),
            _ => (None, None, ""),
        };
        if name.is_empty() || parts.iter().any(|part| part.is_empty()) {
            return Err(Failure::usage(format!(
                "{raw:?} is not NAME, NAMESPACE/NAME or CLUSTER/NAMESPACE/NAME"
            ))
            .into());
        }
        let merge = |held: Option<&str>, flag: &Option<String>, what: &str| match (held, flag) {
            (Some(held), Some(flag)) if held != flag => Err(Failure::usage(format!(
                "{raw} is in {what} {held}, and --{what} says {flag}"
            ))),
            (held, flag) => Ok(held.map(str::to_owned).or_else(|| flag.clone())),
        };
        let at = Self {
            cluster: merge(cluster, &self.cluster, "cluster")?,
            namespace: merge(namespace, &self.namespace, "namespace")?,
        };
        Ok((at.one(ctx)?, name.to_owned()))
    }

    fn resolve(&self, ctx: &Ctx, single: bool) -> Result<Target> {
        let scopes = scopes(ctx.config())?;
        if scopes.is_empty() {
            return Err(Failure::setup(format!("no [[k8s.scope]] in {}", ctx.config().path().display()))
                .hint("agent-cli aks cluster connect NAME prints one to paste; config.example.toml shows the keys")
                .into());
        }
        // A kube context names its scope too: an AKS cluster's is its name,
        // which is what Datadog and `aks cluster list` call it.
        let wanted = self.cluster.as_deref().map(|wanted| {
            scopes
                .iter()
                .find(|scope| scope.name == wanted)
                .or_else(|| scopes.iter().find(|scope| scope.context() == wanted))
                .map_or(wanted, |scope| scope.name.as_str())
        });
        let scope = pick("scope", "--cluster", wanted, &scopes, |scope| &scope.name).map_err(
            |failure| match failure.exit {
                Exit::Usage if wanted.is_some() => failure.hint("agent-cli k8s context list"),
                _ => failure,
            },
        )?;
        let listed = &scope.namespaces;
        let namespace = match (&self.namespace, listed.as_slice()) {
            (Some(wanted), []) => Some(wanted.clone()),
            (Some(wanted), _) if listed.contains(wanted) => Some(wanted.clone()),
            (Some(wanted), _) => {
                return Err(Failure::usage(format!(
                    "namespace {wanted:?} is not in scope {}; --namespace takes one of: {}",
                    scope.name,
                    listed.join(", ")
                ))
                .hint(format!(
                    "add it to the namespaces of [[k8s.scope]] {:?}",
                    scope.name
                ))
                .into());
            }
            (None, [only]) => Some(only.clone()),
            (None, []) if !single => None,
            (None, []) => {
                return Err(Failure::usage(format!(
                    "scope {} names no namespaces; say which with --namespace",
                    scope.name
                ))
                .into());
            }
            (None, _) => {
                return Err(Failure::usage(format!(
                    "scope {} has more than one namespace; name one with --namespace: {}",
                    scope.name,
                    listed.join(", ")
                ))
                .into());
            }
        };
        Ok(Target {
            scope: scope.name.clone(),
            context: scope.context().to_owned(),
            namespace,
        })
    }
}

impl Target {
    /// `kubectl --context C --request-timeout=10s <args> -n NS` (or
    /// `--all-namespaces`).
    fn kubectl(&self, args: &[&str]) -> Command {
        let mut command = program("kubectl");
        command
            .args(["--context", &self.context, REQUEST_TIMEOUT])
            .args(args);
        match &self.namespace {
            Some(namespace) => command.args(["-n", namespace]),
            None => command.arg("--all-namespaces"),
        };
        command
    }

    fn read(&self, ctx: &Ctx, args: &[&str]) -> Result<String> {
        finished(ctx.read(self.kubectl(args))?)
    }

    fn json(&self, ctx: &Ctx, args: &[&str]) -> Result<Value> {
        let text = self.read(ctx, args)?;
        if text.trim().is_empty() {
            anyhow::bail!("kubectl answered with nothing, which is not an empty list");
        }
        serde_json::from_str(&text).context("kubectl answered with something other than JSON")
    }

    /// Every change k8s makes (delete, restart, scale) is destructive: it
    /// needs --yes.
    fn write(&self, ctx: &Ctx, args: &[&str]) -> Result<String> {
        finished(ctx.write(Effect::Destructive, self.kubectl(args))?)
    }

    /// The namespace a row carries: only when the listing spans several, where
    /// it is the one thing that tells two rows apart.
    fn row_namespace(&self, item: &Value) -> Option<String> {
        self.namespace
            .is_none()
            .then(|| item["metadata"]["namespace"].as_str().map(str::to_owned))
            .flatten()
    }

    /// An object's id, `cluster/namespace/name`: what every k8s command that
    /// takes one object accepts, with no other flag.
    fn id(&self, item: &Value) -> String {
        let namespace = item["metadata"]["namespace"]
            .as_str()
            .or(self.namespace.as_deref())
            .unwrap_or_default();
        let name = item["metadata"]["name"].as_str().unwrap_or_default();
        format!("{}/{namespace}/{name}", self.scope)
    }
}

/// A child process. Tests put the repo's `scripts/fake` first on its PATH:
/// kubectl is not installed where they run.
fn program(name: &str) -> Command {
    #[allow(unused_mut)]
    let mut command = Command::new(name);
    #[cfg(test)]
    command.env(
        "PATH",
        format!(
            "{}/../../scripts/fake:{}",
            env!("CARGO_MANIFEST_DIR"),
            std::env::var("PATH").unwrap_or_default()
        ),
    );
    command
}

fn finished(output: Output) -> Result<String> {
    if output.status.success() {
        return Ok(output.stdout);
    }
    let message = kubectl_error(&output.stderr);
    let lower = message.to_ascii_lowercase();
    let failure = if message.contains("(NotFound)") || lower.ends_with(" not found") {
        Failure::not_found(message)
    } else if message.contains("context \"") && message.contains("does not exist") {
        Failure::setup(message).hint(
            "the kubeconfig has no such context: agent-cli aks cluster connect NAME, or fix [[k8s.scope]] context",
        )
    } else if lower.contains("az login")
        || lower.contains("kubelogin")
        || lower.contains("unauthorized")
    {
        Failure::setup(message).hint("az login, then kubelogin convert-kubeconfig -l azurecli")
    } else if lower.contains("must be specified") || lower.contains("unknown flag") {
        Failure::usage(message)
    } else {
        Failure::new(Exit::Failed, message)
    };
    Err(failure.into())
}

/// The one line of kubectl's complaint that says what to fix. The client
/// logs a retry or two before it gives up and puts a documentation link after
/// the reason, so neither the first line nor the last is the one.
fn kubectl_error(stderr: &str) -> String {
    let lines: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !is_klog(line))
        .collect();
    let chosen = lines
        .iter()
        .find(|line| line.contains("az login"))
        .or_else(|| {
            lines.iter().find(|line| {
                line.starts_with("error:")
                    || line.starts_with("Error from server")
                    || line.starts_with("Unable to connect")
            })
        })
        .or_else(|| lines.first());
    chosen.map_or_else(
        || "kubectl failed".to_owned(),
        |line| {
            line.strip_prefix("error:")
                .map_or_else(|| (*line).to_owned(), |rest| rest.trim().to_owned())
        },
    )
}

/// `E0830 12:00:00.000000   12345 round_trippers.go:…] …`: the client's own
/// log line, which says nothing a person can act on.
fn is_klog(line: &str) -> bool {
    let mut characters = line.chars();
    matches!(characters.next(), Some('E' | 'W' | 'I' | 'F'))
        && characters.by_ref().take(4).all(|c| c.is_ascii_digit())
        && characters.next() == Some(' ')
}

fn items(listed: &Value) -> impl Iterator<Item = &Value> {
    listed["items"].as_array().into_iter().flatten()
}

fn non_empty(value: &Value) -> Option<&str> {
    value.as_str().filter(|held| !held.is_empty())
}

/// How long ago an RFC 3339 stamp was, as kubectl's AGE column says it:
/// `40m`, `6h`, `3d`, `2mo`, `2y`.
fn age(stamp: &Value) -> Option<String> {
    let then = OffsetDateTime::parse(stamp.as_str()?, &Rfc3339).ok()?;
    let seconds = (agent_cli_core::now() - then).whole_seconds().max(0);
    let (minutes, hours, days) = (seconds / 60, seconds / 3600, seconds / 86_400);
    Some(match () {
        () if minutes < 1 => format!("{seconds}s"),
        () if hours < 1 => format!("{minutes}m"),
        () if days < 1 => format!("{hours}h"),
        () if days < 60 => format!("{days}d"),
        () if days < 365 => format!("{}mo", days / 30),
        () => format!("{}y", days / 365),
    })
}

/// The first `limit` rows, with a note when there were more.
fn limited<T>(ctx: &Ctx, mut rows: Vec<T>, limit: usize) -> Vec<T> {
    if rows.len() > limit {
        ctx.note(format!("[{limit} of {}; --limit N]", rows.len()));
        rows.truncate(limit);
    }
    rows
}

// ---------- k8s context list ----------

#[derive(clap::Args)]
pub struct NoArgs {}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ContextRow {
    /// What --cluster takes.
    name: String,
    /// The kubeconfig context it runs kubectl against.
    context: String,
    /// Empty: every namespace.
    namespaces: Vec<String>,
    /// Whether the kubeconfig has that context.
    known: bool,
}

fn context_list(ctx: &Ctx, _: NoArgs) -> Result<Vec<ContextRow>> {
    let scopes = scopes(ctx.config())?;
    let mut contexts = program("kubectl");
    contexts.args(["config", "get-contexts", "-o", "name"]);
    let known = finished(ctx.read(contexts)?)?;
    let known: Vec<&str> = known.lines().map(str::trim).collect();
    Ok(scopes
        .into_iter()
        .map(|scope| ContextRow {
            known: known.contains(&scope.context()),
            context: scope.context().to_owned(),
            name: scope.name,
            namespaces: scope.namespaces,
        })
        .collect())
}

command! {
    pub CONTEXT_LIST = ["k8s", "context", "list"], Read,
    "List the configured cluster scopes and whether kubectl knows each context",
    keywords: ["kubeconfig", "clusters", "namespaces", "configured", "scope", "which"],
    example: "k8s context list --fields name,context,namespaces,known",
    run: context_list,
}

// ---------- overview and doctor ----------

fn status(config: &Config) -> String {
    if !config.has_section("k8s") {
        return "k8s not set up".to_owned();
    }
    match scopes(config) {
        Ok(scopes) if scopes.len() == 1 => "k8s 1 scope".to_owned(),
        Ok(scopes) => format!("k8s {} scopes", scopes.len()),
        Err(_) => "k8s config broken".to_owned(),
    }
}

/// kubectl on PATH, then one read in each scope and namespace: `get pods -o
/// name`, which every role that can use the scope at all is allowed.
fn doctor(ctx: &Ctx) -> Vec<Check> {
    if !ctx.config().has_section("k8s") {
        return Vec::new();
    }
    let scopes = match scopes(ctx.config()) {
        Ok(scopes) => scopes,
        Err(error) => {
            return vec![Check::failed(
                "config",
                format!("{error:#}"),
                "fix [[k8s.scope]]; config.example.toml shows the keys",
            )];
        }
    };
    let mut version = program("kubectl");
    version.args(["version", "--client", "-o", "json"]);
    let client = ctx.read(version).and_then(finished);
    match client {
        Ok(raw) => {
            let said: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
            let version = said["clientVersion"]["gitVersion"].as_str().unwrap_or("present");
            vec![Check::ok("kubectl", version.to_owned())]
        }
        Err(error) => {
            return vec![Check::failed("kubectl", format!("{error:#}"), "install kubectl, or put it on PATH")];
        }
    }
    .into_iter()
    .chain(scopes.iter().flat_map(|scope| {
        let namespaces: Vec<Option<String>> = if scope.namespaces.is_empty() {
            vec![None]
        } else {
            scope.namespaces.iter().cloned().map(Some).collect()
        };
        namespaces.into_iter().map(|namespace| {
            let check = match &namespace {
                Some(namespace) => format!("scope {}/{namespace}", scope.name),
                None => format!("scope {}", scope.name),
            };
            let target = Target {
                scope: scope.name.clone(),
                context: scope.context().to_owned(),
                namespace,
            };
            if ctx.remaining().is_err() {
                return Check::failed(check, "not checked: --timeout ran out", "agent-cli doctor k8s --timeout 120");
            }
            let started = std::time::Instant::now();
            match target.read(ctx, &["get", "pods", "-o", "name"]) {
                Ok(_) => Check::ok(check, format!("{} answered in {} ms", target.context, started.elapsed().as_millis())),
                Err(error) => {
                    let said = format!("{error:#}");
                    let hint = if said.contains("Forbidden") {
                        "RBAC: this login may not list pods here; ask for a role, or narrow the scope's namespaces"
                    } else {
                        "agent-cli aks cluster connect NAME refreshes the kubeconfig"
                    };
                    Check::failed(check, said, hint)
                }
            }
        })
    }))
    .collect()
}

#[cfg(test)]
pub(crate) mod fixtures {
    use agent_cli_core::Setup;
    use agent_cli_core::testing::{FakeTransport, Outcome, run};

    use crate::K8S;

    /// The fake kubectl's two contexts, one with three namespaces.
    pub const SCOPES: &str = r#"
[[k8s.scope]]
name = "qa"
context = "aks-qa"
namespaces = ["dev", "qa", "uat"]

[[k8s.scope]]
name = "prod"
context = "aks-prod"
namespaces = "prod"

[[k8s.scope]]
name = "all"
context = "aks-qa"
"#;

    pub fn k8s(argv: &[&str]) -> Outcome {
        k8s_with(argv, SCOPES)
    }

    pub fn k8s_with(argv: &[&str], config: &str) -> Outcome {
        run(
            &[K8S],
            argv,
            Setup::fake(FakeTransport::default()).with_config(config),
        )
    }
}

#[cfg(test)]
mod tests {
    use agent_cli_core::check_registry;
    use serde_json::json;

    use super::fixtures::{SCOPES, k8s, k8s_with};
    use super::*;

    #[test]
    fn the_registry_keeps_every_rule() {
        assert_eq!(check_registry(&[K8S]), Vec::<String>::new());
    }

    #[test]
    fn a_scope_and_namespace_default_only_when_there_is_exactly_one() {
        let outcome = k8s(&["k8s", "pod", "list"]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("name one with --cluster: qa, prod, all"),
            "{}",
            outcome.stderr
        );

        let outcome = k8s(&["k8s", "pod", "list", "--cluster", "qa"]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome.stderr.contains("--namespace: dev, qa, uat"),
            "{}",
            outcome.stderr
        );

        let outcome = k8s(&[
            "k8s",
            "pod",
            "list",
            "--cluster",
            "qa",
            "--namespace",
            "kube-system",
        ]);
        assert_eq!(
            outcome.code, 2,
            "a namespace outside the scope is refused: {outcome:?}"
        );

        let outcome = k8s(&["k8s", "pod", "list", "--cluster", "nope"]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome.stderr.contains("hint: agent-cli k8s context list"),
            "{}",
            outcome.stderr
        );

        let outcome = k8s(&[
            "k8s",
            "pod",
            "list",
            "--cluster",
            "prod",
            "--fields",
            "name",
        ]);
        assert_eq!(
            outcome.code, 0,
            "the only namespace is the default: {outcome:?}"
        );
        assert_eq!(outcome.json().as_array().unwrap().len(), 5);

        let outcome = k8s(&["k8s", "pod", "get", "redis-0", "--cluster", "all"]);
        assert_eq!(outcome.code, 2, "one object needs a namespace: {outcome:?}");

        let outcome = k8s_with(&["k8s", "pod", "list"], "");
        assert_eq!(outcome.code, 3, "{outcome:?}");
        assert!(
            outcome.stderr.contains("aks cluster connect"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn context_list_says_which_contexts_kubectl_knows() {
        let config = format!("{SCOPES}\n[[k8s.scope]]\nname = \"gone\"\n");
        let outcome = k8s_with(&["k8s", "context", "list"], &config);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let rows = outcome.json();
        assert_eq!(
            rows[0],
            json!({"name": "qa", "context": "aks-qa", "namespaces": ["dev", "qa", "uat"], "known": true})
        );
        assert_eq!(
            rows[3],
            json!({"name": "gone", "context": "gone", "known": false})
        );
    }

    #[test]
    fn kubectl_errors_read_as_the_one_line_that_says_what_to_fix() {
        let stderr = "E0912 12:00:00.000000   12345 memcache.go:265] couldn't get current server API group list\n\
                      Unable to connect to the server: dial tcp 10.0.0.1:443: i/o timeout\n";
        assert_eq!(
            kubectl_error(stderr),
            "Unable to connect to the server: dial tcp 10.0.0.1:443: i/o timeout"
        );
        assert_eq!(
            kubectl_error("error: context \"x\" does not exist\n"),
            "context \"x\" does not exist"
        );
        assert_eq!(kubectl_error(""), "kubectl failed");
        assert_eq!(
            kubectl_error(
                "I0101 00:00:00.0 1 x.go:1] retry\nerror: You must be logged in (run az login)\nsee docs\n"
            ),
            "You must be logged in (run az login)"
        );
    }

    #[test]
    fn an_unknown_context_is_needs_setup_and_an_unreachable_cluster_is_a_failure() {
        let outcome = k8s_with(
            &["k8s", "pod", "list"],
            "[[k8s.scope]]\nname = \"x\"\ncontext = \"aks-nope\"\nnamespaces = \"dev\"\n",
        );
        assert_eq!(outcome.code, 3, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .starts_with("error: context \"aks-nope\" does not exist"),
            "{}",
            outcome.stderr
        );

        let outcome = k8s(&[
            "k8s",
            "pod",
            "list",
            "--cluster",
            "all",
            "--namespace",
            "forbidden",
        ]);
        assert_eq!(outcome.code, 1, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .starts_with("error: Error from server (Forbidden)"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn the_overview_counts_scopes_and_a_broken_section_says_so() {
        let config = |toml: Option<&str>| Config::parse("c.toml", toml, Vec::new());
        assert_eq!(status(&config(Some(SCOPES))), "k8s 3 scopes");
        assert_eq!(status(&config(None)), "k8s not set up");
        assert_eq!(
            status(&config(Some("[[k8s.scope]]\nnam = \"x\"\n"))),
            "k8s config broken"
        );
    }

    #[test]
    fn doctor_checks_kubectl_and_each_scope_and_namespace() {
        let transport = agent_cli_core::testing::FakeTransport::default();
        let ctx = agent_cli_core::testing::ctx(agent_cli_core::Setup::fake(transport).with_config(
            "[[k8s.scope]]\nname = \"qa\"\ncontext = \"aks-qa\"\nnamespaces = [\"dev\", \"qa\"]\n\
                 [[k8s.scope]]\nname = \"x\"\ncontext = \"aks-nope\"\n",
        ));
        let checks = doctor(&ctx);
        let rows: Vec<(String, bool)> = checks
            .iter()
            .map(|check| (check.check.clone(), check.ok))
            .collect();
        assert_eq!(
            rows,
            [
                ("kubectl".to_owned(), true),
                ("scope qa/dev".to_owned(), true),
                ("scope qa/qa".to_owned(), true),
                ("scope x".to_owned(), false),
            ]
        );
        assert_eq!(checks[0].detail, "v1.31.2-fake");
    }
}
