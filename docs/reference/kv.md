# kv — Key Vault (4 commands)

| Command | Effect | Summary |
|---|---|---|
| [`kv secret list`](#kv-secret-list) | read | List Key Vault secrets with expiry and tags (names and metadata, never values) |
| [`kv secret get`](#kv-secret-get) | reveal | Get a Key Vault secret's value (needs --reveal, or --output FILE) |
| [`kv version list`](#kv-version-list) | read | List a secret's versions, newest first (when it was rotated; never values) |
| [`kv vault list`](#kv-vault-list) | read | List the key vaults the az login reaches (within [azure] vaults) |

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
  --limit int                (default 50)
A duration is 500ms, 30s, 15m, 2h, 7d, 1w.
Returns: [{id,vault,name,enabled,content_type,expires,created,updated,managed,tags}]
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

### kv vault list

```text
agent-cli kv vault list — List the key vaults the az login reaches (within [azure] vaults)
  --limit int  (default 50)
Returns: [{name,subscription,resource_group,location,uri}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli kv vault list --fields name,resource_group,uri
```
