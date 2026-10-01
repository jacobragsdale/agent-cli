//! What the log, span and event searches share: log statuses, one page's
//! bound, the notes on a slow window and on more matches, and the ids,
//! counts and durations Datadog writes more than one way.

use agent_cli_core::{Ctx, Failure};
use anyhow::Result;
use serde_json::Value;

use crate::client::{Window, tag, text};

/// A log's status, as Datadog's search syntax spells it.
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum LogStatus {
    Emergency,
    Alert,
    Critical,
    Error,
    Warn,
    Notice,
    Info,
    Debug,
    Ok,
}

/// `values` by their command-line names, for a search term.
pub(crate) fn names<T: clap::ValueEnum>(values: &[T]) -> Vec<String> {
    values
        .iter()
        .filter_map(|value| Some(value.to_possible_value()?.get_name().to_owned()))
        .collect()
}

/// Longer messages are cut, with the count of what was left out.
pub(crate) const MESSAGE_MAX: usize = 300;
/// One page of the logs and spans search APIs.
pub(crate) const PAGE_MAX: usize = 1000;
/// Searches over a longer window are slow; counting is not.
const SLOW_WINDOW: i64 = 86_400;

/// A trace id Datadog wrote as a string or a number.
pub(crate) fn id_text(value: &Value) -> Option<String> {
    match value {
        Value::Number(number) => Some(number.to_string()),
        other => text(other),
    }
}

/// `--limit` fits one page; anything more is a count's job.
pub(crate) fn check_page(limit: usize, what: &str) -> Result<()> {
    if limit > PAGE_MAX {
        return Err(
            Failure::usage(format!("--limit is at most {PAGE_MAX}, one page of {what}"))
                .hint(if what == "logs" {
                    "count every match with agent-cli dd log-count list --by service"
                } else {
                    "count every span with agent-cli dd service get SERVICE"
                })
                .into(),
        );
    }
    Ok(())
}

pub(crate) fn slow_window(ctx: &Ctx, window: Window) {
    if window.seconds() > SLOW_WINDOW {
        ctx.note(
            "[a window over 1d is slow to search; counting is fast: agent-cli dd log-count list]",
        );
    }
}

/// The logs and spans APIs return a cursor, not a total.
pub(crate) fn more_match(ctx: &Ctx, found: &Value, shown: usize, limit: usize, count: &str) {
    if shown >= limit && !found["meta"]["page"]["after"].is_null() {
        ctx.note(format!(
            "[{shown} shown, more match; narrow QUERY, or count them with agent-cli {count}]"
        ));
    }
}

/// A count Datadog wrote as an integer or a float.
pub(crate) fn count(value: &Value) -> u64 {
    value
        .as_u64()
        .or_else(|| value.as_f64().map(|count| count.max(0.0).round() as u64))
        .unwrap_or(0)
}

/// Nanoseconds, as Datadog measures a span, in milliseconds to 0.1.
pub(crate) fn millis(nanos: &Value) -> Option<f64> {
    nanos
        .as_f64()
        .map(|nanos| (nanos / 100_000.0).round() / 10.0)
}

/// Frames in these paths are a library's or the runtime's, not the service's.
const LIBRARIES: [&str; 6] = [
    "site-packages",
    "dist-packages",
    "node_modules",
    "<frozen",
    "/usr/lib",
    "node:internal",
];

/// Where an error's `error.stack` points in the service's own code: the
/// innermost frame with a path that is not a library's, `PATH:LINE` as the
/// stack printed it. With the source code integration's tags naming an Azure
/// DevOps repository and commit, `PROJECT/REPO@SHA:PATH:LINE`, which `ado
/// file get` takes and resolves to the repository's path.
pub(crate) fn at(stack: &Value, tags: &Value) -> Option<String> {
    let stack = stack.as_str()?;
    let mut frames: Vec<(&str, &str)> = stack.lines().filter_map(frame).collect();
    // Python prints the innermost frame last; .NET and JavaScript first.
    if stack.lines().any(|line| line.trim().starts_with("File \"")) {
        frames.reverse();
    }
    let (path, line) = frames
        .into_iter()
        .find(|(path, _)| !LIBRARIES.iter().any(|library| path.contains(library)))?;
    let repo = tag(tags, "git.repository_url")
        .and_then(ado_repo)
        .zip(tag(tags, "git.commit.sha"));
    Some(match repo {
        Some((repo, sha)) => format!("{repo}@{sha}:{path}:{line}"),
        None => format!("{path}:{line}"),
    })
}

/// One stack line's path and line: .NET `at M() in PATH:line N`, Python
/// `File "PATH", line N, in f`, JavaScript `at f (PATH:L:C)`.
fn frame(line: &str) -> Option<(&str, &str)> {
    let line = line.trim();
    let digits = |number: &str| !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit());
    if let Some(rest) = line.strip_prefix("File \"") {
        let (path, rest) = rest.split_once("\", line ")?;
        let number = rest.split(',').next()?.trim();
        return digits(number).then_some((path, number));
    }
    let call = line.strip_prefix("at ")?;
    if let Some((_, place)) = call.rsplit_once(" in ") {
        let (path, number) = place.rsplit_once(":line ")?;
        return digits(number.trim()).then_some((path, number.trim()));
    }
    let place = call.strip_suffix(')')?.rsplit_once('(')?.1;
    let (place, column) = place.rsplit_once(':')?;
    let (path, number) = place.rsplit_once(':')?;
    (digits(column) && digits(number) && !path.is_empty()).then_some((path, number))
}

