# Plan: the redis domain

Redis as a read-only domain: keys and their values, when they expire, and
whether an instance is healthy and how full it is. It reads Azure Cache for
Redis, Azure Managed Redis and self-hosted Redis 7+, one instance per
project and environment (`orders-qa`, `orders-prod`), none of them
clustered. Valkey speaks the same protocol and needs nothing more.

## Why it earns a domain

Agents know `redis-cli`. Its defaults are the losses add-a-domain's second
test looks for:

- `KEYS *` blocks a single-threaded prod instance for seconds; `FLUSHALL`,
  `DEL` and `CONFIG SET` are one wrong guess away.
- `GET` on a 20 MB value, `HGETALL` on a 100k-field hash or `LRANGE 0 -1`
  print far past 30 KB.
- `redis-cli -a PASSWORD` puts the password in the transcript; Azure's
  Entra sign-in needs a token minted by hand and the principal's object id.
- `TTL` is seconds from now, not a time; `INFO` is ~150 lines of text whose
  meaning (is 0 maxmemory unbounded? are evictions happening?) is the
  answer.
- qa against prod is two sessions and a comparison done by eye.

Sign-in is one section with a list of instances, `doctor` checks each with
`PING`, and there are seven commands.

## Step 0: the trial, before code

`scripts/db-up.sh` gains two `redis:7` containers (qa on 6379, prod on 6380)
and `scripts/seed/redis.py` fills them: 200k keys in real patterns
(`order:{id}`, `session:{guid}`, `config:feature-flags`), TTLs mixed with
persistent keys, a 20 MB string, a 100k-field hash, a list, set, sorted set
and stream, values as the apps write them (JSON, gzip and zlib JSON, a zstd
blob, a Django-style pickle at `:1:report:7`, a .NET `IDistributedCache`
hash), and `config:feature-flags` differing between the
two. Six tasks against `redis-cli`:

1. When does `session:…` expire, and when was it last read?
2. Which keys hold order 4417?
3. Is prod near full, and what is using the memory?
4. Does `config:feature-flags` differ between qa and prod?
5. What were the slow commands in the last hour?
6. Which client holds the most connections?

What counts is any `KEYS`, any answer over 30 KB, call counts and wrong
answers (a TTL misread as a time, 0 maxmemory read as full). If `redis-cli`
does as well, the domain does not get built.

## Commands

Every command is `Effect::Read` and keeps core's 60 s. The calls are RESP
commands in one pipelined round trip where they do not depend on each other.

| Command | Calls | Answers |
|---|---|---|
| `redis instance list` | per instance: `INFO server memory stats clients replication keyspace` | one row each: `id`, host, `ok` or `error`, version, role, `used_pct`, `evicted_keys`, `clients`, `keys`. An unreachable instance is a row with `error`, not a failure. |
| `redis instance get [NAME]` | `INFO` (default sections) | version, role, `started_at`; memory (used, peak, `maxmemory`, `free`, `used_pct`, rss, fragmentation, `maxmemory_policy`); keys per db (keys, with expiry, average TTL); stats since start (hits, misses, `hit_ratio`, `evicted_keys`, `expired_keys`, ops/s, rejected connections); clients (connected, blocked, max); persistence (`last_save_at`, last bgsave status, AOF on); replication (role, replicas, link status, lag). |
| `redis key list [--match P] [--type T] [--by size [--sample N]] [--limit 50]` | `DBSIZE`; `SCAN c MATCH P COUNT 1000 TYPE T` until `--limit` rows, cursor 0 or the deadline; per page, pipelined `TYPE`, `PEXPIRETIME`, `MEMORY USAGE`, `OBJECT IDLETIME` | rows: `id` (the key), type, `size`, `expires_at` (absent when persistent), `last_access`. `--by size` scans up to `--sample` keys (10,000) and prints the largest. |
| `redis key get KEY [--limit 50]` | `TYPE`, `PEXPIRETIME`, `MEMORY USAGE`, `OBJECT ENCODING`, `OBJECT IDLETIME`, then by type: `STRLEN` + `GETRANGE` of up to 1 MB; `HLEN` + `HSCAN`; `LLEN` + `LRANGE`; `SCARD` + `SSCAN`; `ZCARD` + `ZRANGE … WITHSCORES`; `XLEN` + `XREVRANGE … COUNT` | type, encoding, `length`, `size`, `expires_at`, `last_access`, `value`, decoded as Values says. A value past 8 KB decoded prints its first 8 KB with `truncated: true` and a note naming `--output FILE`, which saves the whole value. A collection prints its first `--limit` items, long strings cut as `aisearch document list` cuts them. |
| `redis key diff A B` | `key get`'s reads on each side, the whole value up to 1 MB a side (a larger one compares by length, noted) | core's `diff`: sides `[INSTANCE:]KEY`, environments inferred from the two instance names. Compared: type, `compression`, the decoded value (gzip JSON in qa and plain JSON in prod compare by content; JSON as JSON, text by lines, a hash keyed by field, a set as a set, a sorted set keyed by member), and whether it expires at all. Noise: `size`, encoding, `last_access`, the `expires_at` time itself. |
| `redis slowlog list [--since] [--limit 50]` | `SLOWLOG GET 128` | rows: `at` (UTC), `duration_ms`, `command` (its name), `key` (the first key argument, when the command has one), `client`, `client_name`. Other arguments are values and never print. |
| `redis client list [--limit 50]` | `CLIENT LIST` | rows: `addr`, `name`, `user`, `db`, `connected_at`, `last_command_at`, `command`, flags, our own connection left out. |

