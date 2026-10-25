# Team rollout

Decided with the user on 2026-10-24. agent-cli goes to the user's own team at
work (approved by the employer): WSL Ubuntu only, the team's agents are Cursor
and GitHub Copilot, and every one of the sixteen domains will be configured
there. Everything below is built here first; then the repository is
hard-forked into the work Azure DevOps and GitHub is no longer tracked. At work
the user fills in `team/team.toml`, runs the pipeline, runs `doctor` and the
reads of every domain, and fixes what breaks. Only then do teammates install.

Not in scope: agent trials, harness settings (auto-approve rules), skills,
the TUIs, Herdr, repo context, a Windows binary, archiving the GitHub repo.

## Phase A: core

### A1. Team config: `[core] include`

- `[core] include = ["~/.config/agent-cli/team.toml"]` (a string or a list;
  `~/` is the home directory; a relative path is relative to the including
  file). Only the file `Config::load` reads (`config.toml`, or
  `$AGENT_CLI_CONFIG`) may include; an included file that includes is exit 3.
- Merge: included files first, in order, then the including file over them.
  Scalars: the later file wins. Tables: merged key by key. Arrays of tables
  whose entries all have `name` (`[[sql.connection]]`, `[[k8s.scope]]`,
  `[[airflow.instance]]`, …): merged entry by entry on `name`; an entry in the
  later file replaces only the keys it sets, and a new name is appended. Any
  other array: the later file's replaces it.
- A missing included file is exit 3 naming it and the line that includes it,
  for every section (like a parse error), with the hint to rerun `install.sh`.
- `agent-cli config` shows `include` (each file, found or not) and, per
  section, the merged result. The builtin-config feature stays as it is.

### A2. `read_only`

- `read_only = true` in a domain's section (`[sql]`, `[azure]` for the eight
  Azure domains, `[datadog]` for dd, …) makes every `ctx.write` of that domain
  refuse; `read_only` on one scope entry (`[[sql.connection]]`,
  `[[airflow.instance]]`, `[[controlm.instance]]`, `[[redis.instance]]`,
  `[[k8s.scope]]`, later `[keeper]`) overrides the section's for that scope.
- The refusal is exit 2, before anything is sent, naming the scope:
  `sql connection prod is read-only in the config`, hint
  `agent-cli config` (which shows where it is set). `--dry-run` still prints
  the plan, with a note that the change would be refused.
- No flag and no environment variable lifts it (`AGENT_CLI_<SECTION>_READ_ONLY`
  overrides are refused for this key); a person sets `read_only = false` on
  the entry in their own `config.toml`, which wins over `team.toml` (A1).
- `Reveal` is a read; it is not refused.
- Read-only sessions: on a read-only connection, Oracle runs each batch in
  `SET TRANSACTION READ ONLY` and Postgres connects with
  `default_transaction_read_only=on`. SQL Server has no session equivalent:
  a prod MSSQL relies on a read-only login, which `docs/how-to/` says.

### A3. Credentials from Key Vault: `KEY_kv` and `config pull`

- A fourth source beside `KEY`, `KEY_env`, `KEY_cmd`: `KEY_kv = "VAULT/SECRET"`
  (vault name, not URL). `Credential::source()` says `password_kv VAULT/SECRET`.
- Values are kept in `~/.config/agent-cli/secrets.toml` (beside the config
  file; mode 0600; written only by agent-cli), a table keyed `"VAULT/SECRET"`.
  `resolve` uses the kept value; with none, it reads the secret from Key
  Vault with the `az` login (`https://vault.azure.net` token, `host_under`
  `.vault.azure.net`), keeps it, and uses it.
- When a kept value fails to sign in (a 401, SQL Server 18456, ORA-01017,
  Postgres 28P01, Redis WRONGPASS, …), the domain asks the credential to
  refresh: it reads Key Vault again, and if the value changed, keeps it and
  the domain retries once. An unchanged value is the original failure.
- `agent-cli config pull`: reads every `*_kv` key in the merged config,
  reads each from Key Vault, rewrites `secrets.toml`, and prints one row per
  key `{section, key, kv, ok, changed, detail}`, never a value. `install.sh`
  runs it.
- Never printed: `agent-cli config` shows `*_kv` (a reference, like `*_env`)
  and never `secrets.toml`'s values.

### A4. Hide what a person cannot use

- Search and the overview leave out domains whose `status` is not set up, and
  domains the last `doctor` found not permitted. Search ends with a note
  `[N more in domains not set up here: k8s, net; agent-cli search --all WORDS]`;
  `search --all` searches every domain. The overview folds them into one
  line. Running a hidden domain's command still works (a not-set-up one says
  so, as today).
