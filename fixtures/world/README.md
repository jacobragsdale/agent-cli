# The contoso world

A small, consistent, entirely synthetic company that agent trials run against
when no real Azure DevOps, Confluence, Azure, cluster, Airflow or Datadog is
reachable.
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
| `AGENT_CLI_FIXTURES_MATCH` | `loose`: a request with no recording gets the closest recording of its method and path, whatever its query or body, and a path never recorded gets a 404. Agents pick their own windows and limits, and a trial should show them a service, not the harness. Unset it to see what a command would need recorded |
| `AGENT_CLI_CONFIG` | `config.toml` here: `[ado]`, `[confluence]`, `[azure]`, `[[k8s.scope]]`, `[[airflow.instance]]`, one `[[sql.connection]]` (no sql recordings), `[datadog]`. Its credentials are `*_cmd` stand-ins (`echo stand-in`): the recorded `/auth/token` answers any password, and the replayer never checks a token |
| `AGENT_CLI_NOW` | `2026-09-29T12:00:00Z`, the moment the world was recorded. Relative times (`--since 1d`, `--expires-within 30d`, ages) resolve against it, so they keep matching the recordings on any day |
| `PATH` | `target/debug` (the fixtures build) and `scripts/fake` (`az`, `kubectl`, `kubelogin`) first. The fake `az` hands out a stand-in token; nothing checks it |

A release build ignores `AGENT_CLI_FIXTURES`: the feature is not in it.

## What is true here

Each domain's facts are in `facts/`: [ado](facts/ado.md),
[confluence](facts/confluence.md), [acr](facts/acr.md),
[k8s](facts/k8s.md), [kv](facts/kv.md), [aks](facts/aks.md),
[airflow](facts/airflow.md), [dd](facts/dd.md),
[aisearch](facts/aisearch.md), [arm](facts/arm.md), [net](facts/net.md),
[lake](facts/lake.md). They tie the domains together:
the api image the cluster runs is the one the registry and the build hold, the
worker crash-loops on the password Key Vault let expire, the nightly DAG's pod
runs in the same namespace, Datadog alerted at the rollout, and the order the
DAG failed on is the one AI Search's indexer left out.

The deploy trace, for example (see AGENTS.md):

```sh
agent-cli k8s deployment list --fields id,images            # api runs contosoacr.azurecr.io/api:v1.4.2
agent-cli ado run list --branch refs/tags/v1.4.2 --fields id,result
agent-cli ado run get 8812 --fields commit,pr,workitems     # PR 431, work items 1207 and 1210
```

Why last night's DAG failed, and the hop to its pod:

```sh
agent-cli airflow run list --dag etl_nightly --state failed --since 1d --fields id
agent-cli airflow run get etl_nightly/latest --fields failed      # …/load_orders/2
agent-cli airflow task logs etl_nightly/latest/load_orders/2 --tail 20 --fields error
agent-cli airflow task get etl_nightly/latest/load_orders --fields pod
agent-cli k8s pod logs prod/web/etl-nightly-load-orders-q8x1k2vz --tail 20
```

Why order 88123 is missing from the orders search, and what the runbook says:

```sh
agent-cli aisearch document get srch-contoso-prod/orders/88123               # exit 4; the hint names the failing indexers
agent-cli aisearch indexer get srch-contoso-prod/orders-sql --fields errors  # key 88123: customer_id is null
agent-cli confluence page list "runbook etl_nightly" --fields id,title       # 1101 Runbook: etl_nightly
agent-cli confluence page get 1101 --section "An order without customer_id" --fields body
```

The four cluster traces (`crates/cli/tests/world_cross.rs`), each from the
symptom to its cause:

```sh
# 1. Private DNS: billing-sync cannot resolve its vault
agent-cli k8s pod run prod/web/billing-sync-5f8b6c9d7-b2s9x --yes -- nslookup kv-contoso-billing.vault.azure.net
agent-cli net endpoint list --fields id,target        # pe-kv-contoso-billing: no zone group, no record
# 2. Multi-Attach: orders-db-0 stuck creating, its disk on the NotReady node
agent-cli k8s pvc get prod/web/data-orders-db-0       # notes node get, then aks nodepool get
# 3. Pending pods: reports-api's two new pods fit nowhere
agent-cli k8s node list aks-user --fields id,nodepool,cpu
agent-cli aks nodepool get aks-contoso-prod/user      # the autoscaler at its maximum
# 4. Workload identity 403: the notifier's Key Vault reads refused
agent-cli k8s serviceaccount get prod/web/notifier --fields client_id
agent-cli arm identity get 00000000-0000-4000-8000-00000000b0f1 --fields principal_id,roles
agent-cli arm activity list --resource kv-contoso-prod     # dana deleted its role
```

