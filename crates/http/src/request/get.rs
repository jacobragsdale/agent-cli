use agent_cli_core::{Body, Ctx, Method, command};
use anyhow::Result;

use crate::answer::Answer;
use crate::call::Call;

#[derive(clap::Args)]
pub struct RequestGetArgs {
    /// The URL: https://HOST/PATH?QUERY
    url: String,
    /// HEAD instead of GET: the status and headers only
    #[arg(long)]
    head: bool,
    /// Follow redirects; the sign-in stays on the first origin
    #[arg(long)]
    follow: bool,
    /// Items kept per JSON array
    #[arg(long, default_value_t = 50)]
    limit: usize,
    #[command(flatten)]
    call: Call,
}

fn request_get(ctx: &Ctx, args: RequestGetArgs) -> Result<Answer> {
    let prepared = args.call.prepare(ctx)?;
    let url = prepared.url(&args.url)?;
    let method = if args.head { Method::Head } else { Method::Get };
    let exchange = prepared.send(ctx, method, &url, Body::None, args.follow)?;
    Answer::of(ctx, exchange, args.limit, true)?.finish(ctx)
}

command! {
    pub REQUEST_GET = ["http", "request", "get"], Read,
    "GET a URL, as curl does: status, headers, the body bounded; a 4xx or 5xx fails",
    keywords: ["curl", "rest", "fetch", "endpoint", "url", "status", "headers", "json", "http", "returning"],
    example: "http request get https://api.contoso.example/orders/42 --bearer-env API_TOKEN",
    run: request_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::Setup;
    use agent_cli_core::testing::{Answer, FakeTransport, run};
    use serde_json::json;

    use crate::DOMAIN;

    #[test]
    fn a_get_prints_the_status_headers_and_body() {
        let transport =
            FakeTransport::answering([Answer::json(&json!({"id": 42, "status": "open"}))
                .with_header("Content-Type", "application/json")
                .with_header("X-Request-Id", "r-1")]);
        let outcome = run(
            &[DOMAIN],
            &[
                "http",
                "request",
                "get",
                "https://api.contoso.example/orders/42",
                "--query",
                "expand=lines",
            ],
            Setup::fake(transport.clone()),
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let row = outcome.json();
        assert_eq!(row["status"], 200);
        assert_eq!(row["reason"], "OK");
        assert_eq!(row["body"], json!({"id": 42, "status": "open"}));
        assert_eq!(row["headers"]["x-request-id"], "r-1");
        assert_eq!(
            transport.sent()[0].url,
            "https://api.contoso.example/orders/42?expand=lines"
        );
    }

    #[test]
    fn a_not_found_prints_the_answer_and_exits_four() {
        let transport =
            FakeTransport::answering([Answer::status(404, r#"{"detail":"order 9999 not found"}"#)
                .with_header("Content-Type", "application/json")]);
        let outcome = run(
            &[DOMAIN],
            &[
                "http",
                "request",
                "get",
                "https://api.contoso.example/orders/9999",
            ],
            Setup::fake(transport),
        );
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert_eq!(outcome.json()["body"]["detail"], "order 9999 not found");
        assert!(
            outcome
                .stderr
                .contains("answered 404 Not Found: order 9999 not found"),
            "{}",
            outcome.stderr
        );
        assert!(
            outcome
                .stderr
                .contains("agent-cli http operation list https://api.contoso.example"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn a_server_error_names_its_trace_for_datadog() {
        let transport = FakeTransport::answering([Answer::status(
            503,
            r#"{"error":"upstream worker unavailable"}"#,
        )
        .with_header(
            "traceparent",
            "00-0000000000000000199f1a0de21b4536-00f067aa0ba902b7-01",
        )]);
        let outcome = run(
            &[DOMAIN],
            &[
                "http",
                "request",
                "get",
                "https://api.contoso.example/orders",
            ],
            Setup::fake(transport),
        );
        assert_eq!(outcome.code, 1, "{outcome:?}");
        assert_eq!(outcome.json()["trace_id"], "1846223019175527734");
        assert!(
            outcome
                .stderr
                .contains("agent-cli dd span list --trace 1846223019175527734"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn a_bearer_from_the_environment_signs_the_call_and_never_prints() {
        let transport = FakeTransport::answering([Answer::json(&json!({"ok": true}))]);
        let outcome = run(
            &[DOMAIN],
            &[
                "http",
                "request",
                "get",
                "https://api.contoso.example/secure",
                "--bearer-env",
                "API_TOKEN",
            ],
            Setup::fake(transport.clone()).with_env("API_TOKEN", "t0ken"),
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            transport.sent()[0].authorization.as_deref(),
            Some("Bearer t0ken")
        );
        assert!(!outcome.stdout.contains("t0ken") && !outcome.stderr.contains("t0ken"));
        let unset = run(
            &[DOMAIN],
            &[
                "http",
                "request",
                "get",
                "https://api.contoso.example/secure",
                "--bearer-env",
                "API_TOKEN",
            ],
            Setup::fake(FakeTransport::default()),
        );
        assert_eq!(unset.code, 3, "{unset:?}");
    }

    #[test]
    fn a_redirect_to_another_origin_goes_without_the_sign_in() {
        let transport = FakeTransport::answering([
            Answer::status(302, "").with_header("Location", "https://files.contoso.example/o/1"),
            Answer::json(&json!({"id": 1})),
        ]);
        let outcome = run(
            &[DOMAIN],
            &[
                "http",
                "request",
                "get",
                "https://api.contoso.example/redirect",
                "--follow",
                "--bearer-env",
                "T",
                "--header-env",
                "X-Api-Key=K",
            ],
            Setup::fake(transport.clone())
                .with_env("T", "t0ken")
                .with_env("K", "k3y"),
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let sent = transport.sent();
        assert_eq!(sent[0].authorization.as_deref(), Some("Bearer t0ken"));
        assert_eq!(
            sent[0].secret_headers,
            [("X-Api-Key".to_owned(), "k3y".to_owned())]
        );
        assert_eq!(sent[1].url, "https://files.contoso.example/o/1");
        assert_eq!(sent[1].authorization, None);
        assert!(sent[1].secret_headers.is_empty());
        let row = outcome.json();
        assert_eq!(row["hops"][0]["unsigned"], true);
        assert_eq!(row["url"], "https://files.contoso.example/o/1");
    }

    #[test]
    fn an_unfollowed_redirect_names_the_next_request() {
        let transport =
            FakeTransport::answering(
                [Answer::status(302, "").with_header("Location", "/orders/1")],
            );
        let outcome = run(
            &[DOMAIN],
            &[
                "http",
                "request",
                "get",
                "https://api.contoso.example/redirect",
            ],
            Setup::fake(transport),
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert!(
            outcome.stderr.contains(
                "[next: agent-cli http request get https://api.contoso.example/orders/1]"
            ),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn a_long_list_is_cut_to_the_limit_and_a_secret_is_masked() {
        let orders: Vec<_> = (0..600).map(|id| json!({"id": id})).collect();
        let transport = FakeTransport::answering([
            Answer::json(&json!({"items": orders})),
            Answer::json(&json!({"access_token": "s3cret", "token_type": "Bearer"})),
        ]);
        let setup = Setup::fake(transport);
        let outcome = run(
            &[DOMAIN],
            &[
                "http",
                "request",
                "get",
                "https://api.contoso.example/orders",
                "--limit",
                "3",
            ],
            setup,
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["body"]["items"].as_array().unwrap().len(), 3);
        assert!(
            outcome.stderr.contains("items[3 of 600]"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn a_proxy_from_the_environment_is_named_on_the_answer() {
        let transport = FakeTransport::answering([Answer::json(&json!({}))]);
        let outcome = run(
            &[DOMAIN],
            &[
                "http",
                "request",
                "get",
                "https://api.contoso.example/healthz",
            ],
            Setup::fake(transport.clone())
                .with_env("HTTPS_PROXY", "http://proxy.contoso.example:3128"),
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            transport.sent()[0].proxy.as_deref(),
            Some("http://proxy.contoso.example:3128")
        );
        assert_eq!(
            outcome.json()["proxy"],
            "http://proxy.contoso.example:3128 (HTTPS_PROXY)"
        );
    }
}