A missing key is exit 4 with a hint naming `agent-cli redis key list
--match PATTERN`. `instance get` notes `[next: agent-cli redis key list
--instance NAME --by size]` when it is evicting or above 90% of maxmemory.
With maxmemory 0 (unbounded, self-hosted), `free` is the host's memory less
what Redis uses, and the row says `maxmemory: unbounded`. `INFO`'s counters
start at `started_at`; trends over days are dd's or Azure Monitor's.

Lifespan: Redis keeps no creation time. `expires_at` comes from
`PEXPIRETIME`, `last_access` from `OBJECT IDLETIME`; under an LFU
`maxmemory_policy` idle time is not tracked, so `access_frequency` from
`OBJECT FREQ` takes its place. A sliding expiry the application renews on
read looks like any other TTL, except in a .NET entry, which says so.

## Values

Most caches are not compressed: redis-py, Django's cache, django-redis,
Flask-Caching and .NET's `IDistributedCache` store values as they are by
default, and teams add compression when values grow. So `key get` and `key
diff` decode by what the bytes are, never by configuration, in this order:

1. A hash of exactly `absexp`, `sldexp` and `data` is a .NET
   `IDistributedCache` entry: `absolute_expires_at` and `sliding` (a `Span`)
   from its ticks, and `data` decoded by the steps below. Any other hash is
   a hash.
2. gzip (`1f 8b`) or zlib (`78` then `01`, `5e`, `9c` or `da`) is
   decompressed, up to 1 MB, and named in `compression`. A value that fails
   to decompress is taken as it is: a text value may begin with `x^`.
3. JSON when it parses whole, else UTF-8 text.
4. A pickle (`80` then a protocol byte, 2 to 5) is `format: "pickle"` with
   no value and a note naming `--output FILE` and `python -m pickletools
   FILE`, which disassembles it without running it. `pickle.loads` would
   run the classes a pickle names, so nothing loads one.
5. zstd (`28 b5 2f fd`), an lz4 frame (`04 22 4d 18`) and xz (`fd 37 7a 58
   5a 00`) are named in `compression` and print as base64; their pure-Rust
   decoders (ruzstd, lz4_flex) come when a real value shows one. Anything
   else is base64 with `binary: true`: MessagePack, snappy and brotli carry
   no magic bytes, so they land here.

## Ids

- An instance is its `name`. `--instance` defaults to the only one through
  `pick`, and also takes a configured instance's host, `host:port`, a
  `redis://` or `rediss://` URL or a StackExchange.Redis connection string
  (`HOST:PORT,password=…,ssl=True`), matched against the config and never
  connected to otherwise (exit 2). An app's connection string from `k8s
  secret get` or Key Vault leads straight to the instance.
- A key is itself, `id` on every row, what `key get` takes with
  `--instance`. A key that is not UTF-8 prints with `\xNN` escapes, which
  `key get` reads back.
- A diff side is `INSTANCE:KEY` when the text before the first colon names
  a configured instance, else `KEY` on `--instance`. Instance names may not
  hold a colon; the config refuses one.

## Config and credential

```toml
[[redis.instance]]
name = "orders-qa"
host = "contoso-orders-qa.redis.cache.windows.net"
port = 6380            # default 6379
tls = true             # default false
auth = "az"            # Entra: an az token, the principal's object id as user
db = 0                 # default 0

[[redis.instance]]
name = "orders-prod"
host = "contoso-orders.eastus.redis.azure.net"   # Azure Managed Redis
port = 10000
tls = true
auth = "az"

