use agent_cli_core::{Ctx, Effect, Exit, Failure, command, redact};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::kubectl::{At, finished};

use super::running;

/// What kubectl adds to stderr when the command in the pod exits nonzero.
const TERMINATED: &str = "command terminated with exit code";

#[derive(clap::Args)]
pub struct PodRunArgs {
    /// The pod: its id (cluster/namespace/name), namespace/name, or name
    pod: String,
    #[command(flatten)]
    at: At,
    /// Which container; defaults to the pod's only one
    #[arg(long)]
    container: Option<String>,
    /// How many of the last lines of stdout, and of stderr, to keep
    #[arg(long, default_value_t = 200)]
    tail: usize,
    /// The command after `--`: nslookup HOST, env, cat /etc/resolv.conf, nc -zv HOST PORT
    #[arg(last = true, required = true)]
    argv: Vec<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Ran {
    /// The pod's id: what `pod get` and `pod logs` take.
    id: String,
    container: String,
    command: String,
    exit_code: i32,
    /// The last `--tail` lines, values named like secrets masked.
    stdout: String,
    stderr: String,
}

fn pod_run(ctx: &Ctx, args: PodRunArgs) -> Result<Ran> {
    let (target, pod) = args.at.named(ctx, &args.pod)?;
    // Read first: a pod that is not there is exit 4, and the container
    // defaults to its only one.
    let item = target.json(ctx, &["get", "pod", &pod, "-o", "json"])?;
    let id = target.id(&item);
    let names: Vec<&str> = running(&item["spec"]).map(|(name, _)| name).collect();
    let container = match (args.container, names.as_slice()) {
        (Some(wanted), _) if names.contains(&wanted.as_str()) => wanted,
        (None, [only]) => (*only).to_owned(),
        (wanted, _) => {
            let what = wanted.map_or_else(
                || format!("{id} runs {} containers", names.len()),
                |wanted| format!("{id} has no container {wanted}"),
            );
            return Err(Failure::usage(format!(
                "{what}; --container takes one of: {}",
                names.join(", ")
            ))
            .into());
        }
    };
    let mut argv = vec!["exec", pod.as_str(), "-c", container.as_str(), "--"];
    argv.extend(args.argv.iter().map(String::as_str));
    // VERIFY(work): whether --request-timeout=10s cuts an exec that runs
    // longer; the command's deadline bounds it either way.
    let output = ctx.write(Effect::Destructive, target.kubectl(&argv))?;
    // kubectl's own failure (no such container, a pod not running) has no
    // exit code from the command: it reads as every other kubectl error.
    if !output.status.success() && !output.stderr.contains(TERMINATED) {
        return Err(finished(output).expect_err("finished fails a failed kubectl"));
    }
    let stderr: Vec<&str> = output
        .stderr
        .lines()
        .filter(|line| !line.starts_with(TERMINATED))
        .collect();
    let ran = Ran {
        id,
        container,
        command: args.argv.join(" "),
        exit_code: output.status.code().unwrap_or(1),
        stdout: last(ctx, "stdout", &output.stdout, args.tail),
        stderr: last(ctx, "stderr", &stderr.join("\n"), args.tail),
    };
    if ran.exit_code != 0 {
        let message = format!("`{}` exited {} in {}", ran.command, ran.exit_code, ran.id);
        return Err(Failure::new(Exit::Failed, message).with_data(&ran).into());
    }
    Ok(ran)
}

/// The last `tail` lines of `text`, redacted, with a note when it cut.
fn last(ctx: &Ctx, what: &str, text: &str, tail: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() > tail {
        ctx.note(format!(
            "[last {tail} of {} lines of {what}; --tail N]",
            lines.len()
        ));
    }
    redact(&lines[lines.len().saturating_sub(tail)..].join("\n"))
}

command! {
    pub POD_RUN = ["k8s", "pod", "run"], Destructive,
    "Run a command in a pod's container, from where the app runs (kubectl exec)",
    keywords: ["exec", "shell", "nslookup", "dns", "resolve", "env", "environment", "printenv", "resolv.conf", "nc", "connectivity", "reach", "inside", "curl"],
    example: "k8s pod run prod/web/api-7d9f8c6b5-x2k4q --yes -- nslookup kv-contoso-prod.vault.azure.net",
    run: pod_run,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::{dry_run, k8s};

    #[test]
    fn the_command_runs_in_the_only_container_with_the_namespace_before_the_dashes() {
        let plans = dry_run(&[
            "k8s",
            "pod",
            "run",
            "qa/dev/orders-api-7d9f5b-abc12",
            "--",
            "nslookup",
            "-n",
            "x",
        ]);
        assert_eq!(
            plans[0]["run"],
            json!([
                "kubectl",
                "--context",
                "aks-qa",
                "--request-timeout=10s",
                "exec",
                "orders-api-7d9f5b-abc12",
                "-c",
                "api",
                "-n",
                "dev",
                "--",
                "nslookup",
                "-n",
                "x"
            ])
        );

        let outcome = k8s(&[
            "k8s",
            "pod",
            "run",
            "qa/dev/orders-api-7d9f5b-abc12",
            "--",
            "env",
        ]);
        assert_eq!(outcome.code, 2, "an exec needs --yes: {outcome:?}");

        let outcome = k8s(&[
            "k8s",
            "pod",
            "run",
            "qa/dev/orders-api-7d9f5b-abc12",
            "--yes",
            "--tail",
            "2",
            "--",
            "env",
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"id": "qa/dev/orders-api-7d9f5b-abc12", "container": "api", "command": "env",
                   "exit_code": 0, "stdout": "DB_PASSWORD=***\nLOG_LEVEL=info"})
        );
        assert!(
            outcome
                .stderr
                .contains("[last 2 of 3 lines of stdout; --tail N]"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn a_failing_command_prints_its_row_and_exits_1() {
        let outcome = k8s(&[
            "k8s",
            "pod",
            "run",
            "qa/dev/orders-api-7d9f5b-abc12",
            "--yes",
            "--",
            "nc",
            "-zv",
            "orders-db",
            "5432",
        ]);
        assert_eq!(outcome.code, 1, "{outcome:?}");
        assert_eq!(outcome.json()["exit_code"], 1);
        assert_eq!(
            outcome.json()["stderr"],
            "nc: orders-db (10.0.12.7:5432): Connection timed out"
        );
        assert!(
            outcome.stderr.contains("`nc -zv orders-db 5432` exited 1"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn a_pod_with_two_containers_needs_one_named() {
        let pod = "qa/dev/billing-api-1a2b3c-qq111";
        let outcome = k8s(&["k8s", "pod", "run", pod, "--yes", "--", "env"]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome.stderr.contains(
                "qa/dev/billing-api-1a2b3c-qq111 runs 2 containers; --container takes one of: api, proxy"
            ),
            "{}",
            outcome.stderr
        );
        let outcome = k8s(&[
            "k8s",
            "pod",
            "run",
            pod,
            "--container",
            "sidecar",
            "--yes",
            "--",
            "env",
        ]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
        let outcome = k8s(&["k8s", "pod", "run", "qa/dev/nope", "--yes", "--", "env"]);
        assert_eq!(outcome.code, 4, "{outcome:?}");
    }
}
