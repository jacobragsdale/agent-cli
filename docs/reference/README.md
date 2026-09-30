# Command reference

Generated from the registry by `crates/cli` (`UPDATE_DOCS=1 cargo test -p agent-cli reference`); a test fails when it is stale. Each domain's page holds, for every command, what `agent-cli <domain> <resource> <verb> --help` prints: arguments (`*` required), `Returns:`, the effect, and an example.

93 commands in 8 domains. Every command also takes the globals `--fields a,b.c`, `--raw`, `--dry-run`, `--yes`, `--reveal`, `--timeout S`, `--output FILE` and `--no-cache`. Exit codes: 0 ok, 1 failed, 2 fix the call, 3 needs setup, 4 not found, 5 conflict, 124 timed out.

| Domain | Summary | Commands |
|---|---|---|
| [ado](ado.md) | Azure DevOps | 29 |
| [kv](kv.md) | Key Vault | 4 |
| [acr](acr.md) | Container Registry | 4 |
| [aks](aks.md) | AKS | 2 |
| [k8s](k8s.md) | Kubernetes | 13 |
| [sql](sql.md) | SQL Server/Oracle | 6 |
| [airflow](airflow.md) | Apache Airflow | 15 |
| [dd](dd.md) | Datadog | 20 |
