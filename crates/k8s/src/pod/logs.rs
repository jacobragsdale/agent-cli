use agent_cli_core::{Ctx, Failure, When, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::kubectl::At;

#[derive(clap::Args)]
pub struct PodLogsArgs {
    /// The pod: its id (cluster/namespace/name), namespace/name, or name
    pod: String,
    #[command(flatten)]
    at: At,
    /// Which container; kubectl picks the default one when left out
    #[arg(long)]
    container: Option<String>,
    /// How many of the last lines
    #[arg(long, default_value_t = 200)]
    tail: usize,
    /// The run before the last restart: where a crash loop says why
    #[arg(long)]
    previous: bool,
    /// Only lines logged after this
    #[arg(long)]
    since: Option<When>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Logs {
    /// The pod's id.
    pod: String,
    namespace: String,
    container: Option<String>,
    lines: usize,
    /// The log itself; cut from the front when it is long.
    text: String,
}

fn pod_logs(ctx: &Ctx, args: PodLogsArgs) -> Result<Logs> {
    let (target, pod) = args.at.named(ctx, &args.pod)?;
    let tail = format!("--tail={}", args.tail);
    let mut argv = vec!["logs", pod.as_str(), tail.as_str()];
    if let Some(container) = &args.container {
        argv.extend(["-c", container]);
    }
    if args.previous {
        argv.push("-p");
    }
    let since = args
        .since
        .map(|since| format!("--since-time={}", since.utc()));
    if let Some(since) = &since {
        argv.push(since);
    }
    let id = format!(
        "{}/{}/{pod}",
        target.scope,
        target.namespace.as_deref().unwrap_or_default()
    );
    // Two ways a container has no log yet, each with its own next step.
    let text = target.read(ctx, &argv).map_err(|error| {
        let said = format!("{error:#}");
        let hint = if said.contains("previous terminated container") {
            format!(
                "it has not restarted, so there is no previous run: agent-cli k8s pod logs {id}"
            )
        } else if said.contains("is waiting to start") {
            format!("agent-cli k8s pod get {id}  (what it waits on)")
        } else {
            return error;
        };
        match error.downcast::<Failure>() {
            Ok(failure) => failure.hint(hint).into(),
            Err(error) => error,
        }
    })?;
    let namespace = target.namespace.unwrap_or_default();
    Ok(Logs {
        pod: format!("{}/{namespace}/{pod}", target.scope),
        namespace,
        container: args.container,
        lines: text.lines().count(),
        text,
    })
}

command! {
    pub POD_LOGS = ["k8s", "pod", "logs"], Read,
    "Read the tail of a pod's log, or of the container's previous run",
    keywords: ["log", "output", "stdout", "stderr", "tail", "crash", "error", "previous", "container"],
    example: "k8s pod logs qa/dev/orders-worker-5c4d3e-q8zt --previous --tail 50",
    run: pod_logs,
}

#[cfg(test)]
mod tests {

    use crate::testing::run;

    #[test]
    fn logs_are_bounded_by_tail_and_can_read_the_previous_run() {
        let outcome = run(&[
            "k8s",
            "pod",
            "logs",
            "orders-worker-5c4d3e-q8zt",
            "--tail",
            "3",
            "--previous",
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let logs = outcome.json();
        assert_eq!(logs["lines"], 3);
        let text = logs["text"].as_str().unwrap();
        assert!(text.ends_with("api previous line 30\n"), "{text}");
        assert!(text.starts_with("2026-09-12T12:00:28Z ERROR"), "{text}");

        let outcome = run(&[
            "k8s",
            "pod",
            "logs",
            "billing-api-1a2b3c-qq111",
            "--container",
            "proxy",
            "--tail",
            "1",
        ]);
        assert!(
            outcome.json()["text"]
                .as_str()
                .unwrap()
                .contains(" proxy line 30"),
            "{outcome:?}"
        );
    }
}
