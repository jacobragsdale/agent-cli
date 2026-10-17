# agent-cli

One command-line tool that gives AI coding agents Azure DevOps, Confluence
Cloud, Azure (Key Vault, Container Registry, AKS, AI Search, Data Lake), Kubernetes, SQL
Server and Oracle, Apache Airflow, BMC Control-M and Datadog. Its only users
are agents: every command is `agent-cli <domain> <resource> <verb>`, output is
compact JSON that `--fields` narrows, errors name the command to run next, and
every change honours `--dry-run` and `--yes`.

## Install

You need Rust 1.88 or later and a C compiler (the Oracle driver builds a small
C library).

```sh
cargo install --git https://github.com/jacobragsdale/agent-cli agent-cli
```

Oracle connections also need Oracle Instant Client at run time; nothing else
does. To tell your agents the tool exists, one line in a global `CLAUDE.md` or
`AGENTS.md` is enough: "agent-cli gives you Azure DevOps, Confluence, Azure
(AI Search too), Kubernetes, SQL, Airflow, Control-M and Datadog; start with
`agent-cli`."

## Configure

Copy [config.example.toml](config.example.toml) to
`~/.config/agent-cli/config.toml` (or point `AGENT_CLI_CONFIG` at a file) and
uncomment the sections you need. The binary carries it: `agent-cli config
example sql` prints one domain's section, and `agent-cli config` shows what
was read. [Set up your services](docs/how-to/set-up-your-services.md) walks
through each domain, WSL included. Each domain reads only its own section, so a
mistake in one breaks only that domain. `AGENT_CLI_<SECTION>_<KEY>` overrides
any key, for example `AGENT_CLI_ADO_PROJECT=web`.

| Domain | Section | Sign-in |
|---|---|---|
| ado | `[ado]` org, project, team | `pat`, else `AZURE_DEVOPS_EXT_PAT`, else your `az login` |
| confluence | `[confluence]` url, email | an API token: `token` |
| kv, acr, aks, aisearch, arm, net, lake, entra | `[azure]` (optional allowlists) | your `az login` (aisearch: a token, or an admin key it fetches, per service; lake: a data role or ACLs; entra: a member of the tenant, the logs a reader role) |
| k8s | `[[k8s.scope]]` per cluster | your kubeconfig; `agent-cli aks cluster connect NAME` fetches it for AKS |
| sql | `[[sql.connection]]` per database | `user` and `password` |
| redis | `[[redis.instance]]` per Redis | `password` (and an ACL `user`), or your `az login` (`auth = "az"`) |
| airflow | `[[airflow.instance]]` per server | `username` and `password`, or `token` |
| controlm | `[[controlm.instance]]` per Enterprise Manager | `username` and `password`, or an API token: `token` |
| dd | `[datadog]` site and env | `token`, else `DD_ACCESS_TOKEN` |

One file holds every service, credentials included (`chmod 600` it). Any
credential `KEY` can instead be `KEY_env` (a variable that holds it) or
`KEY_cmd` (a command that prints it, such as `pass show contoso/db`). Each
goes only to its own service's hosts. Check the setup with:

```sh
agent-cli doctor
```

It prints one JSON row per check and exits 1 if any failed, each failure with
a hint.

## The 60-second tour

```sh
agent-cli                                            # the overview: domains, flags, exit codes, config
agent-cli search "which image is running in prod"    # commands ranked for a task
agent-cli k8s deployment list --help                 # args, the Returns: shape, an example
agent-cli k8s deployment list --fields id,images     # the call, narrowed to what you need
```

Search is the front door; help shows the exact shape a command returns, so
`--fields` can ask for only that. Without `--fields`, output over 12 KB is cut
to valid JSON (in a terminal too, pretty-printed) and the whole of it saved
to a file whose path stderr names; `--raw` prints everything.

## Safety

Every command declares an effect, shown in search and help:

| Effect | What happens |
|---|---|
| read | Runs. |
| write | Runs; `--dry-run` prints the planned request instead. |
| destructive | Refused without `--yes` (exit 2, naming the exact command to re-run). |
| reveals a secret | Refused without `--reveal`, or `--output FILE`, which writes the value to a 0600 file and prints only the path. |
| read or write | Decided by the input, as for SQL: a write follows the rules above. |

Tokens and passwords never appear in output, plans or errors; a `diff`
compares two secrets' values in memory and prints only whether they match. Where an agent
must not change something, give it a credential that can only read: a Reader
role, a read-only database login, an Airflow Viewer.

## Domains

| Domain | Service | Commands |
|---|---|---|
| ado | Azure DevOps: work items, pull requests, pipelines, runs, approvals, service connections | 59 |
| confluence | Confluence Cloud pages as Markdown, page trees, comments, attachments, versions | 14 |
| kv | Key Vault secrets and versions (metadata; values only with `--reveal`) | 4 |
| acr | Container Registry repositories, tags, manifests | 4 |
| aks | AKS clusters, node pools, upgrades, credentials, kubectl through ARM | 8 |
| aisearch | Azure AI Search services, indexes, documents (keyword, vector, hybrid, semantic), indexers | 16 |
| arm | Any Azure resource: Resource Graph lists and queries, ARM reads by id, activity log, changes, role assignments, managed identities, policy, metrics | 12 |
| net | Azure vnets, subnets and their free IPs, NSG rules, NICs' effective rules and routes, route tables | 7 |
| lake | Data Lake Storage Gen2: containers, folders, Parquet tables and partitions from footers, why a path is refused; queries go to Synapse through `sql` | 8 |
| entra | Microsoft Entra ID through Graph: who an object id is, users, groups and nested members, app registrations and their expiring credentials, service principals and managed identities with their grants, sign-in and audit logs, directory roles, two principals' access compared (read-only) | 12 |
| k8s | Kubernetes pods, logs, events, deployments, config maps, secrets | 14 |
| sql | SQL Server, Oracle and Synapse serverless queries and catalog | 6 |
| redis | Redis keys, values decoded (JSON, gzip, .NET cache entries; pickles named, never loaded), expiry, qa against prod, instance health, slow log, clients (read-only) | 7 |
| airflow | Apache Airflow 2.9+ and 3 DAGs, runs, task instances, logs, import errors | 21 |
| controlm | BMC Control-M job runs, their status, output and definitions; rerun and order | 7 |
| dd | Datadog logs, metrics, monitors, downtimes, events, APM, incidents, SLOs | 20 |

## Documentation

- [Tutorial: trace a deploy and a failed DAG](docs/tutorial.md), against a
  recorded world with no cloud access.
- How-to guides: [set up your services](docs/how-to/set-up-your-services.md),
  [add a command](docs/how-to/add-a-command.md),
  [add a domain](docs/how-to/add-a-domain.md),
  [run agent trials](docs/how-to/run-agent-trials.md),
  [the first live run against each service](docs/first-live-run.md).
- Reference: [every command](docs/reference/README.md), one page per domain,
  generated from the registry; [config.example.toml](config.example.toml).
- Explanation: [why the interface is shaped this way](docs/explanation/design.md).
- Contributors and agents working on this repository: [AGENTS.md](AGENTS.md).

## License

MIT.
