use agent_cli_core::{Ctx, Diff, DiffArgs, Side, command, diff};
use anyhow::Result;

use crate::call::Fetch;
use crate::openapi::compare::{RULES, row};
use crate::openapi::load;

#[derive(clap::Args)]
pub struct SpecDiffArgs {
    /// The left spec, which clients were written against: a URL or a file
    left: String,
    /// The right spec, the one replacing it
    right: String,
    /// Only the changes that break a client of the left spec
    #[arg(long)]
    breaking: bool,
    #[command(flatten)]
    diff: DiffArgs,
    #[command(flatten)]
    fetch: Fetch,
}

fn spec_diff(ctx: &Ctx, args: SpecDiffArgs) -> Result<Diff> {
    let prepared = args.fetch.prepare(ctx)?;
    let left = load::spec(ctx, &prepared, &args.left)?;
    let right = load::spec(ctx, &prepared, &args.right)?;
    let mut wanted = args.diff.clone();
    if args.breaking {
        wanted.limit = usize::MAX;
    }
    let mut compared = diff(
        ctx,
        &wanted,
        &RULES,
        Side::new(&args.left, row(&left)),
        Side::new(&args.right, row(&right)),
    )?;
    let breaking: Vec<&str> = compared
        .changes
        .iter()
        .filter_map(|c| c.breaking.as_deref())
        .collect();
    if breaking.is_empty() {
        ctx.note("[nothing here breaks a client of the left spec]");
    } else {
        let shown: Vec<&str> = breaking.iter().take(3).copied().collect();
        let more = if breaking.len() > 3 { "; …" } else { "" };
        ctx.note(format!(
            "[{} breaking: {}{more}]",
            breaking.len(),
            shown.join("; ")
        ));
    }
    if args.breaking {
        compared.changes.retain(|change| change.breaking.is_some());
        if compared.changes.len() > args.diff.limit {
            ctx.note(format!(
                "[{} of {} breaking changes; --limit N]",
                args.diff.limit,
                compared.changes.len()
            ));
            compared.changes.truncate(args.diff.limit);
        }
    }
    Ok(compared)
}

command! {
    pub SPEC_DIFF = ["http", "spec", "diff"], Read,
    "What changed between OpenAPI specs, and which changes break their clients",
    keywords: ["openapi", "swagger", "breaking", "clients", "contract", "compatibility", "oasdiff"],
    example: "http spec diff https://api-staging.contoso.example/openapi.json ./openapi.yaml --breaking",
    run: spec_diff,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::Setup;
    use agent_cli_core::testing::{Answer, FakeTransport, run};

    use crate::DOMAIN;
    use crate::testing::{spec_v1, spec_v2};

    #[test]
    fn a_new_version_names_each_change_and_what_breaks() {
        let transport =
            FakeTransport::answering([Answer::json(&spec_v1()), Answer::json(&spec_v2())]);
        let outcome = run(
            &[DOMAIN],
            &[
                "http",
                "spec",
                "diff",
                "https://api.contoso.example/openapi.json",
                "https://api-next.contoso.example/openapi.json",
            ],
            Setup::fake(transport),
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let diff = outcome.json();
        let breaking: Vec<&str> = diff["changes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|c| c["breaking"].as_str())
            .collect();
        assert_eq!(
            breaking,
            [
                "GET /orders's parameter query status no longer takes on_hold",
                "GET /orders's 200 answer lost the field [].note",
                "POST /orders's body has a new required field currency",
                "POST /orders's 201 answer lost the field note",
                "GET /orders/{}'s 200 answer lost the field note",
                "DELETE /orders/{} is gone",
            ]
        );
        let added: Vec<&str> = diff["changes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["left"].is_null() && c["breaking"].is_null())
            .map(|c| c["path"].as_str().unwrap())
            .collect();
        assert!(
            added.contains(&"operations.GET /orders/{}/fulfilment"),
            "{added:?}"
        );
        assert!(
            outcome.stderr.contains("[6 breaking:"),
            "{}",
            outcome.stderr
        );
        let only = run(
            &[DOMAIN],
            &[
                "http",
                "spec",
                "diff",
                "https://api.contoso.example/openapi.json",
                "https://api-next.contoso.example/openapi.json",
                "--breaking",
            ],
            Setup::fake(FakeTransport::answering([
                Answer::json(&spec_v1()),
                Answer::json(&spec_v2()),
            ])),
        );
        assert_eq!(only.json()["changes"].as_array().unwrap().len(), 6);
    }
}
