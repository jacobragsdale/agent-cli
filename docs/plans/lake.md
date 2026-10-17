# Plan: lake, Azure Data Lake Storage Gen2

Status: plan (2026-10-16), nothing built. A domain that connects with the
`az login` already in use, shows every container and the data domains in
it, and runs SQL over its Parquet files. Gen1 was retired in 2024 and is out
of scope.

Decided with the user: the lake's data domains are its top-level folders
(perhaps its containers), so a container's row carries both; the data is
Parquet, so no Delta, CSV or JSON reader is planned; no Synapse or
Databricks SQL sits over the lake, so queries run locally.

## Why it earns a domain

The recurring tasks, seven against the bar's six:

1. Did today's file or partition land, and when?
2. Which partitions of a table are missing or late?
3. What is in a file: its schema, rows, size, writer and time?
4. A table's schema, row count and last write.
5. Why does a pipeline's identity get 403 on a path?
6. Query a file or table: a count, the nulls, a sample, raw against curated.
7. Every container across the accounts, the data domains in each, and their
   tables.

Where the raw tools lose (step 0's trial confirms or refutes each):

- `az storage fs file list` recurses by default and prints every property of
  every path; a real table's listing runs to megabytes, with no time filter.
- Every call needs `--account-name` and `--auth-mode login`; without the
  latter az falls back to account keys and fails or asks for them.
- A 403 needs RBAC and the ACL of every parent folder: execute on each
  folder, read on the file. Agents check the file's ACL alone, and miss that a
  data role (Storage Blob Data Reader) bypasses ACLs while Owner on the
  account grants no data at all.
- A Parquet file's schema or row count means downloading it; a query means a
  local engine, a download of the right files and a token: many calls.
- An `abfss://` URL from a log has to be split into account, filesystem and
  path by hand.

## Shape

`lake` is the seventh domain in `crates/azure` (`agent_cli_azure::LAKE`),
over plain HTTPS with `az_token(ctx, "https://storage.azure.com/", fresh)`,
as kv's data plane does. `crates/cli` gains the feature `lake`; the query
engine is an optional dependency of `agent-cli-azure` behind a `lake`
feature of its own, so a build without lake compiles no DuckDB.

Synonyms: adls, datalake, data lake, abfss, blob, storage, parquet.

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

- An account `contosolake`, a container `contosolake/raw`, a path
  `contosolake/raw/sales/orders/date=2026-10-16/part-0001.parquet` (a
  directory the same, without a trailing slash).
- Every verb that takes a path also takes `abfss://` and `abfs://`
  (`raw@contosolake.dfs.core.windows.net/sales/…`), `wasbs://`, the
  `https://` URL on `dfs` or `blob`, and Snowflake's `azure://` stage URL
  (`contosolake.blob.core.windows.net/raw/…`): parsed and percent-decoded,
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
| `lake container list [ACCOUNT]` | `GET https://{a}.dfs.core.windows.net/?resource=account` per account, then each container's root, one level, through `client::parallel` | `[{id,account,modified,folders[]}]` |
| `lake path list REF` | `GET https://{a}.dfs.core.windows.net/{fs}?resource=filesystem&directory={dir}&recursive=false&maxResults=5000`, paged by `x-ms-continuation` | `[{id,kind,size,modified,owner}]` |
| `lake path get REF` | `HEAD` the path and `HEAD ?action=getAccessControl&upn=true`; for a Parquet file, its footer (below) | `{id,kind,size,modified,etag,owner,group,permissions,acl[],rows,schema[{name,type}]}` |

- `container list` with no account lists every container in every account,
  each with its top-level folders as path ids: the data domains in one call.
  At most 50 folders a container, with a note naming `lake path list
  CONTAINER` when there are more.
- `path list` reads one level; `--recursive` descends, bounded by `--limit`
  (50) and the deadline. `--since` and `--until` filter on last-modified
  client-side, since the API has none. The note gives the totals:
  `[312 files, 4.1 GiB under contosolake/raw/sales/orders; --limit N]`.
- Core gains `Method::Head`: Get Properties and the ACL come only as a HEAD's
  headers.

### Reading a Parquet footer

A Parquet file ends in its metadata, its length and `PAR1`. Two ranged GETs
(`x-ms-range`: the last 8 bytes, then the footer) fetch it without the data.
The bytes are written at their offsets into a sparse local file of the
file's size, and DuckDB's `parquet_schema()` and `parquet_metadata()` read
the schema and each row group's row count from it, never touching the
holes (VERIFY: else the parquet crate's metadata reader, a second
dependency). `path get`, `table get` and `partition list` read footers this
way; only `query run` downloads data.

### Phase 2: tables

| Command | API | Returns |
|---|---|---|
| `lake table list REF` | listings, at most `--depth` (3) levels below REF | `[{id,partitioned_by[],files,modified}]` |
| `lake table get REF` | a recursive listing and the footers of at most 200 files | `{id,partitioned_by[],schema[{name,type}],files,size,rows,modified,newest}` |
| `lake partition list REF` | one listing per partition level, and each partition's footers within the same 200 | `[{id,values{},files,size,rows,modified}]` |

- A table root is a folder holding `key=value` folders or `.parquet` files.
  `table list` stops descending at one; with no REF it starts at every data
  domain (each container's top-level folders).
- `table get`'s schema is the newest file's; a file whose schema differs is
  named in a note. Past 200 files, `rows` counts the newest 200 and the note
  says so.
- `partition list` takes `--since` and `--limit`. A date-valued partition
  column's gaps go in a note:
  `[missing between 2026-10-10 and 2026-10-16: date=2026-10-12, date=2026-10-14]`.

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
(the `duckdb` crate, bundled, with its parquet feature; no httpfs or azure
extension; extension autoload and autoinstall off). `SQL` reads stdin for
`-`, and `--sql-file` reads a file, as `sql query run`'s do. Each `--table`
(repeatable) is bound before the SQL runs:

1. REF resolves to Parquet files: one file, a folder (every `.parquet`
   under it), or a glob in the last segment (`…/orders/date=2026-10-1*`).
2. Past the cap (`--max-mb`, 1024) the command is exit 2 before any
   download, naming the size and `agent-cli lake partition list REF`. It
   never samples quietly.
3. Each file downloads through `ctx.read`, the token checked per URL, in
   parallel, into a private folder under the cache dir keyed by path and
   etag, so the next query on the same files downloads nothing.
4. `CREATE VIEW NAME AS SELECT * FROM read_parquet([...], hive_partitioning = true)`,
   so partition folders' values are columns.

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
  `contosolakedev`; containers `raw` and `curated` in `contosolake`, each
  with the data domains `sales` and `finance` as top-level folders.
- `raw/sales/orders`: Parquet, partitioned by `date`, with
  `date=2026-10-14` missing and `date=2026-10-16` written by a Control-M job
  at 06:02 (a job the controlm world already has, if one fits).
- `curated/sales/orders_daily`: written by an Airflow DAG run from
  `raw/sales/orders`, one row a day, so raw against curated finds the gap.
- `curated/finance/ledger`: Airflow's workload identity holds no data role,
  and r-x on `/finance` but no execute on `/finance/ledger`. Its task log
  prints the 403 with the `abfss://` URL.
- The trace `world_cross.rs` walks: the Airflow task's 403, `lake access get`
  breaking at `/finance/ledger`, `arm role list --principal`.
- Replay answers are text today. Parquet files need binary bodies (a
  `body_file` under `fixtures/world/files/`, generated by a seed script with
  DuckDB), and the footer reads need replay to serve an `x-ms-range` from
  that file: a core change.

## Step 0: the trial

Before code, as `docs/how-to/add-a-domain.md` asks: agents with `az` and the
`duckdb` CLI against a sandbox account seeded with the world's files
(`scripts/seed/lake.sh`), on five tasks:

1. Did yesterday's orders partition land, and when?
2. Which days of October are missing from `raw/sales/orders`?
3. What is `curated/finance/ledger`'s schema, and its row count?
4. Why does Airflow's identity get 403 on `curated/finance/ledger`?
5. How many of today's orders have no `customer_id`?

Count calls, tokens and wrong answers. lake earns its place if it wins at
least half.

## Costs and ceilings

- DuckDB is C++: minutes on a cold build, `g++` (which `build-essential`
  already brings for Oracle), and about 20 to 30 MB of binary. DataFusion is
  the pure-Rust alternative, with a larger dependency tree and a dialect
  agents know less well. DuckDB, for `read_parquet`, hive partitioning and
  `parquet_metadata()` built in.
- Download then query: a GiB cap and no pushdown to the lake. The upgrade is
  DuckDB's azure extension with a secret scoped to the accounts, which reads
  only the row groups a query needs.
- Path List has no time filter: `--since` scans, bounded by `--limit` and
  the deadline, with a note.
- Shared key and SAS: not planned; RBAC through the az login only.
- The overview's 1 KB budget and the azure card's 3 KB take one more domain;
  either may need trimming.

## Open questions

1. Are the data domains the same top-level folders in every container
   (zones as containers, domains as folders), or does each container hold
   one domain?
2. Typical partition sizes: does a 1 GiB cap cover the questions agents ask?
3. Is the lake reachable only through private endpoints (WSL resolving
   privatelink)?
4. May the az login's Graph token call `getMemberObjects` at work?
5. Query rows reach the agent's context, as sql's do. Do some containers
   need to stay out (a container allowlist beside `storage_accounts`)?

Once built: what stays true moves to `crates/azure/AGENTS.md` and
`docs/explanation/design.md`, open items to `TODO.md`, and this plan goes.
