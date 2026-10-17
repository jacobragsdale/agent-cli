# lake — Data Lake Storage (9 commands)

| Command | Effect | Summary |
|---|---|---|
| [`lake account list`](#lake-account-list) | read | List the Data Lake storage accounts the az login reaches |
| [`lake container list`](#lake-container-list) | read | List Data Lake containers with their top-level folders (the data domains) |
| [`lake path list`](#lake-path-list) | read | List the files and folders under a Data Lake folder, with their total size |
| [`lake path get`](#lake-path-get) | read | Show a Data Lake file's size, time, owner, ACL, and Parquet schema and rows |
| [`lake table list`](#lake-table-list) | read | List the Parquet tables under a Data Lake folder or in every data domain |
| [`lake table get`](#lake-table-get) | read | Show a Parquet table's schema, rows, size and last write, read from its footers |
| [`lake table diff`](#lake-table-diff) | read | Compare two Parquet tables, partitions or files: their schema (drift) and types |
| [`lake partition list`](#lake-partition-list) | read | List a Parquet table's partitions with files, rows and writes; note missing days |
| [`lake access get`](#lake-access-get) | read | Say whether a principal can read or write a Data Lake path, and where it breaks |

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
Returns: [{id,partitioned_by[],files,modified,bulk}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli lake table list contosolake/raw --fields id,partitioned_by,files,modified
```

### lake table get

```text
agent-cli lake table get — Show a Parquet table's schema, rows, size and last write, read from its footers
 *<path> str  The table's folder: an id from lake table list, or its abfss:// or https:// URL
Returns: {id,partitioned_by[],schema[{name,type}],files,size,rows,modified,newest,bulk}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli lake table get contosolake/curated/finance/ledger --fields schema,rows,modified
```

### lake table diff

```text
agent-cli lake table diff — Compare two Parquet tables, partitions or files: their schema (drift) and types
 *<left> str      One side: a table's or partition's folder, or a Parquet file (an id or its URL)
 *<right> str     The other side, as left
  --as str[]      Treat a word on the left as one on the right, A=B (repeatable); none turns off the names inferred from the two ids
  --ignore str[]  Leave out a path, such as containers.*.resources (repeatable; * is one segment)
  --all           Keep what is noise for this resource (uids, timestamps, status)
  --limit int     Most differences to print (default 50)
  --exact         Texts compare as they are: whitespace, case and comments count
Returns: {left,right,same,as[],changes[{path,left,right,redacted,same,lines[]}]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli lake table diff contosolake/raw/sales/orders/date=2026-09-28 contosolake/raw/sales/orders/date=2026-09-29
```

### lake partition list

```text
agent-cli lake partition list — List a Parquet table's partitions with files, rows and writes; note missing days
 *<path> str    The table's folder: an id from lake table list, or its abfss:// or https:// URL
  --since time  Only partitions written to since then
  --limit int   (default 50)
A time is 15m, 2h, 7d, 1w (ago), now-15m, 2026-09-29, or RFC 3339.
Returns: [{id,values,files,size,rows,modified,bulk}]
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
