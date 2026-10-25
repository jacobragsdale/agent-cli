# sql: SQL Server, Oracle and Postgres

Queries, objects and schemas: SQL Server over tiberius, Oracle over the
`oracle` crate and the Instant Client, Postgres over tokio-postgres.

## Config
`[sql]`: `oracle_client_dir` (or `AGENT_CLI_SQL_ORACLE_CLIENT_DIR`),
`read_only` (core's). `[[sql.connection]]`: `name` (`--conn`, via `pick`),
`kind` (`mssql`, `oracle`, `postgres`), `host`, `port`, `database` (not
oracle), `service` (oracle), `user`, `trust_cert`, `encrypt` (not oracle;
TLS required unless false), `auth` (`az`: `AZURE_SQL`, or on postgres
`AZURE_POSTGRES` as `user`'s password), `read_only` (beats the section's),
`password` or its `_env`, `_cmd`, `_kv`. `Session::sign_in` retries a
refused login (18456, ORA-01017, 28P01) once with `Credential::refresh`.
`Sql::connection` enters the scope `ctx.write` checks; read-only, Oracle
reads in `SET TRANSACTION READ ONLY`, Postgres starts read-only.

## Ids
An object is `schema.name`, folded the way each server folds names (Oracle
upper-cases unquoted ones, Postgres lower-cases them; SQL Server compares
case-insensitively, an exact match winning). A connection is its `name`.
On Postgres a materialized view is a `view` and a function's overloads one.

## Where things are (`src/`)
A command is `<resource>/<verb>.rs`: its args, rows, handler, `command!`
and tests (`query/run.rs` is `sql query run`). Copy a sibling.
- `lib.rs`: `DOMAIN`. `doctor.rs`. `testing.rs`:
  `sql(argv, setup)` over a `CONFIG` whose servers listen on port 9.
- `query/mod.rs`: what run and bench share. `catalog.rs`: the catalog SQL
  (`objects_sql`, `catalog_read`, `ObjectRow`, …); `catalog/detail.rs`:
  the diffs' (`table_row`); `catalog/postgres.rs`: Postgres's.
- `config.rs`: `Sql::load(config)`, `connection(ctx, name)`, `Kind`.
- `db.rs`: `OnConnection` (the `Op` every sql command performs through
  `ctx.read` / `ctx.write`), `Session::run(sql, Fetch, deadline)`,
  `bounded`, cell helpers `int`, `decimal`, `text`, `maybe`, `whole`.
- `split.rs`: cuts a script (`GO`; `;` and `/`; `;` outside `$$` and
  `begin atomic`) and classifies statements.
- `mssql.rs`, `oracle.rs`, `postgres.rs`: drivers.

## Fixtures
No world recording. `tests/dbs.rs` needs `scripts/db-up.sh` and
`AGENT_CLI_TEST_DBS=1`, else skips. Queries `crates/sql/search.toml`.

## Quirks
- The classifier decides the effect: a batch holding any write goes through
  `ctx.write` as destructive.
- `panic = "unwind"` stays: tiberius panics on some column types
  (`sql_variant`); `mssql.rs` catches that as a failed query.
- Stopping at `--max-rows` drops a SQL Server or Postgres socket, ending the
  session: the next statement reconnects, `bench` says. So only a read
  with no write before it is cut; an open transaction is rolled back, said.
- A signal or deadline ends the server's work: SQL Server's with the
  socket, Oracle's by a break, Postgres's by a cancel request.
- Postgres: a prepare for the types, then a simple query (values as text).

## Never needed
Other crates' sources, `docs/plans/`, `docs/reference/`, `fixtures/world/`.
