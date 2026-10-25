# keeper: Artifact Keeper

Repositories, packages, versions and files on a self-hosted Artifact Keeper
(artifactkeeper.com, an Artifactory/Nexus alternative), read only, over
`/api/v1` and a PyPI repository's JSON API (`/pypi/KEY/pypi/...`). Checked
against 1.11.7 live; a server run with ENABLE_SWAGGER serves its OpenAPI
spec at /api/v1/openapi.json.

## Config
`[keeper]`: `url` (the server's address, `check_base_url`; not `/api/v1`),
optional `token`, `token_env`, `token_cmd` or `token_kv` (an API token,
Bearer, only under the url: `same_origin`; a refused `token_kv` is read
again in `authorization`). None is anonymous: public repositories only.

## Ids
Repo `KEY`; package `KEY/NAME`; version `KEY/NAME@VERSION`; file `KEY/PATH`.
The verbs take `--repo` for a bare name or path, and URLs (`ids.rs`): the web
UI's `/repositories/KEY` and `/packages/ID`, the API's
`/api/v1/repositories/KEY/download/PATH`, a pip index `/pypi/KEY/simple/[NAME/]`.

## Where things are (`src/`)
- `lib.rs`: `DOMAIN`. `doctor.rs`: `/health` and `/auth/me` (or an
  anonymous `/repositories`).
- `client.rs`: `Section`, `Keeper::load`, `api`, `get`, `fetch` (bytes),
  `repo`, `pages` (page numbers), `artifacts` (keyset cursor), `segment`,
  `normalize` (PEP 503); rows `text`, `stamp`, `moment`, `quarantine`.
- `package/mod.rs`: `hosted` (find by name), `upstream` (a proxy's PyPI
  JSON), `cached`. `file/mod.rs`: `FileRow`. `repo/mod.rs`: `RepoRow`.
- `testing.rs`: `keeper_run`, `anonymous`, `page`, `files`, `repository`,
  `artifact`.

## Fixtures
`fixtures/world/http/keeper.json`, `fixtures/world/facts/keeper.md`,
`crates/cli/tests/world_keeper.rs`; queries `search.toml`. Live: eval
what `scripts/keeper-up.sh` prints, then `AGENT_CLI_TEST_KEEPER=1 cargo test
-p agent-cli --test live_keeper`.

## Quirks
- Missing and private are both a 404 (a download is a 401).
- A package is what Keeper parsed from an upload: PyPI metadata, or a
  generic path's first two segments (`NAME/VERSION/FILE`). `latest/VERSION`
  makes none; `a/b/1.0/f` is package `a` version `b`.
- A path is immutable unless the repository keeps versions (409).
- A proxy (`remote`) has no packages: versions come from upstream through
  it; its cache is the files under `simple/NAME/`, which have no row of
  their own (file get finds them in the listing). Upstream missing is 404,
  unreachable 502.
- Virtual repositories list their hosted members' packages; members need
  a token.
- Downloads are limited per address (30 a minute by default, `Retry-After:
  60`), so `file get` waits up to 100 s.
- `per_page` stops at 100; the artifact list's total is a lower bound.

## Never needed
Other crates' sources, `docs/plans/`, `docs/reference/`.
