//! What every k8s command shares: the scopes, `At` and `Target`, kubectl
//! run and its errors read, and the readers of kubectl's JSON.

use std::process::Command;

use agent_cli_core::{Config, Ctx, Effect, Exit, Failure, Output, pick};
use anyhow::{Context, Result, bail};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

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
pub(crate) struct Scope {
    pub(crate) name: String,
    #[serde(default)]
    context: Option<String>,
    #[serde(default, deserialize_with = "one_or_many")]
    pub(crate) namespaces: Vec<String>,
}

impl Scope {
    pub(crate) fn context(&self) -> &str {
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

pub(crate) fn scopes(config: &Config) -> Result<Vec<Scope>> {
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
pub(crate) struct Target {
    pub(crate) scope: String,
    pub(crate) context: String,
    /// `None` is every namespace, for a listing in a scope that names none.
    pub(crate) namespace: Option<String>,
}

impl At {
    /// For a listing: a scope that names no namespaces lists all of them.
    pub(crate) fn listing(&self, ctx: &Ctx) -> Result<Target> {
        self.resolve(ctx, false)
    }

    /// For one object, which lives in exactly one namespace.
    pub(crate) fn one(&self, ctx: &Ctx) -> Result<Target> {
        self.resolve(ctx, true)
    }

    /// One object as an agent was handed it: its id (`cluster/namespace/name`,
    /// as every row prints it), `namespace/name`, or a name, with `--cluster`
    /// and `--namespace` filling in what it leaves out. A ref and a flag that
    /// disagree is exit 2: the CLI never picks one.
    pub(crate) fn named(&self, ctx: &Ctx, raw: &str) -> Result<(Target, String)> {
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
                .hint("agent-cli aks cluster connect NAME  (it prints one to paste; config.example.toml shows the keys)")
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
    pub(crate) fn kubectl(&self, args: &[&str]) -> Command {
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

    pub(crate) fn read(&self, ctx: &Ctx, args: &[&str]) -> Result<String> {
        finished(ctx.read(self.kubectl(args))?)
    }

    pub(crate) fn json(&self, ctx: &Ctx, args: &[&str]) -> Result<Value> {
        let text = self.read(ctx, args)?;
        if text.trim().is_empty() {
            anyhow::bail!("kubectl answered with nothing, which is not an empty list");
        }
        serde_json::from_str(&text).context("kubectl answered with something other than JSON")
    }

    /// Every change k8s makes (delete, restart, scale) is destructive: it
    /// needs --yes.
    pub(crate) fn write(&self, ctx: &Ctx, args: &[&str]) -> Result<String> {
        finished(ctx.write(Effect::Destructive, self.kubectl(args))?)
    }

    /// The namespace a row carries: only when the listing spans several, where
    /// it is the one thing that tells two rows apart.
    pub(crate) fn row_namespace(&self, item: &Value) -> Option<String> {
        self.namespace
            .is_none()
            .then(|| item["metadata"]["namespace"].as_str().map(str::to_owned))
            .flatten()
    }

    /// An object's id, `cluster/namespace/name`: what every k8s command that
    /// takes one object accepts, with no other flag.
    pub(crate) fn id(&self, item: &Value) -> String {
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
pub(crate) fn program(name: &str) -> Command {
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

pub(crate) fn finished(output: Output) -> Result<String> {
    if output.status.success() {
        return Ok(output.stdout);
    }
    let message = kubectl_error(&output.stderr);
    let lower = message.to_ascii_lowercase();
    let failure = if message.contains("(NotFound)") || lower.ends_with(" not found") {
        // `pods "x" not found`, `deployments.apps "x" not found`: the listing
        // that shows what is there.
        let listed = ["pod", "deployment", "configmap", "secret"]
            .into_iter()
            .find(|kind| {
                message.contains(&format!("{kind}s \""))
                    || message.contains(&format!("{kind}s.apps \""))
            });
        match listed {
            Some(kind) => Failure::not_found(message).hint(format!("agent-cli k8s {kind} list")),
            None => Failure::not_found(message),
        }
    } else if message.contains("context \"") && message.contains("does not exist") {
        Failure::setup(message).hint(
            "the kubeconfig has no such context: agent-cli aks cluster connect NAME, or fix [[k8s.scope]] context",
        )
    } else if lower.contains("az login")
        || lower.contains("kubelogin")
        || lower.contains("unauthorized")
    {
        Failure::setup(message).hint("az login, then kubelogin convert-kubeconfig -l azurecli")
    } else if lower.contains("unable to connect to the server") {
        Failure::new(Exit::Failed, message)
            .hint("agent-cli doctor k8s  (the cluster's API server did not answer)")
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

pub(crate) fn items(listed: &Value) -> impl Iterator<Item = &Value> {
    listed["items"].as_array().into_iter().flatten()
}

pub(crate) fn non_empty(value: &Value) -> Option<&str> {
    value.as_str().filter(|held| !held.is_empty())
}

/// How long ago an RFC 3339 stamp was, as kubectl's AGE column says it:
/// `40m`, `6h`, `3d`, `2mo`, `2y`.
pub(crate) fn age(stamp: &Value) -> Option<String> {
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
pub(crate) fn limited<T>(ctx: &Ctx, mut rows: Vec<T>, limit: usize) -> Vec<T> {
    if rows.len() > limit {
        ctx.note(format!("[{limit} of {}; --limit N]", rows.len()));
        rows.truncate(limit);
    }
    rows
}

/// The `sha256:…` a container status's `imageID` ends in
/// (`docker-pullable://…@sha256:…` or `…/api@sha256:…`).
pub(crate) fn digest(image_id: &Value) -> Option<String> {
    let (_, digest) = image_id.as_str()?.rsplit_once('@')?;
    (!digest.is_empty()).then(|| digest.to_owned())
}

/// What made the pod, as `Kind/name`. A ReplicaSet named after a
/// pod-template hash is a Deployment's, and is reported as that Deployment,
/// which is the name `k8s deployment restart` takes.
// ponytail: a Job's CronJob is not resolved; a ReplicaSet with no hash label
// stays a ReplicaSet.
pub(crate) fn owner_of(item: &Value) -> Option<String> {
    let references = item["metadata"]["ownerReferences"].as_array()?;
    let owner = references
        .iter()
        .find(|reference| reference["controller"].as_bool() == Some(true))
        .or_else(|| references.first())?;
    let kind = owner["kind"].as_str()?;
    let name = owner["name"].as_str()?;
    if kind == "ReplicaSet"
        && let Some(hash) = non_empty(&item["metadata"]["labels"]["pod-template-hash"])
        && let Some(base) = name.strip_suffix(&format!("-{hash}"))
    {
        return Some(format!("Deployment/{base}"));
    }
    Some(format!("{kind}/{name}"))
}

/// What a change did, as kubectl said it.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Changed {
    cluster: String,
    namespace: String,
    /// pod/NAME, deployment/NAME…
    object: String,
    /// kubectl's own line: `pod "x" deleted`.
    said: String,
    /// After a scale.
    replicas: Option<u32>,
    /// Before a scale.
    previous: Option<i64>,
}

impl Changed {
    pub(crate) fn new(
        target: &Target,
        object: String,
        said: &str,
        replicas: Option<u32>,
        previous: Option<i64>,
    ) -> Self {
        Self {
            cluster: target.scope.clone(),
            namespace: target.namespace.clone().unwrap_or_default(),
            object,
            said: said.trim().to_owned(),
            replicas,
            previous,
        }
    }
}

/// How many bytes a base64 string decodes to, without decoding it.
pub(crate) fn decoded_len(encoded: &str) -> usize {
    encoded.trim_end_matches('=').len() * 3 / 4
}

/// Standard base64, with or without padding, whitespace ignored.
// ponytail: twenty lines rather than a crate for the one decode.
pub(crate) fn base64_decode(encoded: &str) -> Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(encoded.len() * 3 / 4);
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for character in encoded.bytes() {
        let value = match character {
            b'A'..=b'Z' => character - b'A',
            b'a'..=b'z' => character - b'a' + 26,
            b'0'..=b'9' => character - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b' ' | b'\n' | b'\r' | b'\t' => continue,
            other => bail!("not base64: byte {other:#x}"),
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            bytes.push(((buffer >> bits) & 0xff) as u8);
        }
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::testing::{k8s, k8s_with};

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
}