Airflow's identity refused the finance ledger, and what the lake holds:

```sh
agent-cli arm identity get id-contoso-airflow --fields principal_id
agent-cli lake access get contosolake/curated/finance/ledger --principal 00000000-0000-4000-8000-00000000a1f2
agent-cli lake partition list contosolake/raw/sales/orders --fields id,rows,modified   # date=2026-09-27 missing
agent-cli lake table get contosolake/raw/sales/orders --fields schema,rows,bulk   # bulk: what Synapse's OPENROWSET reads
```

## Files

```
config.toml       the config agent-cli reads
http/ado.json     Azure DevOps answers
http/confluence.json Confluence Cloud answers (contoso.atlassian.net)
http/azure.json   Resource Graph, Key Vault and ACR answers
http/aisearch.json AI Search's data plane and its admin key (ARM)
http/lake.json    Data Lake answers and Graph's groups, written with files/ by scripts/seed/lake.py
files/lake/       the Data Lake's Parquet files, which lake.json's body_file answers serve
http/airflow.json Airflow's REST API (/api/v2 and /auth/token)
http/dd.json      Datadog answers (api.datadoghq.eu)
kubectl.json      what the fake kubectl knows: objects per context, and logs
facts/<domain>.md what the recordings say, one file per domain
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
- `body_file` (optional): a file under this directory whose bytes are the
  answer; a request with `Range: bytes=A-B` gets those bytes as a 206 (a
  Parquet footer read).
- `exact` (optional): the query parameters or body fields that name *what*
  is asked for (`["path"]` on a file read, `["searchText"]` on a code
  search). Trials match loosely, answering a miss with the closest recording
  on the same path; `exact` keeps that from answering one file with its
  neighbour's text. An unrecorded file is then a 404, and a code search
  falls back to the recording without a body, which finds nothing.

`kubectl.json` is `{"contexts": {"<context>": [objects]}, "logs": {...}}`.
Objects are what `kubectl get -o json` prints (`kind`, `metadata.namespace`
and `metadata.name` are what the fake filters on; a `CustomResourceDefinition`
among them names its kind for `get` and `api-resources`). Logs are keyed
`<context>/<namespace>/<pod>/<container>`, with `/previous` for `--previous`.
`"exec": {"<context>": {"<namespace>/<pod>/<container>": {"<argv>": {"stdout",
"stderr", "exit"}}}}` answers `k8s pod run`, the argv joined by spaces; an
argv not recorded is an error. A change (`cordon`, `rollout undo`, `scale`…)
answers what kubectl prints and changes nothing.

## Extend it

1. Run the command against the world with strict matching
   (`unset AGENT_CLI_FIXTURES_MATCH`; the tests always match strictly). A
   request with no recording fails with the one it wanted, the closest
   recording, and the body it sent:

   ```
   error: fixtures: no recorded answer for GET https://…/build/builds/8813?api-version=7.1; closest recorded: GET https://…/build/builds/8812?api-version=7.1
   ```

2. Add that exchange, with an answer in the shape the service really sends
   (the domain's own fixture tests show the fields it reads). Keep it
   consistent with the domain's `facts/<domain>.md`; add a line there when
   you add a fact.
3. Timestamps in answers are as the service writes them (ADO's seven
   fractional digits, offsets); the CLI prints them as UTC `Z`. Pick times
   before `AGENT_CLI_NOW`.
4. `cargo test -p agent-cli --features fixtures --test world_<domain>` runs
   the commands the world must answer of that domain; add yours to
   `crates/cli/tests/world_<domain>.rs`. A check that hops between domains
   goes in `world_cross.rs`.

**A new domain** (airflow, dd) adds `http/<domain>.json` for its host
(`https://airflow.contoso.example`, `https://api.datadoghq.eu`), its section in
`config.toml` (a credential as a `*_cmd` stand-in such as `token_cmd = "echo
stand-in"`, so neither `scripts/trial-env.sh` nor the tests change), and a
`facts/<domain>.md` that ties it to the rest: the Airflow DAG whose task pods
run in `prod/web`, the Datadog monitor on `service:api` that alerted when the
worker began crash-looping.

Names stay `contoso`/`fabrikam`-style placeholders: the repository is public.
