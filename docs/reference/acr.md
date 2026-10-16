# acr — Container Registry (9 commands)

| Command | Effect | Summary |
|---|---|---|
| [`acr repo list`](#acr-repo-list) | read | List container image repositories with tag counts and last push |
| [`acr tag list`](#acr-tag-list) | read | List an image repository's tags, newest first, with their digests |
| [`acr tag delete`](#acr-tag-delete) | destructive | Delete an image tag from a registry (the manifest and its other tags stay) |
| [`acr manifest get`](#acr-manifest-get) | read | Describe one image by tag or digest: size, architecture, os, tags on it |
| [`acr run list`](#acr-run-list) | read | List ACR Tasks runs, newest first: outcome, trigger and the images they pushed |
| [`acr run get`](#acr-run-get) | read | Show one ACR Tasks run: outcome, trigger, images it pushed, duration, error |
| [`acr run logs`](#acr-run-logs) | read | Read the tail of an ACR Tasks run's log, each step's output |
| [`acr registry list`](#acr-registry-list) | read | List the container registries the az login reaches (within [azure] registries) |
| [`acr registry get`](#acr-registry-get) | read | Show a container registry's SKU, access, network rules, retention and webhooks |

### acr repo list

```text
agent-cli acr repo list — List container image repositories with tag counts and last push
  <name> str        Part of the repository name, any case
  --registry str[]  Only this registry (repeatable; within [azure] registries)
  --since time      Only repositories last pushed to after this
  --until time      Only repositories last pushed to before this: stale ones
  --limit int       (default 50)
A time is 15m, 2h, 7d, 1w (ago), now-15m, 2026-09-29, or RFC 3339.
Returns: [{id,registry,repository,tag_count,manifest_count,created,updated}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli acr repo list api --since 7d --fields id,tag_count,updated
```

### acr tag list

```text
agent-cli acr tag list — List an image repository's tags, newest first, with their digests
 *<repo> str      The repository: its exact name (team/api) or id (contosoacr.azurecr.io/team/api)
  --registry str  The registry that holds it; needed when more than one does
  --limit int     (default 50)
Returns: [{id,tag,digest,created,updated}]
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli acr tag list team/api --fields id,digest,updated
```

### acr tag delete

```text
agent-cli acr tag delete — Delete an image tag from a registry (the manifest and its other tags stay)
 *<image> str     The tag: loginserver/repo:tag as acr tag list prints it, or repo:tag
  --registry str  The registry that holds it; needed when more than one does
Returns: {id,digest}
Destructive: needs --yes; --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli acr tag delete contosoacr.azurecr.io/api:v1.4.0 --yes
```

### acr manifest get

```text
agent-cli acr manifest get — Describe one image by tag or digest: size, architecture, os, tags on it
 *<image> str      The image: contosoacr.azurecr.io/team/api:1.42.0, team/api@sha256:…, or a repository
  <reference> str  A tag (1.42.0) or a digest (sha256:…), when IMAGE names only the repository
  --registry str   The registry that holds it; needed when more than one does
Returns: {digest,size,architecture,os,created,tags[],pull}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli acr manifest get contosoacr.azurecr.io/team/api:1.42.0 --fields digest,created,tags,pull
```

### acr run list

```text
agent-cli acr run list — List ACR Tasks runs, newest first: outcome, trigger and the images they pushed
  --registry str[]              Only this registry (repeatable; within [azure] registries)
  --task str                    Only runs of this task
  --status queued|started|running|succeeded|failed|canceled|error|timeout  Only runs in this state
  --limit int                   (default 50)
Returns: [{id,registry,run,task,status,trigger,images[],created,started,finished,duration,error}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli acr run list --status failed --fields id,task,status,error
```

### acr run get

```text
agent-cli acr run get — Show one ACR Tasks run: outcome, trigger, images it pushed, duration, error
 *<run> str       The run: REGISTRY/RUN as acr run list prints it, its ARM id, or RUN with --registry
  --registry str  The registry, for a bare run id when more than one is in reach
Returns: {id,registry,run,task,status,trigger,images[],created,started,finished,duration,error}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli acr run get contosoacr/ca1f --fields status,images,error
```

### acr run logs

```text
agent-cli acr run logs — Read the tail of an ACR Tasks run's log, each step's output
 *<run> str       The run: REGISTRY/RUN as acr run list prints it, its ARM id, or RUN with --registry
  --registry str  The registry, for a bare run id when more than one is in reach
  --tail int      How many of the last lines (default 200)
Returns: {id,lines,text}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli acr run logs contosoacr/ca1g --tail 50
```

### acr registry list

```text
agent-cli acr registry list — List the container registries the az login reaches (within [azure] registries)
  --limit int  (default 50)
Returns: [{id,name,login_server,subscription,resource_group,location}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli acr registry list --fields name,login_server
```

### acr registry get

```text
agent-cli acr registry get — Show a container registry's SKU, access, network rules, retention and webhooks
 *<registry> str  The registry: its name, login server, ARM id or portal link
Returns: {id,arm_id,login_server,location,sku,admin_user,anonymous_pull,public_network_access,network{default_action,allowed_ranges[],bypass},endpoints[{id,status}],retention_days,zone_redundancy,replications[{name,location,status}],webhooks[{name,status,actions[],scope}]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli acr registry get contosoacr --fields sku,admin_user,network,retention_days
```
