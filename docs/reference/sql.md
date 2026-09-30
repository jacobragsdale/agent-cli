# sql — SQL Server/Oracle (6 commands)

| Command | Effect | Summary |
|---|---|---|
| [`sql query run`](#sql-query-run) | read or write | Run SQL on a connection and return every result set as JSON |
| [`sql query bench`](#sql-query-bench) | read or write | Time a query over several runs: connect, first row and total latency |
| [`sql object list`](#sql-object-list) | read | List tables and views, procedures, functions, packages and sequences |
| [`sql object get`](#sql-object-get) | read | Show a view, procedure, function or package's source, or a table's columns |
| [`sql schema list`](#sql-schema-list) | read | List the schemas (owners) of a database |
| [`sql connection list`](#sql-connection-list) | read | List the configured database connections (never their passwords) |

### sql query run

```text
agent-cli sql query run — Run SQL on a connection and return every result set as JSON
  <sql> str        The SQL, or - to read it from stdin. SQL Server splits at GO lines, Oracle at ; and / lines
  --conn str       Connection name from `sql connection list`; defaults to the only one
  --max-rows int   Keep at most this many rows of each result set (default 1000)
  --sql-file path  The SQL from a file
Returns: {results[{columns[],types[],rows[],rows_affected,truncated}],elapsed_ms}
Read or write, decided by the input: a write honours --dry-run and may need --yes. Globals: --fields --raw --timeout --output
e.g. agent-cli sql query run --conn local-mssql 'select top 5 id, name from bench.customers'
```

### sql query bench

```text
agent-cli sql query bench — Time a query over several runs: connect, first row and total latency
  <sql> str        The SQL, or - to read it from stdin
  --conn str       Connection name from `sql connection list`; defaults to the only one
  --runs int       How many times to run it on one connection (default 20)
  --max-rows int   Keep at most this many rows of each result set, ending the read there (default: read every row each run)
  --sql-file path  The SQL from a file
Returns: {runs,requested,rows,phases[{phase,min_ms,p50_ms,p95_ms,max_ms}]}
Read or write, decided by the input: a write honours --dry-run and may need --yes. Globals: --fields --raw --timeout --output
e.g. agent-cli sql query bench --conn local-mssql --runs 5 'select count(*) from bench.orders'
```

### sql object list

```text
agent-cli sql object list — List tables and views, procedures, functions, packages and sequences
  <pattern> str                 Only names containing this, any case
  --conn str                    Connection name from `sql connection list`; defaults to the only one
  --schema str                  Only this schema (owner on Oracle)
  --kind table|view|procedure|function|package|sequence  Only this kind
  --limit int                   (default 50)
Returns: [{id,schema,kind,name,modified}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli sql object list --conn local-mssql customer --kind table --fields schema,name
```

### sql object get

```text
agent-cli sql object get — Show a view, procedure, function or package's source, or a table's columns
 *<object> str  schema.name; either part may be quoted as [x] or "x"
  --conn str    Connection name from `sql connection list`; defaults to the only one
Returns: {kind,schema,name,text,columns[{name,type,nullable,pk}],ddl}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli sql object get --conn local-mssql bench.customers --fields kind,columns,ddl
```

### sql schema list

```text
agent-cli sql schema list — List the schemas (owners) of a database
  --conn str   Connection name from `sql connection list`; defaults to the only one
  --limit int  (default 50)
Returns: [str]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli sql schema list --conn local-mssql
```

### sql connection list

```text
agent-cli sql connection list — List the configured database connections (never their passwords)
Returns: [{name,kind,host,port,database,service,user,read_only}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli sql connection list --fields name,kind,host,read_only
```
