# Trace a deploy and a failed DAG with agent-cli

In this tutorial, we will answer two questions an on-call engineer asks every
week, the way an agent would, using only agent-cli: which work items shipped
in the api image running in production, and why last night's Airflow run
failed. We run against the contoso world, a recorded company whose Azure
DevOps, Kubernetes, Airflow and Datadog all describe the same facts, so no
cloud account is needed.

Before you start, you need a clone of this repository, Rust 1.88 or later, a C
compiler, and network access for the first build to fetch crates. Run every
command from the repository's root directory, in a POSIX shell such as bash
or zsh.

## Build the recorded-world binary

First, build agent-cli with the `fixtures` feature, which compiles in the
replayer that answers requests from recordings instead of the network:

```sh
cargo build --features fixtures -p agent-cli
```

Now point your shell at the contoso world:

```sh
eval "$(scripts/trial-env.sh)"
```

It prints nothing. The script puts the fixtures build and the repository's
stand-ins for `az` and `kubectl` first on your `PATH`, selects the world's
config, and freezes the clock at the moment the world was recorded. The
settings last only for this shell: in a new terminal, run it again.

## Read the overview

Run agent-cli with no arguments:

```sh
agent-cli
```

You should see:

```text
agent-cli: 8 services for coding agents — 93 commands. Output: JSON.
Start here:  agent-cli search <what you want to do>    e.g. agent-cli search "list work items"
Browse:      agent-cli <domain> [<resource>]    Details: agent-cli <domain> <resource> <verb> --help
Flags:       --fields a,b.c  --raw  --dry-run  --yes  --reveal  --timeout S (60)  --output FILE  --no-cache
Exit:        0 ok · 1 failed · 2 fix the call · 3 needs setup (run doctor) · 4 not found · 5 conflict · 124 timed out
Config:      ado contoso/Fabrikam · kv 1 vault · acr 1 registry · k8s 1 scope · sql not set up · airflow 1 instance · dd eu    Live check: agent-cli doctor
Now:         2026-09-29T12:00Z
Domains:     ado(29)  kv(4)  acr(4)  aks(2)  k8s(13)  sql(6)  airflow(15)  dd(20)
```

Notice the `Now:` line: `2026-09-29T12:00Z` is the world's frozen clock. If
it shows the current time instead, the binary on your `PATH` was built
without the feature (a `cargo test` rebuilds it that way): run the build
command again. `sql not set up` is expected; the world has no database.

## Find the command

We want to know which image production runs. Ask search in plain words:

```sh
agent-cli search "which image is running in prod"
```

You should see:

```text
agent-cli k8s deployment list  # List deployments: ready pods, images with tag and digest, when they rolled out
agent-cli dd container list  # List containers Datadog sees, with state, image and the k8s pod id
agent-cli acr manifest get <image>  # Describe one image by tag or digest: size, architecture, os, tags on it
agent-cli acr repo list  # List container image repositories with tag counts and last push
agent-cli acr tag list <repo>  # List an image repository's tags, newest first, with their digests
agent-cli aks cluster list  # List the AKS clusters the az login reaches, with version and power state
(6 of 12; --limit N for more. Details: agent-cli <domain> <resource> <verb> --help)
```

The first hit is the one we want. Each line is a call to fill in, with its
required arguments.

## Read the help

```sh
agent-cli k8s deployment list --help
```

You should see:

```text
agent-cli k8s deployment list — List deployments: ready pods, images with tag and digest, when they rolled out
  <name> str       Part of the deployment name
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
  --since time     Only deployments that rolled out after this
  --limit int      (default 50)
A time is 15m, 2h, 7d, 1w (ago), now-15m, 2026-09-29, or RFC 3339.
Returns: [{id,name,namespace,ready,replicas,images[{container,image,digest}],updated}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s deployment list --cluster prod --namespace web --fields id,ready,images,updated
```

Notice the `Returns:` line: it is the exact shape of the answer, so we can ask
for only the fields we need. `--cluster` and `--namespace` default to the only
ones configured, so we can leave them out.

## Ask for only what you need

```sh
agent-cli k8s deployment list --fields id,images
```

You should see:

```text
[{"id":"prod/web/api","images":[{"container":"api","image":"contosoacr.azurecr.io/api:v1.4.2","digest":"sha256:7a72fa7948c3b5e299c40bdb1ebeb125e313c32d4262e130a8206115054663d3"}]},{"id":"prod/web/worker","images":[{"container":"worker","image":"contosoacr.azurecr.io/worker:v1.4.2","digest":"sha256:0bdbdea4bfb8c2015fac6e1b5d750e6ed21374b592b8ed275cb1650230c8c7ff"}]}]
```

