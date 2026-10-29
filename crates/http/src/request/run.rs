use std::path::PathBuf;

use agent_cli_core::{Ctx, Failure, Method, command};
use anyhow::Result;

use crate::answer::Answer;
use crate::call::{Call, body};
use crate::openapi::{check, load};

#[derive(clap::Args)]
pub struct RequestRunArgs {
    /// GET, HEAD, OPTIONS, POST, PUT, PATCH or DELETE
    method: String,
    /// The URL: https://HOST/PATH?QUERY
    url: String,
    /// The body: JSON, or - to read it from stdin
    #[arg(long)]
    body: Option<String>,
    /// The body from a file
    #[arg(long)]
    body_file: Option<PathBuf>,
    /// The body's type (default application/json)
    #[arg(long)]
    content_type: Option<String>,
    /// A form field K=V (repeatable), urlencoded
    #[arg(long, value_name = "K=V")]
    form: Vec<String>,
    /// The POST only reads (a search): no --dry-run stop
    #[arg(long)]
    reads: bool,
    /// Follow redirects; the sign-in stays on the first origin
    #[arg(long)]
    follow: bool,
    /// Check the call against this OpenAPI spec first
    #[arg(long, value_name = "SPEC")]
    spec: Option<String>,
    /// Items kept per JSON array
    #[arg(long, default_value_t = 50)]
    limit: usize,
    #[command(flatten)]
    call: Call,
}

fn request_run(ctx: &Ctx, args: RequestRunArgs) -> Result<Answer> {
    let mut method = crate::request::method(&args.method)?;
    if args.reads {
        if method != Method::Post {
            return Err(
                Failure::usage("--reads marks a POST; other methods say what they do").into(),
            );
        }
        method = Method::Query;
    }
    let (body, content_type) = body(
        ctx,
        args.body.as_deref(),
        args.body_file.as_deref(),
        args.content_type.as_deref(),
        &args.form,
    )?;
    let prepared = args.call.prepare(ctx)?.typed(content_type);
    let url = prepared.url(&args.url)?;
    let spec = args
        .spec
        .as_deref()
        .map(|spec| load::spec(ctx, &prepared, spec))
        .transpose()?;
    let operation = match &spec {
        Some(spec) => {
            let (operation, problems) = check::request(spec, method.wire(), &url, &body)?;
            if !problems.is_empty() {
                return Err(Failure::usage(format!(
                    "the call does not match {} {} in {}: {}",
                    operation.method,
                    operation.path,
                    spec.id,
                    problems.join("; ")
                ))
                .hint(format!(
                    "agent-cli http operation get '{}'  shows what it takes",
                    operation.id(spec)
                ))
                .with_data(&problems)
                .into());
            }
            Some(operation)
        }
        None => None,
    };
    let exchange = prepared.send(ctx, method, &url, body, args.follow)?;
    let raw = exchange.response.body.clone();
    let mut answer = Answer::of(ctx, exchange, args.limit, true)?;
    if let (Some(spec), Some(operation)) = (&spec, operation) {
        answer.spec_problems = check::response(spec, &operation, answer.status, &raw);
        if !answer.spec_problems.is_empty() {
            ctx.note(format!(
                "[{} differences from the spec's {} {}; spec_problems lists them]",
                answer.spec_problems.len(),
                operation.method,
                operation.path
            ));
        }
    }
    answer.finish(ctx)
}

