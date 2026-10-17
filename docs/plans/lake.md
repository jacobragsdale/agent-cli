# Plan: lake, Azure Data Lake Storage Gen2

Status: plan (2026-10-16), nothing built. A domain that connects with the
`az login` already in use, shows every container and table in the lake, and
runs SQL over its files. Gen1 was retired in 2024 and is out of scope.

## Why it earns a domain

The recurring tasks, seven against the bar's six:

1. Did today's file or partition land, and when?
2. Which partitions of a table are missing or late?
3. What is in a file: its first lines, size, writer and time?
4. A table's schema, version, last write and the operation that made it.
5. Why does a pipeline's identity get 403 on a path?
6. Query a file or table: a count, the nulls, a sample, raw against curated.
7. Every container across the accounts (the lake's data domains), and the
   tables in each.

Where the raw tools lose (step 0's trial confirms or refutes each):

- `az storage fs file list` recurses by default and prints every property of
  every path; a real table's listing runs to megabytes, with no time filter.
- Every call needs `--account-name` and `--auth-mode login`; without the
  latter az falls back to account keys and fails or asks for them.
- A 403 needs RBAC and the ACL of every parent folder: execute on each
  folder, read on the file. Agents check the file's ACL alone, and miss that a
  data role (Storage Blob Data Reader) bypasses ACLs while Owner on the
  account grants no data at all.
- A query needs a local engine, a download of the right files and a token:
  many calls, and the wrong files when a Delta table has removed some.
- An `abfss://` URL from a log has to be split into account, filesystem and
  path by hand.

## Shape

`lake` is the seventh domain in `crates/azure` (`agent_cli_azure::LAKE`),
over plain HTTPS with `az_token(ctx, "https://storage.azure.com/", fresh)`,
as kv's data plane does. `crates/cli` gains the feature `lake`; the query
engine is an optional dependency of `agent-cli-azure` behind a `lake`
feature of its own, so a build without lake compiles no DuckDB.

Synonyms: adls, datalake, data lake, abfss, blob, storage, parquet, delta.

## Connect: config and doctor

Nothing to configure when the az login reaches the accounts:

```toml
[azure]
# storage_accounts = ["contosolake", "contosolakedev"]
```

`storage_accounts` is an allowlist like `vaults`; empty is every account
with a hierarchical namespace the login reaches. `graph.rs`'s inventory
gains storage accounts where `properties.isHnsEnabled` is true, cached as
the others (the cache key moves to inventory4).

- The token goes only to `{account}.dfs.core.windows.net` and
  `{account}.blob.core.windows.net` for an account in reach: the account is
  checked, not just the suffix, on every URL, since a ref names the host.
- A ref naming an account out of reach is exit 2, naming `lake account list`.
- `doctor lake`: one row per account,
  `GET https://{account}.dfs.core.windows.net/?resource=account&maxResults=1`.
  A 403 (`AuthorizationPermissionMismatch`) hints the data role and
  `agent-cli arm role list --scope ACCOUNT`, saying Owner or Contributor on
  the account is control plane only. A name that does not resolve hints
  `agent-cli net endpoint list`. No token hints `az login`.
- Requests send `x-ms-version: 2023-11-03` (VERIFY(work) the newest the
  accounts accept).

## Ids

- An account `contosolake`, a container `contosolake/sales`, a path
  `contosolake/sales/orders/date=2026-10-16/part-0001.parquet` (a directory
  the same, without a trailing slash).
- Every verb that takes a path also takes `abfss://` and `abfs://`
  (`sales@contosolake.dfs.core.windows.net/orders/…`), `wasbs://`, the
  `https://` URL on `dfs` or `blob`, and Snowflake's `azure://` stage URL
  (`contosolake.blob.core.windows.net/sales/…`): parsed and percent-decoded,
  never fetched.
- Rows name other domains' things by their ids: an account's `arm` (for
  `arm resource get`) and `endpoints[]` (for `net endpoint get`); `access
  get`'s principal (for `arm role list --principal`, `arm identity get`).

## Commands

All Read. Writes (`lake path delete`, Destructive) wait until asked.

### Phase 1: connect and browse

| Command | API | Returns |
|---|---|---|
| `lake account list` | the inventory (Resource Graph) | `[{id,subscription,resource_group,location,sku,public_network_access,default_action,endpoints[],arm}]` |
| `lake container list [ACCOUNT]` | `GET https://{a}.dfs.core.windows.net/?resource=account` per account, through `client::parallel` | `[{id,account,modified,metadata}]` |
| `lake path list REF` | `GET https://{a}.dfs.core.windows.net/{fs}?resource=filesystem&directory={dir}&recursive=false&maxResults=5000`, paged by `x-ms-continuation` | `[{id,kind,size,modified,owner}]` |
| `lake path get REF` | `HEAD` the path, `HEAD ?action=getAccessControl&upn=true`, and for a text file `GET` with `x-ms-range: bytes=0-16383` | `{id,kind,size,modified,etag,content_type,owner,group,permissions,acl[],head[]}` |

- `container list` with no account lists every container in every account:
  the data domains in one call.
- `path list` reads one level; `--recursive` descends, bounded by `--limit`
  (50) and the deadline. `--since` and `--until` filter on last-modified
  client-side, since the API has none. The note gives the totals:
  `[312 files, 4.1 GiB under contosolake/sales/orders; --limit N]`.
- `path get`'s `head` is the first lines of a CSV, JSON, JSON Lines or text
  file (by extension, else content type), within 16 KiB.
- Core gains `Method::Head`: Get Properties and the ACL come only as a HEAD's
  headers.

### Phase 2: tables

| Command | API | Returns |
|---|---|---|
| `lake table list REF` | listings, at most `--depth` (3) levels below REF | `[{id,format,partitioned_by[],modified}]` |
| `lake table get REF` | Delta: `_delta_log/_last_checkpoint`, the checkpoint, the JSON commits after it. Else the newest data file's schema | `{id,format,version,modified,operation,operation_parameters,partitioned_by[],schema[{name,type,nullable}],files,size}` |
| `lake partition list REF` | Delta: the log's add actions. Else one listing per partition level | `[{id,values{},files,size,modified}]` |

- A table root is a folder holding `_delta_log/` (delta), or `key=value`
  folders, or data files of one format (parquet, csv, json). `table list`
  stops descending at a root.
- `table get` reads a Delta checkpoint (Parquet) and a plain Parquet file's
  schema with the query engine, so phase 2 needs phase 4's dependency.
- `partition list` takes `--since` and `--limit`. A date-valued partition
  column's gaps go in a note:
  `[missing between 2026-10-10 and 2026-10-16: date=2026-10-12, date=2026-10-14]`.
  A Delta table's partitions come from its log, exact for the current
  version, with no listing.

### Phase 3: access

`lake access get REF --principal ID [--write]` answers whether the
principal can read (or write) the path, and if not, the first place it
breaks:

1. RBAC: the role assignments at the account's scope and above
   (`arm/role/mod.rs`'s `assignments`, in the same crate), matched to the
   data roles by definition id; a custom role by its `dataActions`
   (VERIFY(work)). A data role grants, and the ACLs are not consulted.
2. Else the ACLs: `getAccessControl` on the container root and each folder
   down to REF. Each folder needs execute; the leaf needs read (write and
   execute on its parent for `--write`). An entry counts for the principal,
   any of its groups, or other, under the mask; the owning user's bits for
   the owner.

The principal's groups come from Microsoft Graph,
`POST https://graph.microsoft.com/v1.0/directoryObjects/{id}/getMemberObjects`
with a Graph token, so `graph.microsoft.com` joins the token's hosts
(VERIFY(work) that the login may call it). `--principal @me` is the oid in
the az token.

Returns `{id,principal,allowed,via,broken_at{path,needs,has},steps[{path,acl,grants}]}`.
A denial exits 0 and prints
`[next: agent-cli arm role list --principal ID --scope ACCOUNT]`.

### Phase 4: queries

`lake query run SQL --table NAME=REF …` runs SQL in an in-memory DuckDB
(the `duckdb` crate, bundled, with its parquet and json features; no httpfs
or azure extension; extension autoload and autoinstall off). `SQL` reads
stdin for `-`, and `--sql-file` reads a file, as `sql query run`'s do. Each
`--table` (repeatable) is bound before the SQL runs:

1. REF resolves to files: one file; a folder (every data file under it, of
   one format); a Delta table (its active files from the log, so removed
   files are never read); or a glob in the last segment
   (`…/orders/date=2026-10-1*`).
2. Past the cap (`--max-mb`, 1024) the command is exit 2 before any
   download, naming the size and `agent-cli lake partition list REF`. It
   never samples quietly.
3. Each file downloads through `ctx.read`, the token checked per URL, in
   parallel, into a private folder under the cache dir keyed by path and
   etag, so the next query on the same files downloads nothing.
4. `CREATE VIEW NAME AS SELECT * FROM read_parquet([...], hive_partitioning = true)`
   (or `read_csv_auto`, `read_json_auto`).

The SQL then runs with external access limited to that folder
(VERIFY `allowed_directories` in the pinned DuckDB). `--max-rows` (1000)
and the answer match `sql query run`:
`{results[{columns[],types[],rows[],truncated}],elapsed_ms,read{files,bytes,cached}}`.
It declares `timeout: 100`.

Tables are bound by flag rather than named in the SQL, so nothing parses
SQL, and every byte fetched is named on the command line and in the
`--dry-run` plan. It is a Read whatever the SQL says: the database is in
memory, and nothing in it reaches the lake or outlives the run.

## Joins

No crate calls another; each join is an id one side prints and the other
takes.

- airflow, controlm: an `abfss://` URL in a task's log or a job's output is
  a lake ref as it stands. A next note from their side when a log names one
  is their change, later.
- arm: account rows' `arm`; `access get` reads role assignments in-crate;
  its principal goes to `arm role list` and `arm identity get`.
- net: account rows' `endpoints[]`; doctor's resolution failure names
  `net endpoint list`.
- k8s: a workload identity's `client_id` (`k8s serviceaccount get`) leads to
  its `principal_id` (`arm identity get`), which `access get` takes.
- sql, with Snowflake later: stage URLs parse as refs, so a load history's
  file name leads to `lake path get`.

## The world

In `fixtures/world`, scrubbed like the rest:

- Accounts `contosolake` (prod, private endpoint `pe-contosolake-dfs`) and
  `contosolakedev`; containers `raw`, `sales` and `finance` in
  `contosolake`.
- `raw/landing/orders_20261016.csv`, written at 06:02 by a Control-M job (a
  job the controlm world already has, if one fits).
- `sales/orders`: Delta, partitioned by date, with `date=2026-10-14`
  missing and `date=2026-10-16` written by an Airflow DAG run.
- `finance/ledger`: Parquet. Airflow's workload identity holds no data role,
  and r-x on `/finance` but no execute on `/finance/ledger`. Its task log
  prints the 403 with the `abfss://` URL.
- The trace `world_cross.rs` walks: the Airflow task's 403, `lake access get`
  breaking at `/finance/ledger`, `arm role list --principal`.
- Replay answers are text today. Parquet files and Delta checkpoints need a
  binary body (a `body_file` under `fixtures/world/files/`, generated by a
  seed script with DuckDB): a core change.

## Step 0: the trial

Before code, as `docs/how-to/add-a-domain.md` asks: agents with `az` and the
`duckdb` CLI against a sandbox account seeded with the world's files
(`scripts/seed/lake.sh`), on five tasks:

1. Did yesterday's orders partition land, and when?
2. Which days of October are missing from `sales/orders`?
3. What is `finance/ledger`'s schema, and its row count for October?
4. Why does Airflow's identity get 403 on `finance/ledger`?
5. How many rows of today's landing file have no `customer_id`?

Count calls, tokens and wrong answers. lake earns its place if it wins at
least half.

## Costs and ceilings

- DuckDB is C++: minutes on a cold build, `g++` (which `build-essential`
  already brings for Oracle), and about 20 to 30 MB of binary. DataFusion is
  the pure-Rust alternative, with a larger dependency tree and a dialect
  agents know less well. DuckDB, for `read_parquet`, `read_csv_auto` and
  hive partitioning built in.
- Download then query: a GiB cap and no pushdown to the lake. The upgrade is
  DuckDB's azure extension with a secret scoped to the accounts, or a
  Synapse serverless or Databricks SQL connection kind in sql where the org
  has one.
- Path List has no time filter: `--since` scans, bounded by `--limit` and
  the deadline, with a note.
- Shared key and SAS: not planned; RBAC through the az login only.
- The overview's 1 KB budget and the azure card's 3 KB take one more domain;
  either may need trimming.

## Open questions

1. "Domains" at work: containers, top-level folders, or a catalog
   (Purview, Unity Catalog)?
2. Formats: Delta, Parquet, CSV, JSON, Avro? Avro needs DuckDB's avro
   extension, excluded above.
3. Typical partition sizes: does a 1 GiB cap cover the questions agents ask?
4. Is the lake reachable only through private endpoints (WSL resolving
   privatelink)?
5. May the az login's Graph token call `getMemberObjects` at work?
6. Does the org already have Synapse serverless or Databricks SQL over the
   lake?
7. Query rows reach the agent's context, as sql's do. Do some containers
   need to stay out (a container allowlist beside `storage_accounts`)?

Once built: what stays true moves to `crates/azure/AGENTS.md` and
`docs/explanation/design.md`, open items to `TODO.md`, and this plan goes.