/// `PROJECT/REPO` of an Azure DevOps clone URL:
/// `https://dev.azure.com/ORG/PROJECT/_git/REPO`,
/// `https://ORG.visualstudio.com/PROJECT/_git/REPO` or
/// `git@ssh.dev.azure.com:v3/ORG/PROJECT/REPO`. Another host's is none.
fn ado_repo(url: &str) -> Option<String> {
    if let Some((_, path)) = url.split_once("ssh.dev.azure.com:v3/") {
        let mut parts = path.split('/').skip(1);
        let (project, repo) = (parts.next()?, parts.next()?);
        return Some(format!("{project}/{repo}"));
    }
    let rest = url.strip_prefix("https://")?;
    let (host, path) = rest.split_once('/')?;
    let host = host.rsplit('@').next()?;
    if host != "dev.azure.com" && !host.ends_with(".visualstudio.com") {
        return None;
    }
    let parts: Vec<&str> = path.split('/').collect();
    let git = parts
        .iter()
        .position(|part| *part == "_git")
        .filter(|at| *at > 0)?;
    let repo = parts.get(git + 1).filter(|repo| !repo.is_empty())?;
    Some(format!("{}/{repo}", parts[git - 1]))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::at;

    #[test]
    fn at_is_the_innermost_frame_in_the_services_code_in_each_runtime() {
        let dotnet = "System.Net.Http.HttpRequestException: Connection refused\n   at System.Net.Http.HttpConnectionPool.ConnectAsync(HttpRequestMessage request)\n   at Contoso.Api.Orders.OrderClient.GetAsync(Int32 id, CancellationToken cancel) in /app/src/Orders/OrderClient.cs:line 19\n   at Contoso.Api.Orders.OrdersController.Post(Order order) in /app/src/Orders/OrdersController.cs:line 31";
        let python = "Traceback (most recent call last):\n  File \"/app/jobs/sync.py\", line 12, in main\n    run()\n  File \"/app/jobs/client.py\", line 40, in run\n    requests.get(url)\n  File \"/usr/local/lib/python3.12/site-packages/requests/api.py\", line 73, in get\nConnectionError: refused";
        let js = "Error: refused\n    at TCPConnectWrap.afterConnect (node:internal/net:1555:16)\n    at Client.connect (/app/node_modules/pg/lib/client.js:132:7)\n    at loadOrders (/app/src/orders.js:27:11)\n    at main (/app/src/index.js:8:3)";
        let none = json!([]);
        assert_eq!(
            at(&json!(dotnet), &none).as_deref(),
            Some("/app/src/Orders/OrderClient.cs:19")
        );
        assert_eq!(
            at(&json!(python), &none).as_deref(),
            Some("/app/jobs/client.py:40")
        );
        assert_eq!(
            at(&json!(js), &none).as_deref(),
            Some("/app/src/orders.js:27")
        );
        assert_eq!(at(&json!("boom"), &none), None, "no frame, no at");
        assert_eq!(at(&json!(null), &none), None);
    }

    #[test]
    fn an_azure_devops_repository_and_commit_name_the_file_at_that_commit() {
        let stack = json!(
            "   at Contoso.Api.Orders.OrderClient.GetAsync() in /app/src/Orders/OrderClient.cs:line 19"
        );
        for url in [
            "https://dev.azure.com/contoso/Fabrikam/_git/api",
            "https://contoso@dev.azure.com/contoso/Fabrikam/_git/api",
            "https://contoso.visualstudio.com/Fabrikam/_git/api",
            "git@ssh.dev.azure.com:v3/contoso/Fabrikam/api",
        ] {
            let tags = json!([
                "env:prod",
                format!("git.repository_url:{url}"),
                "git.commit.sha:4be1c0d2"
            ]);
            assert_eq!(
                at(&stack, &tags).as_deref(),
                Some("Fabrikam/api@4be1c0d2:/app/src/Orders/OrderClient.cs:19"),
                "{url}"
            );
        }
        let github = json!([
            "git.repository_url:https://github.com/contoso/api",
            "git.commit.sha:4be1c0d2"
        ]);
        assert_eq!(
            at(&stack, &github).as_deref(),
            Some("/app/src/Orders/OrderClient.cs:19"),
            "another host's repository is only the path"
        );
        let no_sha = json!(["git.repository_url:https://dev.azure.com/contoso/Fabrikam/_git/api"]);
        assert_eq!(
            at(&stack, &no_sha).as_deref(),
            Some("/app/src/Orders/OrderClient.cs:19")
        );
    }
}
