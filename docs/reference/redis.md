# redis — Redis caches (7 commands)

| Command | Effect | Summary |
|---|---|---|
| [`redis key list`](#redis-key-list) | read | List Redis keys matching a pattern, with type, size and expiry; or the largest |
| [`redis key get`](#redis-key-get) | read | Show a Redis key: value decoded, type, size, when it expires and was last used |
| [`redis key diff`](#redis-key-diff) | read | Compare a Redis key in two instances (qa and prod): value decoded, type, expiry |
| [`redis instance list`](#redis-instance-list) | read | List the configured Redis instances, each with its memory, evictions and clients |
| [`redis instance get`](#redis-instance-get) | read | Show a Redis instance's health: memory used and free, evictions, hit ratio |
| [`redis slowlog list`](#redis-slowlog-list) | read | List the slow commands a Redis instance logged, newest first, with their keys |
| [`redis client list`](#redis-client-list) | read | List who is connected to a Redis instance: address, name, idle, last command |

### redis key list

```text
agent-cli redis key list — List Redis keys matching a pattern, with type, size and expiry; or the largest
  <pattern> str                 Keys matching this glob: session:*, orders:44?? (default: every key)
  --instance str                Instance name from instance list (or its host, URL or connection string); defaults to the only one
  --type string|hash|list|set|zset|stream  Only keys of this type
  --sort key|size|expires       key: the first matches, by name; size or expires: the largest or the soonest to expire among --sample matches (default key)
  --sample int                  Matching keys --sort size or expires ranks (default 50000)
  --limit int                   Most rows to return (default 50)
Returns: [{id,type,size_bytes,expires_at,last_access}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli redis key list 'session:*' --instance orders-qa --fields id,type,expires_at
```

### redis key get

```text
agent-cli redis key get — Show a Redis key: value decoded, type, size, when it expires and was last used
 *<key> str       The key, as key list prints it (\xNN for a byte that is not UTF-8)
  --instance str  Instance name from instance list (or its host, URL or connection string); defaults to the only one
  --limit int     Most members of a hash, list, set, sorted set or stream to print (default 50)
Returns: {id,type,encoding,length,size_bytes,expires_at,ttl_s,last_access,access_frequency,envelope,absolute_expires_at,sliding_s,compression,format,truncated,value}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli redis key get session:7f3a --instance orders-qa --fields type,expires_at,value
```

### redis key diff

```text
agent-cli redis key diff — Compare a Redis key in two instances (qa and prod): value decoded, type, expiry
 *<left> str      One side: [INSTANCE:]KEY (orders-qa:config:flags); no prefix is --instance's
 *<right> str     The other side, as left
  --instance str  Instance name from instance list; defaults to the only one
  --as str[]      Treat a word on the left as one on the right, A=B (repeatable); none turns off the names inferred from the two ids
  --ignore str[]  Leave out a path, such as containers.*.resources (repeatable; * is one segment)
  --all           Keep what is noise for this resource (uids, timestamps, status)
  --limit int     Most differences to print (default 50)
  --exact         Texts compare as they are: whitespace, case and comments count
Returns: {left,right,same,as[],changes[{path,left,right,redacted,same,lines[]}]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli redis key diff orders-qa:config:feature-flags orders-prod:config:feature-flags
```

### redis instance list

```text
agent-cli redis instance list — List the configured Redis instances, each with its memory, evictions and clients
Returns: [{id,host,version,role,used_bytes,used_pct,maxmemory_bytes,evicted_keys,clients,keys,error}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli redis instance list --fields id,used_pct,evicted_keys,error
```

### redis instance get

```text
agent-cli redis instance get — Show a Redis instance's health: memory used and free, evictions, hit ratio
  <name> str  Instance name from instance list (or its host, URL or connection string); defaults to the only one
Returns: {id,host,version,mode,role,started_at,memory{used_bytes,free_bytes,used_pct,bounded_by,maxmemory_bytes,policy,peak_bytes,rss_bytes,fragmentation},keyspace[{db,keys,expiring,avg_ttl_s}],stats{hit_ratio,hits,misses,evicted_keys,expired_keys,ops_per_sec,rejected_connections},clients{connected,blocked,max},persistence{last_save_at,last_bgsave,changes_since_save,aof},replication{replicas,master,link,last_io_s}}
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli redis instance get orders-prod --fields memory,stats.evicted_keys
```

### redis slowlog list

```text
agent-cli redis slowlog list — List the slow commands a Redis instance logged, newest first, with their keys
  --instance str        Instance name from instance list (or its host, URL or connection string); defaults to the only one
  --since time          Only commands logged since (15m, 2h, 7d, a date)
  --sort time|duration  time: newest first; duration: slowest first (default time)
  --limit int           Most rows to return (default 50)
A time is 15m, 2h, 7d, 1w (ago), now-15m, 2026-09-29, or RFC 3339.
Returns: [{at,duration_ms,command,key,client,client_name}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli redis slowlog list --instance orders-prod --since 1h --sort duration --fields at,duration_ms,command,key
```

### redis client list

```text
agent-cli redis client list — List who is connected to a Redis instance: address, name, idle, last command
  --instance str  Instance name from instance list (or its host, URL or connection string); defaults to the only one
  --limit int     Most rows to return (default 50)
Returns: [{addr,name,user,db,connected_at,last_command_at,command,flags}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli redis client list --instance orders-prod --fields addr,name,last_command_at
```
