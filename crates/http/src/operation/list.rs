use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::call::Fetch;
use crate::openapi::fields::flatten;
use crate::openapi::{Operation, Spec, load, squash};

#[derive(clap::Args)]
pub struct OperationListArgs {
    /// The spec: its URL, its docs page, the API's base URL, or a file
    spec: String,
    /// Words its method, path, summary, id or tags hold
    words: Vec<String>,
    /// Only operations with this tag
    #[arg(long)]
    tag: Option<String>,
    /// Only this method: GET, POST…
    #[arg(long)]
    method: Option<String>,
    /// Only paths matching this, * for any run: /orders/*
    #[arg(long)]
    path: Option<String>,
    /// Only operations taking a parameter or body field so named
    #[arg(long)]
    param: Option<String>,
    /// Only operations using this schema (component)
    #[arg(long, value_name = "SCHEMA")]
    uses: Option<String>,
    /// Only deprecated operations
    #[arg(long)]
    deprecated: bool,
    /// Only operations that need no sign-in
    #[arg(long)]
    anonymous: bool,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
    #[command(flatten)]
    fetch: Fetch,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct OperationRow {
    /// What `http operation get` takes: SPEC#METHOD PATH.
    id: String,
    method: String,
    path: String,
    summary: Option<String>,
    operation_id: Option<String>,
    tags: Vec<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    deprecated: bool,
    /// The sign-in it needs, or none.
    security: Vec<String>,
}

fn operation_list(ctx: &Ctx, args: OperationListArgs) -> Result<Vec<OperationRow>> {
    let prepared = args.fetch.prepare(ctx)?;
    let spec = load::spec(ctx, &prepared, &args.spec)?;
    let words: Vec<String> = args
        .words
        .iter()
        .flat_map(|w| w.split_whitespace())
        .map(str::to_lowercase)
        .collect();
    let matching: Vec<&Operation> = spec
        .operations
        .iter()
        .filter(|op| keeps(&spec, op, &args, &words))
        .collect();
    let total = matching.len();
    let rows: Vec<OperationRow> = matching
        .into_iter()
        .take(args.limit)
        .map(|op| {
            let needs = op.needs(&spec);
            OperationRow {
                id: op.id(&spec),
                method: op.method.clone(),
                path: op.path.clone(),
                summary: op.summary.clone(),
                operation_id: op.operation_id.clone(),
                tags: op.tags.clone(),
                deprecated: op.deprecated,
                security: if needs.is_empty() {
                    vec!["none".to_owned()]
                } else {
                    needs.to_vec()
                },
            }
        })
        .collect();
    if total > rows.len() {
        ctx.note(format!("[{} of {total}; --limit N]", rows.len()));
    }
    if total == 0 {
        ctx.note(format!(
            "[none of its {} operations matches; agent-cli http operation list {}  lists them all]",
            spec.operations.len(),
            spec.id
        ));
    }
    Ok(rows)
}

fn keeps(spec: &Spec, op: &Operation, args: &OperationListArgs, words: &[String]) -> bool {
    if let Some(method) = &args.method
        && !op.method.eq_ignore_ascii_case(method)
    {
        return false;
    }
    if let Some(tag) = &args.tag
        && !op.tags.iter().any(|held| held.eq_ignore_ascii_case(tag))
    {
        return false;
    }
    if let Some(pattern) = &args.path
        && !glob(pattern, &op.path)
    {
        return false;
    }
    if args.deprecated && !op.deprecated {
        return false;
    }
    if args.anonymous && !op.needs(spec).is_empty() {
        return false;
    }
    if let Some(param) = &args.param {
        let wanted = squash(param);
        let in_params = op.parameters.iter().any(|p| squash(&p.name) == wanted);
        let in_body = op.body_schema().is_some_and(|schema| {
            flatten(spec, schema).iter().any(|field| {
                squash(
                    field
                        .path
                        .rsplit(['.', ']', '}'])
                        .next()
                        .unwrap_or(&field.path),
                ) == wanted
            })
        });
        if !in_params && !in_body {
            return false;
        }
    }
    if let Some(name) = &args.uses
        && !spec
            .uses(op)
            .iter()
            .any(|used| used.eq_ignore_ascii_case(name) || squash(used) == squash(name))
    {
        return false;
    }
    let text = format!(
        "{} {} {} {} {} {}",
        op.method,
        op.path,
        op.summary.as_deref().unwrap_or(""),
        op.operation_id.as_deref().unwrap_or(""),
        op.tags.join(" "),
        op.description.as_deref().unwrap_or("")
    )
    .to_lowercase();
    words.iter().all(|word| text.contains(word.as_str()))
}

/// `*` for any run of characters, the rest as written.
fn glob(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }
    let mut at = 0;
    for (index, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        match text[at..].find(part) {
            Some(found) if index > 0 || found == 0 => at += found + part.len(),
            _ => return false,
        }
    }
    parts
        .last()
        .is_some_and(|last| last.is_empty() || text.ends_with(last))
}

command! {
    pub OPERATION_LIST = ["http", "operation", "list"], Read,
    "List a spec's operations (endpoints): by words, tag, path, a parameter taken",
    keywords: ["endpoints", "openapi", "swagger", "paths", "parameter", "authentication", "anonymous", "route", "routes"],
    example: "http operation list https://api.contoso.example --param customer_id --fields id,summary",
    run: operation_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::Setup;
    use agent_cli_core::testing::{Answer, FakeTransport, run};

    use crate::DOMAIN;
    use crate::testing::spec_v1;

    fn ids(argv: &[&str]) -> Vec<String> {
        let transport = FakeTransport::answering([Answer::json(&spec_v1())]);
        let outcome = run(&[DOMAIN], argv, Setup::fake(transport));
        assert_eq!(outcome.code, 0, "{outcome:?}");
        outcome
            .json()
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["id"].as_str().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn operations_are_found_by_what_they_take_use_and_say() {
        let spec = "https://api.contoso.example/openapi.json";
        let all = ids(&["http", "operation", "list", spec]);
        assert_eq!(all.len(), 4);
        assert_eq!(all[0], format!("{spec}#GET /orders"));
        assert_eq!(
            ids(&["http", "operation", "list", spec, "--param", "customerId"]),
            [format!("{spec}#POST /orders")]
        );
        assert_eq!(
            ids(&["http", "operation", "list", spec, "--uses", "Order"]).len(),
            3
        );
        assert_eq!(
            ids(&["http", "operation", "list", spec, "delete"]),
            [format!("{spec}#DELETE /orders/{{order_id}}")]
        );
        assert_eq!(
            ids(&["http", "operation", "list", spec, "--path", "/orders/*"]).len(),
            2
        );
        assert_eq!(
            ids(&["http", "operation", "list", spec, "--anonymous"]),
            [format!("{spec}#GET /orders")]
        );
    }

    #[test]
    fn a_glob_matches_runs() {
        assert!(super::glob("/orders/*", "/orders/{id}"));
        assert!(super::glob("*/fulfilment", "/orders/{id}/fulfilment"));
        assert!(!super::glob("/orders/*", "/customers/1"));
        assert!(super::glob("/orders", "/orders"));
    }
}
