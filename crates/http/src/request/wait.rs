use std::time::{Duration, Instant};

use agent_cli_core::{Body, Ctx, Failure, Method, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::answer::Answer;
use crate::call::Call;

#[derive(clap::Args)]
pub struct RequestWaitArgs {
    /// The URL to GET until it answers as wanted
    url: String,
    /// The status to wait for (default: any 2xx)
    #[arg(long = "expect-status", value_name = "STATUS")]
    expect_status: Option<u16>,
    /// A JSON field's value to wait for, FIELD=VALUE (repeatable): version=1.4.2
    #[arg(long = "expect-field", value_name = "FIELD=VALUE")]
    expect_field: Vec<String>,
    /// Seconds between tries
    #[arg(long, default_value_t = 2)]
    every: u64,
    /// Items kept per JSON array
    #[arg(long, default_value_t = 50)]
    limit: usize,
    #[command(flatten)]
    call: Call,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Waited {
    tries: u32,
    waited_s: u64,
    #[serde(flatten)]
    answer: Answer,
}

fn request_wait(ctx: &Ctx, args: RequestWaitArgs) -> Result<Waited> {
    let wanted: Vec<(Vec<String>, String)> = args
        .expect_field
        .iter()
        .map(|pair| {
            pair.split_once('=')
                .map(|(field, value)| {
                    (
                        field.split('.').map(str::to_owned).collect(),
                        value.to_owned(),
                    )
                })
                .ok_or_else(|| {
                    Failure::usage(format!("--expect-field {pair:?} is not FIELD=VALUE"))
                        .hint("--expect-field version=1.4.2")
                        .into()
                })
        })
        .collect::<Result<_>>()?;
    let prepared = args.call.prepare(ctx)?;
    let url = prepared.url(&args.url)?;
    let started = Instant::now();
    let every = Duration::from_secs(args.every.max(1));
    let mut tries = 0;
    let mut last: Option<String> = None;
    loop {
        tries += 1;
        match prepared.send(ctx, Method::Get, &url, Body::None, false) {
            Ok(exchange) => {
                let response = &exchange.response;
                let status_ok = match args.expect_status {
                    Some(status) => response.status == status,
                    None => (200..300).contains(&response.status),
                };
                let body: Value = serde_json::from_str(&response.body).unwrap_or(Value::Null);
                let unmet: Vec<String> = wanted
                    .iter()
                    .filter(|(path, value)| field(&body, path).as_deref() != Some(value.as_str()))
                    .map(|(path, value)| {
                        let held = field(&body, path).unwrap_or_else(|| "nothing".to_owned());
                        format!("{} is {held}, not {value}", path.join("."))
                    })
                    .collect();
                if status_ok && unmet.is_empty() {
                    let answer = Answer::of(ctx, exchange, args.limit, true)?;
                    return Ok(Waited {
                        tries,
                        waited_s: started.elapsed().as_secs(),
                        answer,
                    });
                }
                let mut why = vec![format!("answered {}", response.status)];
                why.extend(unmet);
                last = Some(why.join(", "));
                if ctx.remaining()? <= every {
                    let answer = Answer::of(ctx, exchange, args.limit, false)?;
                    return Err(gave_up(&url, started, tries, last.as_deref())
                        .with_data(Waited {
                            tries,
                            waited_s: started.elapsed().as_secs(),
                            answer,
                        })
                        .into());
                }
            }
            // A deploy refuses connections for a while: keep trying.
            Err(error) => {
                if ctx.remaining().is_err() {
                    return Err(gave_up(&url, started, tries, last.as_deref()).into());
                }
                last = Some(format!("{error:#}"));
            }
        }
        let left = ctx
            .remaining()
            .map_err(|_| gave_up(&url, started, tries, last.as_deref()))?;
        if left <= every {
            return Err(gave_up(&url, started, tries, last.as_deref()).into());
        }
        std::thread::sleep(every);
    }
}

fn gave_up(url: &str, started: Instant, tries: u32, last: Option<&str>) -> Failure {
    Failure::timed_out(format!(
        "{url} did not answer as wanted in {}s ({tries} tries); last: {}",
        started.elapsed().as_secs(),
        last.unwrap_or("nothing")
    ))
    .hint("run it again with a larger --timeout, or check what the last answer says")
}

/// A field of `body` by its dotted path (`items.0.status`), as text.
fn field(body: &Value, path: &[String]) -> Option<String> {
    let mut at = body;
    for segment in path {
        at = match at {
            Value::Object(map) => map.get(segment)?,
            Value::Array(items) => items.get(segment.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(match at {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    })
}

command! {
    pub REQUEST_WAIT = ["http", "request", "wait"], Read,
    "Poll a URL until it answers the status and the JSON fields wanted",
    keywords: ["poll", "until", "healthz", "returns", "url", "http", "up", "yet", "checking", "available"],
    example: "http request wait https://api.contoso.example/healthz --expect-field version=1.4.2",
    timeout: 100,
    run: request_wait,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::Setup;
    use agent_cli_core::testing::{Answer, FakeTransport, run};
    use serde_json::json;

    use crate::DOMAIN;

    #[test]
    fn it_tries_until_the_field_matches() {
        let transport = FakeTransport::answering([
            Answer::status(503, "{}"),
            Answer::json(&json!({"status": "ok", "version": "1.4.1"})),
            Answer::json(&json!({"status": "ok", "version": "1.4.2"})),
        ]);
        let outcome = run(
            &[DOMAIN],
            &[
                "http",
                "request",
                "wait",
                "https://api.contoso.example/healthz",
                "--expect-field",
                "version=1.4.2",
                "--every",
                "1",
            ],
            Setup::fake(transport),
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["tries"], 3);
        assert_eq!(outcome.json()["body"]["version"], "1.4.2");
    }

    #[test]
    fn it_gives_up_at_the_deadline_with_the_last_answer() {
        let answers = (0..5).map(|_| Answer::json(&json!({"version": "1.4.1"})));
        let outcome = run(
            &[DOMAIN],
            &[
                "http",
                "request",
                "wait",
                "https://api.contoso.example/healthz",
                "--expect-field",
                "version=1.4.2",
                "--every",
                "1",
                "--timeout",
                "2",
            ],
            Setup::fake(FakeTransport::answering(answers)),
        );
        assert_eq!(outcome.code, 124, "{outcome:?}");
        assert!(
            outcome.stderr.contains("version is 1.4.1, not 1.4.2"),
            "{}",
            outcome.stderr
        );
        assert_eq!(outcome.json()["body"]["version"], "1.4.1");
    }
}
