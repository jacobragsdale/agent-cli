# Command reference

Generated from the registry by `crates/cli` (`UPDATE_DOCS=1 cargo test -p agent-cli reference`); a test fails when it is stale. Each domain's page holds, for every command, what `agent-cli <domain> <resource> <verb> --help` prints: arguments (`*` required), `Returns:`, the effect, and an example.

Every command also takes the globals `--fields a,b.c`, `--raw`, `--dry-run`, `--yes`, `--reveal`, `--timeout S`, `--output FILE` and `--no-cache` (fetch again, and refresh the cache with it). Exit codes: 0 ok, 1 failed, 2 fix the call, 3 needs setup, 4 not found, 5 conflict, 124 timed out.

| Domain | Summary |
|---|---|
| [ado](ado.md) | Azure DevOps |
| [confluence](confluence.md) | Confluence Cloud |
| [kv](kv.md) | Key Vault |
| [acr](acr.md) | Container Registry |
| [aks](aks.md) | AKS |
| [aisearch](aisearch.md) | Azure AI Search |
| [k8s](k8s.md) | Kubernetes |
| [sql](sql.md) | SQL Server/Oracle |
| [airflow](airflow.md) | Apache Airflow |
| [controlm](controlm.md) | BMC Control-M |
| [dd](dd.md) | Datadog |
