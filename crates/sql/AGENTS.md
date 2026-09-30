# sql: SQL Server and Oracle

Queries, objects and schemas, ported from sql-bench. SQL Server over
tiberius (https://docs.rs/tiberius), Oracle over the `oracle` crate and the
Instant Client (https://docs.rs/oracle).

## Config
`[sql]`: `oracle_client_dir` (optional; `AGENT_CLI_SQL_ORACLE_CLIENT_DIR`
overrides it). `[[sql.connection]]`: `name` (what `--conn` takes; defaults
to the only one through `pick`), `kind` (`mssql` or `oracle`), `host`,
`port`, `database` (mssql), `service` (oracle), `user`, `trust_cert`,
`encrypt`, `read_only`. Credential: `password`, `password_env` or
`password_cmd`, resolved only when a connection opens. No HTTP, so no host
rule: the password goes only to the connection's own server.

## Ids
An object is `schema.name`, folded the way each server folds names (Oracle
upper-cases unquoted ones; SQL Server compares case-insensitively, an exact
match winning). A connection is its `name`.

## Where things are (`src/`)
A command is `<resource>/<verb>.rs`: its args, rows, handler, `command!`
and tests (`query/run.rs` is `sql query run`). Copy a sibling.
- `lib.rs`: `DOMAIN`, whose `commands` registers every command (its order
  is the listing's). `doctor.rs`: status and doctor. `testing.rs`: the
  tests' `sql(argv, setup)` over a `CONFIG` whose servers listen on port 9.
- `query/mod.rs`: what run and bench share (`statements`, `plan`, `door`,
  `failed_at`). `catalog.rs`: the catalog SQL the object and schema
  commands share (`objects_sql`, `catalog_read`, `ObjectRow`, …).
- `config.rs`: `Sql::load(config)`, `connection(name)`, `Connection`, `Kind`.
- `db.rs`: `OnConnection` (the `Op` every sql command performs through
  `ctx.read` / `ctx.write`), `Session::run(sql, Fetch, deadline)`,
  `bounded`, cell helpers `int`, `decimal`, `text`, `maybe`, `whole`.
- `split.rs`: cuts a script (`GO`; `;` and `/`) and classifies statements.
- `mssql.rs`, `oracle.rs`: the drivers. `tests/dbs.rs`: the compose DBs.

## Fixtures
No world recording (trials have no database). Integration tests need
`scripts/db-up.sh`, then `AGENT_CLI_TEST_DBS=1 cargo test -p agent-cli-sql`;
without the variable they skip. Queries `crates/sql/search.toml`.

## Quirks
- The classifier decides the effect: a batch holding any write goes through
  `ctx.write` as destructive, and a `read_only` connection refuses it.
- `panic = "unwind"` stays in the release profile: tiberius panics on some
  column types (`sql_variant`) and `mssql.rs` catches that as a failed query.
- ODPI-C `dlopen`s the Instant Client on the first Oracle connection, so
  the binary runs without it; doctor reports whether it loaded.
- Stopping at `--max-rows` drops the SQL Server socket, which ends the
  session: the next statement reconnects, and `bench` names the reconnects.
- The only async code (a current-thread runtime for tiberius) is here.

## Never needed
Other crates' sources, `PLAN.md`, `docs/plans/`, `docs/reference/`,
`fixtures/world/`.
