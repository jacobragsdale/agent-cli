# redis: Redis caches, read only

Keys, values, expiry and instance health on Azure Cache for Redis, Azure
Managed Redis and self-hosted Redis (Valkey too), over RESP2 on a
`TcpStream` with rustls: https://redis.io/docs/latest/commands/

## Config
`[[redis.instance]]`: `name` (what `--instance` takes; no colon), `host`,
`port` (6380 on `*.redis.cache.windows.net`, 10000 on `*.redis.azure.net`,
else 6379), `tls` (default true on those hosts), `trust_cert`, `db`,
`auth` (`az`: a token for `AZURE_REDIS`, sent only to those hosts, signed in
as the token's `oid` unless `user` names one), `user` (an ACL user).
Credential: `password`, `password_env` or `password_cmd`, resolved only
when a connection opens.

## Ids
An instance is its `name`; `--instance` also takes its host, `host:port`,
a `redis://` URL or a StackExchange.Redis connection string, matched
against the config (`Redis::instance`), never connected to otherwise. A key
is itself, a byte that is not UTF-8 as `\xNN` (`escape`, `unescape`). A
diff side is `INSTANCE:KEY` when the text before the first colon is an
instance's name, else `KEY` on `--instance`.

## Where things are (`src/`)
A command is `<resource>/<verb>.rs`. Copy a sibling.
- `lib.rs`: `DOMAIN`. `doctor.rs`: status and doctor (`PING`).
- `session.rs`: the door. `Session::open` (TCP, TLS, `AUTH`, `SELECT`),
  `pipeline`, `call`, `refused` (Redis's error to a `Failure`), `READS`
  (the only commands it sends), `cmd!`, `Reply`, `OnInstance` and `on`
  (the `Op` every command performs through `ctx.read`).
- `config.rs`: `Redis::load`, `Instance`, `credentials`.
- `instance/mod.rs`: `Info` (`INFO` parsed: `memory`, `stats`,
  `needs_room` …).
- `key/mod.rs`: `read` (one key, as far as a `Reach` goes), `decode` (the
  value by its bytes: IDistributedCache, gzip, zlib, JSON, text, pickle,
  zstd, lz4, xz, base64), `member`, `Key`.
- `testing.rs`: `serve` (a scripted RESP server on a port of its own;
  `sent()`), `config`, `redis`, `bulk`, `array`.

## Fixtures
No world recording. Handler tests run against `serve`. The database tests,
`crates/redis/tests/redis.rs` and the `COMMAND INFO` check of `READS`,
need `scripts/redis-up.sh` (two seeded Redis 7.4 containers: qa with a
read-only ACL user, prod with TLS and LFU; `scripts/seed/redis.py`) and
`AGENT_CLI_TEST_REDIS=1`. Queries `crates/redis/search.toml`.

## Quirks
- Never `KEYS`: `key list` scans, stopped by `--limit`, `--sample` or the
  deadline, and says how far it got.
- Expiry is `PTTL` from now, which Redis 6 (Azure Cache) has too; Redis
  keeps no creation time. `OBJECT IDLETIME` fails under an LFU policy, then
  `OBJECT FREQ` answers.
- A pickle is named, never loaded; `--output FILE` saves a string whole,
  decompressed.
- `slowlog list` prints a key only for commands whose first argument is one
  (`KEY_FIRST`): any other argument can be a value.

## Never needed
Other crates' sources, `docs/plans/`, `docs/reference/`, `fixtures/world/`.
