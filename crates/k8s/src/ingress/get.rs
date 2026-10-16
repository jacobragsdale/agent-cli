use agent_cli_core::{Ctx, Exit, Failure, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};

use crate::kubectl::{At, Target, items, non_empty};
use crate::service::{endpoints, ready};
use crate::shape::{EventRow, events_for};

use super::{address, class};

#[derive(clap::Args)]
pub struct IngressGetArgs {
    /// The ingress: its id (cluster/namespace/name), namespace/name, or name
    ingress: String,
    #[command(flatten)]
    at: At,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct IngressDetail {
    id: String,
    name: String,
    namespace: String,
    class: Option<String>,
    /// What serves the class, from its IngressClass: k8s.io/ingress-nginx…
    controller: Option<String>,
    address: Vec<String>,
    /// One per host and path.
    rules: Vec<Backend>,
    /// Where a request no rule matches goes.
    default_backend: Option<Backend>,
    tls: Vec<Tls>,
    /// The five newest events about it, newest first.
    events: Vec<EventRow>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Backend {
    host: Option<String>,
    path: Option<String>,
    /// The service's id: what `k8s service get` takes.
    service: Option<String>,
    /// The service port, by number or name.
    port: Option<String>,
    /// The service's endpoints ready of all, or `no such service`.
    ready: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Tls {
    hosts: Vec<String>,
    /// The secret's id: what `k8s secret list` shows.
    secret: Option<String>,
    /// Whether the secret is there (read by name only, never its values).
    exists: Option<bool>,
}

fn ingress_get(ctx: &Ctx, args: IngressGetArgs) -> Result<IngressDetail> {
    let (target, name) = args.at.named(ctx, &args.ingress)?;
    let item = target.json(ctx, &["get", "ingress", &name, "-o", "json"])?;
    let listed = target.json(
        ctx,
        &[
            "get",
            "services,endpointslices,ingressclasses",
            "-o",
            "json",
        ],
    )?;
    let spec = &item["spec"];
    let class = class(&item);
    let mut rules = Vec::new();
    for rule in spec["rules"].as_array().into_iter().flatten() {
        let host = non_empty(&rule["host"]);
        for path in rule["http"]["paths"].as_array().into_iter().flatten() {
            let path_text = non_empty(&path["path"]);
            rules.push(backend(&target, &listed, host, path_text, &path["backend"]));
        }
    }
    let default_backend = spec
        .get("defaultBackend")
        .map(|held| backend(&target, &listed, None, None, held));
    if let Some(next) = rules
        .iter()
        .chain(&default_backend)
        .find_map(|backend| attention(&target, backend))
    {
        ctx.note(format!("[next: {next}]"));
    }
    let tls = spec["tls"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|tls| {
            let secret = non_empty(&tls["secretName"]);
            Tls {
                hosts: tls["hosts"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|host| host.as_str().map(str::to_owned))
                    .collect(),
                secret: secret.map(|secret| id(&target, secret)),
                exists: secret.and_then(|secret| exists(ctx, &target, secret)),
            }
        })
        .collect();
    let controller = class.as_deref().and_then(|class| {
        items(&listed)
            .find(|held| held["kind"] == "IngressClass" && held["metadata"]["name"] == class)
            .and_then(|held| non_empty(&held["spec"]["controller"]))
            .map(str::to_owned)
    });
    Ok(IngressDetail {
        id: target.id(&item),
        name,
        namespace: target.namespace.clone().unwrap_or_default(),
        class,
        controller,
        address: address(&item),
        rules,
        default_backend,
        tls,
        events: events_for(ctx, &target, &item),
    })
}

fn backend(
    target: &Target,
    listed: &Value,
    host: Option<&str>,
    path: Option<&str>,
    backend: &Value,
) -> Backend {
    let service = &backend["service"];
    let name = non_empty(&service["name"]);
    let port = &service["port"];
    Backend {
        host: host.map(str::to_owned),
        path: path.map(str::to_owned),
        service: name.map(|name| id(target, name)),
        port: port["number"]
            .as_i64()
            .map(|number| number.to_string())
            .or_else(|| non_empty(&port["name"]).map(str::to_owned)),
        ready: name.map(|name| {
            items(listed)
                .find(|held| held["kind"] == "Service" && held["metadata"]["name"] == name)
                .map_or_else(
                    || "no such service".to_owned(),
                    |held| ready(&endpoints(target, listed, held)),
                )
        }),
    }
}

/// The command for a backend that answers nothing: the service's own
/// `get` says why, and the namespace's services when it is not there.
fn attention(target: &Target, backend: &Backend) -> Option<String> {
    let ready = backend.ready.as_deref()?;
    if ready == "no such service" {
        let namespace = target.namespace.as_deref().unwrap_or_default();
        return Some(format!(
            "agent-cli k8s service list --cluster {} --namespace {namespace}",
            target.scope
        ));
    }
    let service = backend.service.as_deref()?;
    ready
        .starts_with("0/")
        .then(|| format!("agent-cli k8s service get {service}"))
}

/// Whether a secret is there, asked by name so its values never leave the
/// API server: `None` when kubectl could not tell (a note says why).
fn exists(ctx: &Ctx, target: &Target, secret: &str) -> Option<bool> {
    match target.read(ctx, &["get", "secret", secret, "-o", "name"]) {
        Ok(_) => Some(true),
        Err(error)
            if error
                .downcast_ref::<Failure>()
                .is_some_and(|failure| matches!(failure.exit, Exit::NotFound)) =>
        {
            Some(false)
        }
        Err(error) => {
            ctx.note(format!("[secret {secret} not read: {error:#}]"));
            None
        }
    }
}

fn id(target: &Target, name: &str) -> String {
    target.id(&json!({"metadata": {"name": name}}))
}

command! {
    pub INGRESS_GET = ["k8s", "ingress", "get"], Read,
    "Show an ingress: each host and path's service and ready endpoints, TLS secrets",
    keywords: ["kubernetes", "backend", "https", "certificate", "502", "503", "controller", "url", "path"],
    example: "k8s ingress get qa/dev/orders --fields rules,tls",
    run: ingress_get,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::run;

    #[test]
    fn an_ingress_shows_each_backend_ready_and_whether_its_tls_secrets_exist() {
        let outcome = run(&["k8s", "ingress", "get", "orders"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let ingress = outcome.json();
        assert_eq!(ingress["controller"], "k8s.io/ingress-nginx");
        assert_eq!(ingress["address"], json!(["10.240.0.60"]));
        assert_eq!(
            ingress["rules"],
            json!([
                {"host": "orders.contoso.example", "path": "/", "service": "qa/dev/orders-api", "port": "http", "ready": "2/2"},
                {"host": "orders.contoso.example", "path": "/reports", "service": "qa/dev/reports", "port": "80", "ready": "0/0"},
                {"host": "billing.contoso.example", "path": "/", "service": "qa/dev/billing-gone", "port": "443", "ready": "no such service"},
            ])
        );
        assert_eq!(
            ingress["tls"],
            json!([
                {"hosts": ["orders.contoso.example"], "secret": "qa/dev/orders-tls", "exists": true},
                {"hosts": ["billing.contoso.example"], "secret": "qa/dev/billing-tls", "exists": false},
            ])
        );
        assert!(
            outcome
                .stderr
                .contains("[next: agent-cli k8s service get qa/dev/reports]"),
            "{}",
            outcome.stderr
        );
        assert!(
            !outcome.stdout.contains("LS0tLS1CRUdJTg"),
            "never the secret's values"
        );

        let outcome = run(&["k8s", "ingress", "get", "gone"]);
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome.stderr.contains("hint: agent-cli k8s ingress list"),
            "{}",
            outcome.stderr
        );
    }
}