The api deployment runs `api:v1.4.2`. In this world images are tagged with the
git tag that built them, so `v1.4.2` is also the tag the build ran on.

## Make a mistake on purpose

An agent often puts the id where the verb goes. Try it:

```shell
agent-cli ado run 8812
```

You should see this on stderr, and `echo $?` prints the exit code, 2:

```text
error: unknown verb "8812" in ado run; an id goes after the verb
hint: closest commands:
  agent-cli ado run get 8812  # Show a run: status, commit, timing, what failed, its pull request and work items
  agent-cli ado run logs <id>  # Print the tail of a run's logs: failed tasks by default, or a job or task
  agent-cli ado run list  # List pipeline runs (builds), newest first
  agent-cli ado run retry <id>  # Retry the failed jobs of a finished run (write)
  agent-cli ado run cancel <id>  # Cancel a run that is queued or in progress (destructive)
```

Exit code 2 means "fix the call", and the first suggested command is the
fixed call.

## Trace the deploy to its work items

Find the builds that ran on the tag:

```sh
agent-cli ado run list --branch refs/tags/v1.4.2 --fields id,result,finished
```

You should see:

```text
[{"id":8812,"result":"succeeded","finished":"2026-09-28T21:03:11Z"},{"id":8809,"result":"failed","finished":"2026-09-28T20:44:02Z"}]
```

Runs are listed newest first: 8809, the first build on the tag, failed, and
its re-run, 8812, succeeded. Every `id` is exactly what the matching `get`
takes, so ask the successful run what produced it:

```sh
agent-cli ado run get 8812 --fields commit,pr,workitems
```

You should see:

```text
{"commit":"4be1c0d2e8f1a9b3c5d7e9f1a2b3c4d5e6f7a8b9","pr":{"id":431,"title":"Retry on 429 from the orders service","status":"completed"},"workitems":[{"id":1207,"type":"User Story","title":"Retry orders-service calls on 429","state":"Resolved"},{"id":1210,"type":"Task","title":"Log the retry count on each attempt","state":"Resolved"}]}
```

Work items 1207 and 1210 shipped in production's api image, through pull
request 431: three calls from the cluster to the backlog.

## Trace the failed DAG to its exception

Last night's `etl_nightly` run failed. Find it:

```sh
agent-cli airflow run list --dag etl_nightly --state failed --since 1d --fields id
```

You should see:

```text
[{"id":"etl_nightly/scheduled__2026-09-29T00:00:00+00:00"}]
```

Ask the run which task failed. In any Airflow id, `latest` stands for the
DAG's most recent run:

```sh
agent-cli airflow run get etl_nightly/latest --fields failed
```

You should see:

```text
{"failed":["etl_nightly/scheduled__2026-09-29T00:00:00+00:00/load_orders/2"]}
```

The failed task instance's id is `dag/run/task/try`: `load_orders` failed on
its second try. Read that try's exception:

```sh
agent-cli airflow task logs etl_nightly/latest/load_orders/2 --tail 20 --fields error
```

You should see:

```text
{"error":"ValueError: order 88123 has no customer_id (at /opt/airflow/dags/etl_nightly.py:42 in load_orders)"}
```

That is the answer: order 88123 has no customer, at line 42 of the DAG file.
This Airflow runs each task as a Kubernetes pod, and a failed task's pod
stays in the cluster. Ask the task, without a try number, for its pod:

```sh
agent-cli airflow task get etl_nightly/latest/load_orders --fields pod
```

You should see:

```text
{"pod":"prod/web/etl-nightly-load-orders-q8x1k2vz"}
```

The pod's id is what `k8s pod logs` takes, so the hop from Airflow to
Kubernetes is one copy:

```sh
agent-cli k8s pod logs prod/web/etl-nightly-load-orders-q8x1k2vz --tail 2 --fields text
```

You should see:

```text
{"text":"    raise ValueError(f\"order {order_id} has no customer_id\")\nValueError: order 88123 has no customer_id\n"}
```

## Next steps

You have traced a production image back to the work items it shipped, and a
failed nightly run to the line that raised, in eight data calls and with no
cloud account. To add a command of your own, see
[How to add a command](how-to/add-a-command.md). To watch agents do what you
did here, see [How to run agent trials](how-to/run-agent-trials.md). To
understand why ids, `--fields` and hints look the way they do, read
[The design of agent-cli](explanation/design.md). Every command is listed in
the [command reference](reference/commands.md).
