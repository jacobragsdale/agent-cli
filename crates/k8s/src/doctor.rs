//! The overview's line for k8s, and `agent-cli doctor k8s`.

use agent_cli_core::{Check, Config, Ctx};
use serde_json::Value;

use crate::kubectl::{Target, finished, program, scopes};

pub(crate) fn status(config: &Config) -> String {
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
pub(crate) fn doctor(ctx: &Ctx) -> Vec<Check> {
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
mod tests {

    use super::*;
    use crate::testing::SCOPES;

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
