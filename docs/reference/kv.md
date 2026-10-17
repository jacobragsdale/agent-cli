# kv — Key Vault (12 commands)

| Command | Effect | Summary |
|---|---|---|
| [`kv secret list`](#kv-secret-list) | read | List Key Vault secrets with expiry and tags (names and metadata, never values) |
| [`kv secret get`](#kv-secret-get) | reveal | Get a Key Vault secret's value (needs --reveal, or --output FILE) |
| [`kv secret diff`](#kv-secret-diff) | read | Compare two Key Vault secrets: expiry, enabled, value same or not (never shown) |
| [`kv secret update`](#kv-secret-update) | write | Set a Key Vault secret's new value (a new version) from stdin or a file |
| [`kv version list`](#kv-version-list) | read | List a secret's versions, newest first (when it was rotated; never values) |
| [`kv certificate list`](#kv-certificate-list) | read | List Key Vault certificates and when they expire, or those due in the next days |
| [`kv certificate get`](#kv-certificate-get) | read | Show a Key Vault certificate: subject, SANs, autorenew, policy, pending renewal |
| [`kv key list`](#kv-key-list) | read | List Key Vault keys with expiry and tags (never key material) |
| [`kv key get`](#kv-key-get) | read | Show a Key Vault key's type, size, operations, expiry and rotation policy |
| [`kv vault list`](#kv-vault-list) | read | List the vaults the az login reaches (within [azure] vaults) |
| [`kv vault get`](#kv-vault-get) | read | Show a key vault's access model, network rules, endpoints and soft delete |
| [`kv vault diff`](#kv-vault-diff) | read | Compare two key vaults: names on one side only, expiry, each value same or not |

### kv secret list

```text
agent-cli kv secret list — List Key Vault secrets with expiry and tags (names and metadata, never values)
  <name> str                 Part of the secret name, any case; or a secret's id (vault/name), exactly
  --vault str[]              Only this vault (repeatable; within [azure] vaults)
  --expires-within duration  Only secrets expiring within this long, or already expired
  --expired                  Only secrets whose expiry has passed
  --disabled                 Only disabled secrets
  --tag str[]                Only secrets tagged key, or key=value, any case (repeatable)
  --content-type str         Only secrets whose content type contains this, any case
  --managed true|false       True for certificates' backing secrets only, false for none of them
  --deleted                  The deleted secrets instead: when each was deleted and will be purged
  --limit int                (default 50)
A duration is 500ms, 30s, 15m, 2h, 7d, 1w.
Returns: [{id,vault,name,enabled,content_type,expires,created,updated,managed,tags,deleted,purge}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli kv secret list db --expires-within 30d --fields id,expires
```

### kv secret get

```text
agent-cli kv secret get — Get a Key Vault secret's value (needs --reveal, or --output FILE)
 *<name> str     The secret: its name, its id (vault/name[/version]) or its URI
  --vault str    The vault that holds it; needed when more than one does
  --version str  A version id from `kv version list`; the current one when left out
Returns: {vault,name,version,content_type,value}
Reveals a secret: --reveal prints it; --output FILE saves it (0600) and prints only the path. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli kv secret get kv-contoso-dev/db-password --fields value --output db-password.txt
```

### kv secret diff

```text
agent-cli kv secret diff — Compare two Key Vault secrets: expiry, enabled, value same or not (never shown)
 *<left> str      One side: the secret's id (vault/name) or URI
 *<right> str     The other side, as left
  --as str[]      Treat a word on the left as one on the right, A=B (repeatable); none turns off the names inferred from the two ids
  --ignore str[]  Leave out a path as the changes print it, and all under it (repeatable; * is one segment)
  --all           Keep what this resource's diff leaves out as noise (ids, times, counts)
  --limit int     Most differences to print (default 50)
  --exact         Texts compare as they are: whitespace, case and comments count
Returns: {left,right,same,as[],changes[{path,left,right,redacted,same,lines[]}]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli kv secret diff kv-contoso-staging/worker-db-password kv-contoso-prod/worker-db-password
```

### kv secret update

```text
agent-cli kv secret update — Set a Key Vault secret's new value (a new version) from stdin or a file
 *<secret> str        The secret: its name, its id (vault/name) or its URI
  <value> str         - reads the new value from stdin (at most 25 KiB); never the value itself
  --value-file path   Read the new value from this file instead
  --vault str         The vault that holds it; needed when more than one does
  --content-type str  The new version's content type (text/plain …); the current one's when left out
  --expires str       When the new version expires: a length from now (90d) or a date or RFC 3339 time
Returns: {id,vault,name,version,content_type,expires,tags}
Write: --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli kv secret update kv-contoso-prod/db-password --value-file new-password.txt --expires 90d
```

### kv version list

```text
agent-cli kv version list — List a secret's versions, newest first (when it was rotated; never values)
 *<secret> str  The secret: its name, its id (vault/name) or its URI
  --vault str   The vault that holds it; needed when more than one does
  --limit int   (default 50)
Returns: [{id,version,enabled,created,updated,expires}]
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli kv version list kv-contoso-dev/db-password --fields id,created,enabled
```

### kv certificate list

```text
agent-cli kv certificate list — List Key Vault certificates and when they expire, or those due in the next days
  <name> str                 Part of the certificate name, any case; or a certificate's id (vault/name), exactly
  --vault str[]              Only this vault (repeatable; within [azure] vaults)
  --expires-within duration  Only certificates expiring within this long, or already expired
  --expired                  Only certificates whose expiry has passed
  --limit int                (default 50)
A duration is 500ms, 30s, 15m, 2h, 7d, 1w.
Returns: [{id,vault,name,subject,sans[],issuer,thumbprint,enabled,expires,not_before,updated,autorenew,tags}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli kv certificate list --expires-within 30d --fields id,subject,expires,autorenew
```

### kv certificate get

```text
agent-cli kv certificate get — Show a Key Vault certificate: subject, SANs, autorenew, policy, pending renewal
 *<certificate> str  The certificate: its name, its id (vault/name[/version]) or its URI
  --vault str        The vault that holds it; needed when more than one does
  --version str      A version id; the current one when left out
Returns: {id,vault,name,subject,sans[],issuer,thumbprint,enabled,expires,not_before,updated,autorenew,tags,version,policy{key_type,key_size,curve,reuse_key,exportable,content_type,validity_months,key_usage[],lifetime_actions[{action,lifetime_percentage,days_before_expiry}]},pending{state,details,error,request_id}}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli kv certificate get kv-contoso-prod/www-contoso-example --fields subject,expires,autorenew,pending
```

### kv key list

```text
agent-cli kv key list — List Key Vault keys with expiry and tags (never key material)
  <name> str                 Part of the key name, any case; or a key's id (vault/name), exactly
  --vault str[]              Only this vault (repeatable; within [azure] vaults)
  --expires-within duration  Only keys expiring within this long, or already expired
  --expired                  Only keys whose expiry has passed
  --limit int                (default 50)
A duration is 500ms, 30s, 15m, 2h, 7d, 1w.
Returns: [{id,vault,name,enabled,expires,created,updated,managed,tags}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli kv key list --expires-within 30d --fields id,expires
```

### kv key get

```text
agent-cli kv key get — Show a Key Vault key's type, size, operations, expiry and rotation policy
 *<key> str      The key: its name, its id (vault/name[/version]) or its URI
  --vault str    The vault that holds it; needed when more than one does
  --version str  A version id; the current one when left out
Returns: {id,vault,name,version,key_type,size,curve,operations[],enabled,expires,not_before,created,updated,managed,tags,rotation{expires_after,actions[{action,after_create,before_expiry}]}}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli kv key get kv-contoso-prod/orders-cmk --fields key_type,size,rotation
```

### kv vault list

```text
agent-cli kv vault list — List the vaults the az login reaches (within [azure] vaults)
  --limit int  (default 50)
Returns: [{id,name,subscription,resource_group,location,uri}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli kv vault list --fields name,resource_group,uri
```

### kv vault get

```text
agent-cli kv vault get — Show a key vault's access model, network rules, endpoints and soft delete
 *<vault> str  The vault: its name, URI, ARM id or portal link
Returns: {id,arm_id,uri,location,sku,authorization,access_policies[{object_id,secrets[],keys[],certificates[]}],soft_delete,soft_delete_days,purge_protection,public_network_access,network{default_action,bypass,allowed_ranges[],vnet_rules[]},endpoints[{id,status}]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli kv vault get kv-contoso-prod --fields authorization,network,endpoints
```

### kv vault diff

```text
agent-cli kv vault diff — Compare two key vaults: names on one side only, expiry, each value same or not
 *<left> str      One vault, by name
 *<right> str     The other vault, by name
  --as str[]      Treat a word on the left as one on the right, A=B (repeatable); none turns off the names inferred from the two ids
  --ignore str[]  Leave out a path as the changes print it, and all under it (repeatable; * is one segment)
  --all           Keep what this resource's diff leaves out as noise (ids, times, counts)
  --limit int     Most differences to print (default 50)
  --exact         Texts compare as they are: whitespace, case and comments count
Returns: {left,right,same,as[],changes[{path,left,right,redacted,same,lines[]}]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli kv vault diff kv-contoso-staging kv-contoso-prod
```
