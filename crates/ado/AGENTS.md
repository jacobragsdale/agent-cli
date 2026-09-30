# ado: Azure DevOps

Work items, pull requests, repos, pipelines, runs and approvals, live over
REST: https://learn.microsoft.com/rest/api/azure/devops/

## Config
`[ado]`: `org` (slug or URL), `project`, `code_project` (repos and pipelines,
when they live elsewhere), `team` (one or a list; what `@current` means).
Unset org and project fall back to `az devops configure` defaults.
Credential: `AZURE_DEVOPS_EXT_PAT` (Basic), else an `az` token (Bearer). It
goes only to `dev.azure.com` and `*.visualstudio.com` (`trusted`).

## Ids
Work item, PR and run: `8812`, `#8812`, `AB#8812`, or its web URL in this org
(`Ado::id(Kind, raw)`; another org or kind is exit 2). Repos by name; an
approval by its GUID; a pipeline by id or name (`Ado::pipeline_id`).

## Where things are
- `lib.rs`: `DOMAIN`, whose `commands` registers every command (its order
  is the listing's), status, doctor, and `testkit` (`ado`, `ado_with`,
  `ado_piped`, `urls`, `dry_run`, `CONFIG`).
- `client.rs`: `Ado::load(ctx)`, then:
  - `get(ctx, url)` a read; `query(ctx, url, body)` a POST that only reads
    (WIQL, batches); `change(ctx, effect, method, url, body)` a write;
    `patch_work_item` a JSON Patch.
  - URLs: `api(project, path, query, version)`, `work`, `code`, `team`;
    `API`, `PREVIEW_API`, `COMMENTS_API` versions.
  - Cached lookups: `me`, `identity` (`@me`), `repo`, `repos`, `pipeline_id`.
  - Row helpers: `text`, `stamp` (UTC), `list`, `short_branch`, `full_ref`,
    `segment`, `query_value`.
- `workitem.rs`, `pr.rs`, `pipeline.rs` (pipelines, runs, approvals):
  commands with their rows and tests.
- `markdown.rs`: rich text to Markdown and back, for descriptions and
  comments.

## Fixtures
`fixtures/world/http/ado.json`; facts `fixtures/world/facts/ado.md`; world
checks `crates/cli/tests/world_ado.rs`; queries `crates/ado/search.toml`.

## Quirks
- Bad credentials can come back as a **203 sign-in page**, not a 401: `send`
  reads it as exit 3.
- WIQL returns ids only, capped by `$top` (20,000); rows come from a batch
  fetch. With `--since`/`--until` the request needs `timePrecision=true`, or
  dates compare by day.
- Lists ask for `limit + 1` (`$top`) to know when to note "more".
- `@me` resolves through connection data, cached for a day.

## Never needed
Other crates' sources, `PLAN.md`, `docs/plans/`, `docs/reference/`, and the
other domains' fixtures.