- Not permitted: a doctor `Check` can be marked denied (a 403 on the check's
  read); `doctor` writes the denied domains to the state file
  `~/.local/state/agent-cli/denied.json` (`$XDG_STATE_HOME`), and a later
  `doctor` that passes clears them. Its detail names the missing role where
  the domain knows it.
- The search gate and the trials run with every domain configured, as now.

### A5. Usage log: `agent-cli usage`

- Every command run appends one JSON line to
  `~/.local/state/agent-cli/usage.jsonl`: time, version, command path
  (`sql query run`), flag names given (never values, never positionals),
  exit code, milliseconds, HTTP requests sent, bytes received, child
  processes run. Nothing is ever sent anywhere. `AGENT_CLI_USAGE=off` stops
  it. The file is cut to its newest half past 5 MB. Tests and fixtures never
  write it.
- `agent-cli usage [--since 7d]`: a summary as JSON: calls, by command
  (count, failures by exit code, median and slowest ms, requests, bytes), the
  most frequent exit 2s with their flag names, the slowest, the most
  requests. For a teammate to paste into the internal ADO board.
- This is the TODO's first ADO efficiency step (count requests and bytes).

### A6. `doctor` checks the version

- `[core] releases = "https://keeper.contoso.example/api/v1/repositories/agent-cli/download"`
  (in `team.toml`). `doctor`'s core row reads `{releases}/latest/VERSION`
  (anonymous, a few seconds at most) and says `agent-cli 0.2.0 · 0.3.0 is
  out; rerun install.sh` when newer. No releases key, or no answer, is a
  detail, never a failure. Nothing else reaches the network for this.

## Phase B: Postgres in sql

- `kind = "postgres"` in `[[sql.connection]]`: `host`, `port` (5432),
  `database`, `user`, the password keys (all four), `auth = "az"` (an Entra
  token for `https://ossrdbms-aad.database.windows.net` as the password, for
  Azure Database for PostgreSQL Flexible Server), `sslmode`-like TLS keys as
  sql's mssql ones are. Self-hosted in AKS is a plain host.
- All eight verbs: `query run` and `bench`, `object list|get|diff` (function
  and procedure source through `pg_get_functiondef`), `schema list|diff`,
  `connection list`. Read-only connections connect with
  `default_transaction_read_only=on` (A2).
- `scripts/db-up.sh` starts a Postgres beside SQL Server and Oracle; the
  `AGENT_CLI_TEST_DBS=1` suite covers it, two drifted shop schemas included.

## Phase C: `keeper`

- Artifact Keeper (artifactkeeper.com, MIT), built and run live against the
  latest stable release (v1.11.7 on 2026-10-24) in a local container,
  `scripts/keeper-up.sh`, seeded with a PyPI and a generic repository; the
  live suite is opt-in (`AGENT_CLI_TEST_KEEPER=1`).
- Read only: `repository list|get`, `package list|get` (versions), `version
  get` (size, checksum, created, who published), `file get --output FILE`.
  A remote (proxy) repository's package says whether it is cached.
- `[keeper] url`; anonymous by default, `token`/`token_env`/`token_cmd`/
  `token_kv` optional for private repositories; `doctor` names which, and the
  server's version.

## Phase D: distribution

- `team/team.toml`: the team's config with contoso placeholders, every
  section, `[core] releases`, `read_only` on prod entries, `*_kv` for shared
  logins, `[sql] oracle_client_dir` matching `install.sh`'s Instant Client.
- `scripts/install.sh`, served from Keeper's generic repository with no
  sign-in: the binary to `~/.local/bin`, `team.toml` to
  `~/.config/agent-cli/team.toml` (replaced on every run), `[core] include`
  added to `config.toml` on the first run only, Oracle Instant Client Basic
  Light pinned and checksummed to `~/.local/opt/oracle` with `libaio1t64`
  (`libaio1` before 24.04), the `libaio.so.1` link and the run path, then
  `agent-cli config pull` and `agent-cli doctor`. Running it again updates.
- Release layout in the generic repository: `VERSION/agent-cli-x86_64-linux.tar.gz`,
  `.sha256`, `team.toml`, `install.sh`, and the same under `latest/` with a
  `VERSION` file.
- `azure-pipelines.yml`: in a container built from `ci/Dockerfile`
  (`ubuntu:22.04`, rustup, the workspace's toolchain), CI's five checks on
  every push to main; on a `v*` tag matching the workspace version, a release
  build uploaded to Keeper with a pipeline secret.
- The one-line note for each person's system prompt names no services.
- Verify at work: whether the build workers reach crates.io (else Keeper's
  crates.io proxy or `cargo vendor`), Control-M's Automation API, the Keeper
  version, the roles entra, arm and aisearch need.
