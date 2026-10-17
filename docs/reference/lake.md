# lake — Data Lake Storage (9 commands)

| Command | Effect | Summary |
|---|---|---|
| [`lake account list`](#lake-account-list) | read | List the Data Lake storage accounts the az login reaches |
| [`lake container list`](#lake-container-list) | read | List Data Lake containers with their top-level folders (the data domains) |
| [`lake path list`](#lake-path-list) | read | List the files and folders under a Data Lake folder, with their total size |
| [`lake path get`](#lake-path-get) | read | Show a Data Lake file's size, time, owner, ACL, and Parquet schema and rows |
| [`lake table list`](#lake-table-list) | read | List the Parquet tables under a Data Lake folder or in every data domain |
| [`lake table get`](#lake-table-get) | read | Show a Parquet table's schema, rows, size and last write, read from its footers |
| [`lake partition list`](#lake-partition-list) | read | List a Parquet table's partitions with files, rows and writes; note missing days |
| [`lake access get`](#lake-access-get) | read | Say whether a principal can read or write a Data Lake path, and where it breaks |
| [`lake query run`](#lake-query-run) | read | Run SQL (DuckDB) over Data Lake Parquet files bound as tables with --table |

### lake account list

```text
agent-cli lake account list — List the Data Lake storage accounts the az login reaches
  --limit int  (default 50)
Returns: [{id,subscription,resource_group,location,sku,public_network_access,default_action,endpoints[],arm}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli lake account list --fields id,resource_group,public_network_access,endpoints
```

### lake container list

```text
agent-cli lake container list — List Data Lake containers with their top-level folders (the data domains)
  <account> str  Only this account's (from lake account list), or any id in it; all in reach by default
  --limit int    (default 50)
Returns: [{id,account,modified,folders[]}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli lake container list --fields id,folders
```

### lake path list

```text
agent-cli lake path list — List the files and folders under a Data Lake folder, with their total size
 *<path> str    The container or folder: an id (ACCOUNT/CONTAINER/PATH) or its abfss://, wasbs:// or https:// URL
  --recursive   Everything below it, not one level
  --since time  Only what changed since then
  --until time  Only what changed before then
  --limit int   (default 50)
A time is 15m, 2h, 7d, 1w (ago), now-15m, 2026-09-29, or RFC 3339.
Returns: [{id,kind,size,modified,owner}]
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli lake path list contosolake/raw/sales/orders --since 1d --fields id,size,modified
```

### lake path get

```text
agent-cli lake path get — Show a Data Lake file's size, time, owner, ACL, and Parquet schema and rows
 *<path> str  The file or folder: an id from lake path list, or its abfss://, wasbs://, https:// or azure:// URL
Returns: {id,kind,size,modified,etag,owner,group,permissions,acl[],rows,writer,schema[{name,type}]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli lake path get contosolake/raw/sales/orders/date=2026-09-29/part-0001.parquet --fields size,modified,rows,schema
```

### lake table list

```text
agent-cli lake table list — List the Parquet tables under a Data Lake folder or in every data domain
  <path> str   Where to look: an account, container or folder id, or its URL; every data domain by default
  --depth int  How many folder levels below it to look (default 3)
  --limit int  (default 50)
Returns: [{id,partitioned_by[],files,modified}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli lake table list contosolake/raw --fields id,partitioned_by,files,modified
```

### lake table get

```text
agent-cli lake table get — Show a Parquet table's schema, rows, size and last write, read from its footers
 *<path> str  The table's folder: an id from lake table list, or its abfss:// or https:// URL
Returns: {id,partitioned_by[],schema[{name,type}],files,size,rows,modified,newest}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli lake table get contosolake/curated/finance/ledger --fields schema,rows,modified
```

### lake partition list

```text
agent-cli lake partition list — List a Parquet table's partitions with files, rows and writes; note missing days
 *<path> str    The table's folder: an id from lake table list, or its abfss:// or https:// URL
  --since time  Only partitions written to since then
  --limit int   (default 50)
A time is 15m, 2h, 7d, 1w (ago), now-15m, 2026-09-29, or RFC 3339.
Returns: [{id,values,files,size,rows,modified}]
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli lake partition list contosolake/raw/sales/orders --since 7d --fields id,files,rows,modified
```

### lake access get

```text
agent-cli lake access get — Say whether a principal can read or write a Data Lake path, and where it breaks
 *<path> str       The file or folder: an id from lake path list, or its abfss:// or https:// URL
 *--principal str  Who: a user, group, service principal or managed identity's object id (arm identity get prints it as principal_id), or @me for the az login
  --write          Whether it can write there, rather than read
Returns: {id,principal,allowed,via,broken_at{path,needs,has},steps[{path,acl,grants}]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli lake access get contosolake/curated/finance/ledger --principal 00000000-0000-4000-8000-00000000a1f2
```

### lake query run

```text
agent-cli lake query run — Run SQL (DuckDB) over Data Lake Parquet files bound as tables with --table
  <sql> str        The SQL (DuckDB's dialect), or - to read it from stdin. Statements before the last run; the last one's rows return
 *--table str[]    A view the SQL reads: NAME=REF, REF a Parquet file, a folder (every file under it) or a glob in its last segment (…/date=2026-09-2*); repeat for more
  --sql-file path  The SQL from a file
  --max-mb int     The most to download, all tables together, in MiB (default 1024)
  --max-rows int   Keep at most this many rows (default 1000)
Returns: {results[{columns[],types[],rows[],truncated}],elapsed_ms,read{files,bytes,cached}}
Read. * required. Globals: --fields --raw --timeout (default 100s) --output
e.g. agent-cli lake query run 'select count(*) from orders where customer_id is null' --table orders=contosolake/raw/sales/orders/date=2026-09-29
```