[[redis.instance]]
name = "sessions-qa"
host = "redis-qa.contoso.internal"
user = "agent"         # an ACL user; absent is `default`
password_cmd = "pass show redis/sessions-qa"
trust_cert = true      # accept a self-signed certificate
```

- `auth = "az"`: `ctx.az_token("https://redis.azure.com/", …)`, sent as
  `AUTH OID TOKEN`, the object id read from the token's `oid` claim. The
  token goes only to a host under `redis.cache.windows.net` or
  `redis.azure.net`; any other host with `auth = "az"` is exit 3 naming the
  key. A `WRONGPASS` mints once more, as a 401 does.
- Access keys (Azure Cache for Redis): `password_cmd = "az redis list-keys
  -g RG -n NAME --query primaryKey -o tsv"`, no user.
- Otherwise `password`, `password_env`, `password_cmd` with an optional
  `user`, resolved only when a connection opens; no credential is an open
  dev server. The password goes only to the instance's own host.

TLS is rustls 0.23 with `rustls-platform-verifier` 0.7, both already built
for ureq, and gzip and zlib are flate2 1.1, also ureq's: three direct
dependency lines, nothing new compiled. No `redis`
crate: read-only RESP2 over a `TcpStream` is a writer of arrays and a parser
of five reply types.

## Read-only by construction

The door, `Session::call` in `client.rs`, sends only commands on an
allowlist (`AUTH`, `HELLO`, `PING`, `SELECT`, `CLIENT SETNAME`, `INFO`,
`DBSIZE`, `SCAN`, `TYPE`, `PEXPIRETIME`, `MEMORY USAGE`, `OBJECT`,
`STRLEN`, `GETRANGE`, `HLEN`, `HSCAN`, `LLEN`, `LRANGE`, `SCARD`, `SSCAN`,
`ZCARD`, `ZRANGE`, `XLEN`, `XREVRANGE`, `SLOWLOG GET`, `CLIENT LIST`, `CLIENT ID`); any
other is refused. A database test asks the server's
`COMMAND INFO` for each and fails on a `write` flag. `CLIENT SETNAME
agent-cli` touches only our connection, so admins see who is reading. At
work the credential should also be read-only: an ACL user with `+@read`
and the few admin reads, or an Azure access policy that grants them.

Every session is an `Op` through `ctx.read`; socket timeouts are
`ctx.remaining()`, and a `SCAN` stopped by the deadline notes how far it got
(`[3 matches in 1.2M of 5.0M keys scanned; narrow --match or raise
--timeout]`). A `MOVED` reply (a clustered cache) fails, saying clusters are not
supported.

## Search

- `key` is kv's resource too (a Key Vault key): it joins `SHARED_WORDS` with
  labeled queries for each reading ("redis keys matching session" →
  `redis key list`; "key vault keys" → `kv key list`).
- Synonyms: cache → the domain; ttl, expiry, expire, expires → `key`;
  memory, health, free space, evictions, info → `instance`; slow → `slowlog`;
  connections, connected → `client`. `check_registry` settles the rest.
- Status: `redis orders-qa` or `redis 4 instances`.

## Tests and fixtures

No HTTP, so no world recording (sql has none either). A scripted session in
`testing.rs` answers each command from a table, so handler tests run in CI:
INFO parsing, the scan's stop rules, value decoding, side parsing, the host
check for az tokens, the allowlist. `crates/redis/tests/redis.rs` runs
every command against the two seeded containers under
`AGENT_CLI_TEST_DBS=1`, and trials use the same containers.

## Joins

- From k8s or kv: the connection string an app is configured with names the
  instance (`--instance` takes it).
- To k8s: `client list`'s `addr` on AKS is a pod IP; joining it to a pod
  waits for a k8s lookup by IP.
- To arm: the Azure side (SKU, firewall, private endpoint, persistence) stays
  `arm resource get`; `arm query run` finds a cache by host name.

## Order of work

1. Crate, config, door (RESP, TLS, auth), doctor, `instance list`,
   `instance get`.
2. `key list`, `key get`, `key diff`.
3. `slowlog list`, `client list`, search queries, docs, the trial's six
   tasks against agent-cli.

## Open questions

Python writes most of the caches and .NET some, no project uses RedisJSON,
and no one knows yet whether values are compressed.

1. Which Python libraries write the caches? redis-py with `json.dumps`
   needs nothing more; Django's cache and Flask-Caching pickle by default.
   If pickles are common at work, a pickle reader that never runs code
   (builtins as JSON, a class as its name and arguments) replaces the
   pickletools note.
2. VERIFY(work): which commands each Azure kind allows its principal.
   Azure's built-in "Data Reader" access policy may refuse `INFO`,
   `SLOWLOG` and `CLIENT LIST`; a custom policy may be needed for
   `instance get`. Azure Managed Redis may leave `rdb_*` and some memory
   fields out of `INFO`, so every field is optional.
3. VERIFY(work): the Entra user for `AUTH` is the token's `oid` on both
   Azure kinds.
