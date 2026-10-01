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
approval by its GUID; a pipeline by id or name (`Ado::pipeline_id`). In
`ids.rs`, with their URLs: a file `[PROJECT/]REPO[@REF]:PATH[:LINE[-LINE]]`
(`FileId`, printed by `file_id`), `REPO@BASE..HEAD` (`Range`), a thread
`436/7` (`thread_id`). A bare ref is a branch, else a tag (`resolving`).

## Where things are (`src/`)
A command is `<resource>/<verb>.rs`: its args, rows, handler, `command!`
and tests (`approval/list.rs` is `ado approval list`). Copy a sibling.
- `lib.rs`: `DOMAIN`, whose `commands` registers every command (its order
  is the listing's). `doctor.rs`: status and doctor.
- `client.rs`: `Ado::load(ctx)`, then:
  - `get(ctx, url)` a read; `query(ctx, url, body)` a POST that only reads
    (WIQL, batches); `change(ctx, effect, method, url, body)` a write;
    `patch_work_item` a JSON Patch.
  - URLs: `api(project, path, query, version)`, `work`, `code`, `team`;
    `API`, `PREVIEW_API`, `COMMENTS_API` versions.
  - Cached lookups: `me`, `identity` (`@me`), `repo`, `repos`, `pipeline_id`.
  - Row helpers: `text`, `stamp` (UTC), `list`, `short_branch`, `full_ref`,
    `segment`, `query_value`.
- `work_items.rs`: work item rows read in batches, and the artifact links
  workitem, pr and run share. `markdown.rs`: rich text to Markdown and back.
- `<resource>/mod.rs`: what its verbs share (`pr/mod.rs`: the PR row,
  `latest_iteration`; `run/mod.rs`: `RunRow`; `file/mod.rs`: `fetch` a file
  at a ref; `thread/mod.rs`: `fetch_threads`, placed on the head; `diff/mod.rs`:
  the line diff).
- `testing.rs`: `ado`, `ado_with`, `ado_piped`, `urls`, `dry_run`, `CONFIG`,
  `CODE`, and sample answers (`page`, `item`, `wiql`, `pr`, `build`, …).

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