command! {
    pub REQUEST_RUN = ["http", "request", "run"], Varies,
    "Send a request of any method to a URL, with a body, as curl -X does",
    keywords: ["curl", "rest", "put", "patch", "delete", "send", "posting", "endpoint", "body", "http"],
    example: "http request run POST https://api.contoso.example/orders --body '{\"customer_id\": 7}' --bearer-env API_TOKEN",
    run: request_run,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::Setup;
    use agent_cli_core::testing::{Answer, FakeTransport, assert_dry_run, run};
    use serde_json::json;

    use crate::DOMAIN;

    const POST: &[&str] = &[
        "http",
        "request",
        "run",
        "POST",
        "https://api.contoso.example/orders",
        "--body",
        r#"{"customer_id": 7}"#,
    ];

    #[test]
    fn a_post_sends_its_json_body_and_prints_the_created_answer() {
        let transport = FakeTransport::answering([Answer::status(201, r#"{"id": 601}"#)
            .with_header("Content-Type", "application/json")
            .with_header("Location", "/orders/601")]);
        let outcome = run(&[DOMAIN], POST, Setup::fake(transport.clone()));
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["status"], 201);
        assert_eq!(outcome.json()["body"]["id"], 601);
        let sent = &transport.sent()[0];
        assert_eq!(sent.body, Some(json!({"customer_id": 7})));
        assert_eq!(outcome.json()["headers"]["location"], "/orders/601");
    }

    #[test]
    fn a_post_plans_its_change_under_dry_run() {
        let plans = assert_dry_run(&[DOMAIN], POST, vec![]);
        assert_eq!(plans[0]["method"], "POST");
        assert_eq!(plans[0]["url"], "https://api.contoso.example/orders");
        assert_eq!(plans[0]["body"], json!({"customer_id": 7}));
    }

    #[test]
    fn a_delete_needs_yes_and_a_get_through_run_reads() {
        let argv = [
            "http",
            "request",
            "run",
            "DELETE",
            "https://api.contoso.example/orders/7",
        ];
        let refused = run(&[DOMAIN], &argv, Setup::fake(FakeTransport::default()));
        assert_eq!(refused.code, 2, "{refused:?}");
        assert!(refused.stderr.contains("--yes"));
        let transport = FakeTransport::answering([Answer::status(204, "")]);
        let yes = run(
            &[DOMAIN],
            &[&argv[..], &["--yes"]].concat(),
            Setup::fake(transport),
        );
        assert_eq!(yes.code, 0, "{yes:?}");
        let transport = FakeTransport::answering([Answer::json(&json!([]))]);
        let get = run(
            &[DOMAIN],
            &[
                "http",
                "request",
                "run",
                "get",
                "https://api.contoso.example/orders",
                "--dry-run",
            ],
            Setup::fake(transport.clone()),
        );
        assert_eq!(get.code, 0, "{get:?}");
        assert_eq!(transport.sent().len(), 1, "a read runs under --dry-run");
    }

    #[test]
    fn a_post_that_only_reads_is_sent_under_dry_run() {
        let transport = FakeTransport::answering([Answer::json(&json!({"items": []}))]);
        let outcome = run(
            &[DOMAIN],
            &[
                "http",
                "request",
                "run",
                "POST",
                "https://api.contoso.example/orders/search",
                "--body",
                "{}",
                "--reads",
                "--dry-run",
            ],
            Setup::fake(transport.clone()),
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(transport.sent()[0].method.wire(), "POST");
    }

    #[test]
    fn a_body_that_is_not_json_needs_its_content_type() {
        let refused = run(
            &[DOMAIN],
            &[
                "http",
                "request",
                "run",
                "POST",
                "https://h.example/x",
                "--body",
                "plain words",
            ],
            Setup::fake(FakeTransport::default()),
        );
        assert_eq!(refused.code, 2, "{refused:?}");
        assert!(refused.stderr.contains("--content-type"));
        let transport = FakeTransport::answering([Answer::ok("ok")]);
        let sent = run(
            &[DOMAIN],
            &[
                "http",
                "request",
                "run",
                "POST",
                "https://h.example/x",
                "--body",
                "plain words",
                "--content-type",
                "text/plain",
            ],
            Setup::fake(transport.clone()),
        );
        assert_eq!(sent.code, 0, "{sent:?}");
        assert!(
            transport.sent()[0]
                .headers
                .contains(&("Content-Type".to_owned(), "text/plain".to_owned()))
        );
    }

    #[test]
    fn a_body_comes_from_stdin_for_a_dash() {
        let transport = FakeTransport::answering([Answer::status(201, "{}")]);
        let outcome = run(
            &[DOMAIN],
            &[
                "http",
                "request",
                "run",
                "PUT",
                "https://h.example/orders/1",
                "--body",
                "-",
            ],
            Setup::fake(transport.clone()).with_stdin(r#"{"note": "piped"}"#),
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(transport.sent()[0].body, Some(json!({"note": "piped"})));
    }
}
