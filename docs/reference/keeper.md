# keeper — Artifact Keeper (7 commands)

| Command | Effect | Summary |
|---|---|---|
| [`keeper repo list`](#keeper-repo-list) | read | List Artifact Keeper repositories: PyPI, generic, proxies of upstream, virtual |
| [`keeper repo get`](#keeper-repo-get) | read | Show an Artifact Keeper repository: format, upstream, members, cache, size |
| [`keeper package list`](#keeper-package-list) | read | List packages in Artifact Keeper by name, or what a PyPI proxy has cached |
| [`keeper package get`](#keeper-package-get) | read | Show a package's versions: when each was published, or whether a proxy cached it |
| [`keeper version get`](#keeper-version-get) | read | Show a package version: when and by whom it was published, checksum, its files |
| [`keeper file list`](#keeper-file-list) | read | List stored files under a path prefix, with size, checksum and uploader |
| [`keeper file get`](#keeper-file-get) | read | Show a file's size, checksum and uploader, or download it with --output |

### keeper repo list

```text
agent-cli keeper repo list — List Artifact Keeper repositories: PyPI, generic, proxies of upstream, virtual
  <name> str                   Only repositories whose key or name contains this
  --format str                 Only this format: pypi, generic, npm, maven, docker, cargo, nuget …
  --type local|remote|virtual  Only hosted (local), proxy (remote) or virtual repositories
  --limit int                  Most rows to return (default 50)
Returns: [{id,name,format,type,visibility,upstream,size,created}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli keeper repo list --format pypi --fields id,type,upstream
```

### keeper repo get

```text
agent-cli keeper repo get — Show an Artifact Keeper repository: format, upstream, members, cache, size
 *<repo> str  The repository: its key, as repo list prints it, or its web, API or pip index URL
Returns: {id,name,format,type,visibility,upstream,size,created,description,quota,members[{id,type,priority}],cache_ttl,versioning,curation,updated}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli keeper repo get pypi-proxy
```

### keeper package list

```text
agent-cli keeper package list — List packages in Artifact Keeper by name, or what a PyPI proxy has cached
  <name> str    Only packages whose name contains this
  --repo str    Only this repository's (a PyPI proxy lists what it has cached)
  --format str  Only this format: pypi, generic, npm, maven …
  --limit int   Most rows to return (default 50)
Returns: [{id,repository,name,latest,format,size,downloads,updated,cached_files}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli keeper package list contoso --fields id,latest,updated
```

### keeper package get

```text
agent-cli keeper package get — Show a package's versions: when each was published, or whether a proxy cached it
 *<package> str  The package: REPO/PACKAGE, as package list prints it, PACKAGE with --repo, or its web or pip index URL
  --repo str     The repository, when PACKAGE is a bare name
  --limit int    Most versions to return (default 50)
Returns: {id,repository,name,format,type,latest,description,upstream,cached,downloads,versions[{id,version,created,size,sha256,downloads,cached,yanked,requires_python}]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli keeper package get pypi/contoso-orders-client --fields latest,versions
```

### keeper version get

```text
agent-cli keeper version get — Show a package version: when and by whom it was published, checksum, its files
 *<version> str  The version: REPO/PACKAGE@VERSION, as package get prints it, PACKAGE@VERSION with --repo, or its PyPI JSON URL
  --repo str     The repository, when VERSION names none
Returns: {id,repository,package,version,created,published_by,size,sha256,downloads,cached,yanked,requires_python,files[{id,size,sha256,content_type,created,published_by,downloads,cached,cached_at,quarantine}]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli keeper version get pypi/contoso-orders-client@2.3.0 --fields created,published_by,files
```

### keeper file list

```text
agent-cli keeper file list — List stored files under a path prefix, with size, checksum and uploader
 *<folder> str  REPO, or REPO/PREFIX for the files whose path starts with PREFIX (api/1.4.2/), or a path with --repo
  --repo str    The repository, when FOLDER is a bare path prefix
  --limit int   Most rows to return (default 50)
Returns: [{id,size,sha256,content_type,created,published_by,downloads,cached,cached_at,quarantine}]
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli keeper file list builds/api/1.4.2/ --fields id,size,created,published_by
```

### keeper file get

```text
agent-cli keeper file get — Show a file's size, checksum and uploader, or download it with --output
 *<file> str  The file: REPO/PATH, as file list and version get print it, PATH with --repo, or its download URL. --output FILE saves its bytes
  --repo str  The repository, when FILE is a bare path
Returns: {id,repository,path,package,version,size,sha256,content_type,created,published_by,downloads,origin,cached_at,cache_expires,quarantine,saved}
Read. * required. Globals: --fields --raw --timeout (default 100s) --output
e.g. agent-cli keeper file get builds/api/1.4.2/api-1.4.2-linux-x64.tar.gz --output api.tar.gz
```
