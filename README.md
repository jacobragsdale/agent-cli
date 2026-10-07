# agent-cli

One command-line tool that gives AI coding agents Azure DevOps, Azure (Key
Vault, Container Registry, AKS), Kubernetes, SQL Server and Oracle, Apache
Airflow and Datadog. Its only users are agents: every command is `agent-cli
<domain> <resource> <verb>`, output is compact JSON that `--fields` narrows,
errors name the command to run next, and every change honours `--dry-run`,
`--yes` and a read-only mode.

## Install

You need Rust 1.88 or later and a C compiler (the Oracle driver builds a small
C library).

```sh
cargo install --git https://github.com/jacobragsdale/agent-cli agent-cli
```

Oracle connections also need Oracle Instant Client at run time; nothing else
does. To tell your agents the tool exists, one line in a global `CLAUDE.md` or
`AGENTS.md` is enough: "agent-cli gives you Azure DevOps, Azure, Kubernetes,
SQL, Airflow and Datadog; start with `agent-cli`."

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
| ado | `[ado]` org, project, team | `AZURE_DEVOPS_EXT_PAT`, else your `az login` |
| kv, acr, aks, aisearch | `[azure]` (optional allowlists) | your `az login` (aisearch: a token, or an admin key it fetches, per service) |
| k8s | `[[k8s.scope]]` per cluster | your kubeconfig; `agent-cli aks cluster connect NAME` fetches it for AKS |
| sql | `[[sql.connection]]` per database | `password_env` or `password_cmd` |
| airflow | `[[airflow.instance]]` per server | username with `password_env`/`password_cmd`, or `token_env`/`token_cmd` |
| dd | `[datadog]` site and env | `token_env`/`token_cmd`, else `DD_ACCESS_TOKEN` |

A credential is named by where it comes from (`KEY_env` for a variable,
`KEY_cmd` for a command such as `pass show contoso/db`), never written into the
file, and goes only to its own service's hosts. Check the setup with:

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

`AGENT_CLI_READ_ONLY=1` refuses every write, destructive and reveal command
before anything is sent; the overview's `Config:` line and `agent-cli doctor`
say when it is on. A SQL connection or Airflow instance marked
`read_only = true` refuses changes on its own. Tokens and passwords never
appear in output, plans or errors. The real guard is still a credential that
can only read.

## Domains

| Domain | Service | Commands |
|---|---|---|
| ado | Azure DevOps: work items, pull requests, pipelines, runs, approvals | 57 |
| kv | Key Vault secrets and versions (metadata; values only with `--reveal`) | 4 |
| acr | Container Registry repositories, tags, manifests | 4 |
| aks | AKS clusters and their credentials | 2 |
| aisearch | Azure AI Search services, indexes, documents (keyword, vector, hybrid, semantic), indexers | 16 |
| k8s | Kubernetes pods, logs, events, deployments, config maps, secrets | 14 |
| sql | SQL Server and Oracle queries and catalog | 6 |
| airflow | Apache Airflow 2.9+ and 3 DAGs, runs, task instances, logs, import errors | 21 |
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
