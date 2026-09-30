# The contoso world

A small, consistent, entirely synthetic company that agent trials run against
when no real Azure DevOps, Azure, cluster, Airflow or Datadog is reachable.
Every domain's recordings describe the same facts, so a cross-domain question
("which work items shipped in the api image running in prod?") can be
answered end to end.

## Run it

```sh
cargo build --features fixtures -p agent-cli     # the replayer is compiled in only with this feature
eval "$(scripts/trial-env.sh)"                   # prints the env; pass another world's directory to use that
agent-cli k8s deployment list
```

`scripts/trial-env.sh` sets:

| Variable | Why |
|---|---|
| `AGENT_CLI_FIXTURES` | This directory. The fixtures build answers every HTTP request from `http/*.json` and turns the cache off; the fake kubectl serves `kubectl.json` |
| `AGENT_CLI_CONFIG` | `config.toml` here: `[ado]`, `[azure]`, `[[k8s.scope]]` |
| `AGENT_CLI_NOW` | `2026-09-29T12:00:00Z`, the moment the world was recorded. Relative times (`--since 1d`, `--expires-within 30d`, ages) resolve against it, so they keep matching the recordings on any day |
| `PATH` | `target/debug` (the fixtures build) and `scripts/fake` (`az`, `kubectl`, `kubelogin`) first. The fake `az` hands out a stand-in token; nothing checks it |

A release build ignores `AGENT_CLI_FIXTURES`: the feature is not in it.

## What is true here

| Domain | Facts |
|---|---|
| ado | Org `contoso`, project `Fabrikam`, team `Fabrikam Team`, repo `api`. You are Jane Doe (`@me`). |
| | PR **431** "Retry on 429 from the orders service" (reviewers Sam Lee and Priya Patel, both approved) merged commit `4be1c0d2…` into `main`; it closes work items **1207** and **1210**. |
| | Git tag `v1.4.2` on that commit triggered `api-ci` run **8809**, which **failed** in "Run tests" (`OrdersClientTests.RetriesOn429` timed out; `ado run logs 8809`); the re-run **8812** succeeded and pushed the image. |
| | Assigned to `@me` in Sprint 42: **1218** (Bug, New: the worker crash loop), **1215** (Task, Active: retry jitter, PR 436 open), **1207** (Resolved). |
| | A pending approval: `db-migrations` run 8811, "Apply migration 0042 to prod". |
| acr | Registry `contosoacr` (`contosoacr.azurecr.io`): `api` tags `v1.4.2`, `v1.4.1`, `v1.4.0`; `worker` tags `v1.4.2`, `v1.4.1`. The `v1.4.2` digests match what the pods run. |
| k8s | Scope `prod` (context `aks-contoso-prod`, namespace `web`). Deployment `api` runs `contosoacr.azurecr.io/api:v1.4.2`, 3/3 ready. Deployment `worker` (`worker:v1.4.2`) is 0/1: pod `worker-5c4d3e9f1-q8zt1` is in CrashLoopBackOff with 23 restarts, BackOff events, and a previous log saying the database password was refused. Configmap `api-config`; secrets `api-env`, `worker-db`; SecretProviderClasses `api-kv` and `worker-kv`. |
| kv | Vault `kv-contoso-prod`: `db-password` expires 2026-10-14 (within 30 days), `worker-db-password` expired 2026-09-20 (why the worker cannot log in), `api-key`, `orders-db-conn`. |
| aks | Cluster `aks-contoso-prod` (Running). |

The deploy trace, for example (see AGENTS.md):

```sh
agent-cli k8s deployment list --fields id,images            # api runs contosoacr.azurecr.io/api:v1.4.2
agent-cli ado run list --branch refs/tags/v1.4.2 --fields id,result
agent-cli ado run get 8812 --fields commit,pr,workitems     # PR 431, work items 1207 and 1210
```

## Files

```
config.toml       the config agent-cli reads
http/ado.json     Azure DevOps answers
http/azure.json   Resource Graph, Key Vault and ACR answers
kubectl.json      what the fake kubectl knows: objects per context, and logs
```

Each `http/*.json` is a list of exchanges; the replayer reads every file in
`http/`, in name order:

```json
{"request": "GET https://dev.azure.com/contoso/Fabrikam/_apis/build/builds/8812?api-version=7.1",
 "note": "optional, for readers",
 "answer": {"id": 8812}}
```

- `request` is `METHOD URL`. Query parameters match in any order.
- `body` (optional, for POSTs): the JSON or form fields that must have been
  sent, keys in any order. Leave it out and the exchange answers any body
  (the Resource Graph query, ACR's token posts). An exchange with a `body` is
  preferred over one without.
- `status` (default 200) and `headers` (an object) shape the answer; `answer`
  is JSON, `text` a body taken as it is.

`kubectl.json` is `{"contexts": {"<context>": [objects]}, "logs": {...}}`.
Objects are what `kubectl get -o json` prints (`kind`, `metadata.namespace`
and `metadata.name` are what the fake filters on). Logs are keyed
`<context>/<namespace>/<pod>/<container>`, with `/previous` for `--previous`.

## Extend it

1. Run the command against the world. A request with no recording fails with
   the one it wanted, the closest recording, and the body it sent:

   ```
   error: fixtures: no recorded answer for GET https://…/build/builds/8813?api-version=7.1; closest recorded: GET https://…/build/builds/8812?api-version=7.1
   ```

2. Add that exchange, with an answer in the shape the service really sends
   (the domain's own fixture tests show the fields it reads). Keep the facts
   consistent with the table above; add to it when you add a fact.
3. Timestamps in answers are as the service writes them (ADO's seven
   fractional digits, offsets); the CLI prints them as UTC `Z`. Pick times
   before `AGENT_CLI_NOW`.
4. `cargo test -p agent-cli --features fixtures --test world` runs the
   commands the world must answer; add yours there.

**A new domain** (airflow, dd) adds `http/<domain>.json` for its host
(`https://airflow.contoso.example`, `https://api.datadoghq.eu`), its section in
`config.toml`, any credentials as `*_env` variables `scripts/trial-env.sh`
exports with a stand-in value, and rows to the facts table that tie it to the
rest: the Airflow DAG whose task pods run in `prod/web`, the Datadog monitor
on `service:api` that alerted when the worker began crash-looping.

Names stay `contoso`/`fabrikam`-style placeholders: the repository is public.
