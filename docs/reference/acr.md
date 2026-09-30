# acr — Container Registry (4 commands)

| Command | Effect | Summary |
|---|---|---|
| [`acr repo list`](#acr-repo-list) | read | List container image repositories with tag counts and last push |
| [`acr tag list`](#acr-tag-list) | read | List an image repository's tags, newest first, with their digests |
| [`acr manifest get`](#acr-manifest-get) | read | Describe one image by tag or digest: size, architecture, os, tags on it |
| [`acr registry list`](#acr-registry-list) | read | List the container registries the az login reaches (within [azure] registries) |

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

### acr registry list

```text
agent-cli acr registry list — List the container registries the az login reaches (within [azure] registries)
  --limit int  (default 50)
Returns: [{name,login_server,subscription,resource_group,location}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli acr registry list --fields name,login_server
```
