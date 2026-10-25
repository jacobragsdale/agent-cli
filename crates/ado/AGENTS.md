# ado: Azure DevOps

Work items, sprints, PRs, repos, pipelines, runs and approvals over REST:
https://learn.microsoft.com/rest/api/azure/devops/

## Config
`[ado]`: `org`, `project` and `team` (one or a list; the first project is
the default; `PROJECT/TEAM`), `code_project`, else `az devops configure`.
Credential: `pat`, `pat_env`, `pat_cmd`, `pat_kv`, then
`AZURE_DEVOPS_EXT_PAT`, then `az`. `Ado.project` steers URLs, caches:
lists search all projects in one WIQL, other verbs take `--project` or
the item's `System.TeamProject`.

## Ids
Work item, PR, run: `8812`, `AB#8812` or its web URL (`Ado::id(Kind, raw)`).
Repos by name; approvals and queries by GUID; pipelines by id or name. In
`ids.rs`: a file `[PROJECT/]REPO[@REF]:PATH[:LINE[-LINE]]` (`FileId`),
`REPO@BASE..HEAD` (`Range`), a thread `436/7`. A bare ref is a branch, else
a tag. `each` runs a verb over `ID…`.

## Where things are (`src/`)
A command is `<resource>/<verb>.rs`; `<resource>/mod.rs` is shared.
- `lib.rs`: `DOMAIN` (listing order). `doctor.rs`. `config.rs`:
  `Ado::load`, `load_in`, `in_project`. `auth.rs`: `authorization` (to
  `trusted` hosts; `renew` rereads a refused `pat_kv`).
- `client.rs`: `get`, `query` (a POST that reads), `change`,
  `patch_work_item`; URLs `api`, `work`, `code`, `team`; cached `me`,
  `person` (`@me`, a name, an address), `identity`; `repo`, `pipeline_id` (live);
  row helpers `text`, `stamp` (UTC), `list`, `segment`.
- `iteration.rs`: `team`, `iterations` (an hour), `resolve` (`@next`:
  the team's; sprint complete's: the sprint after the closed one).
- `types.rs`: a type's `states`, `fields` (a day), `done` (by category).
- `work_items.rs`: batch rows, `POINTS`, artifact links. `markdown.rs`:
  HTML to Markdown and back. `compose.rs`: mentions resolved to people.
- Work: `sprint/`, `backlog/`, `history/`, `relation/`, `tree/`, `query/`,
  `attachment/`, `workitem_type/`, `person/`, `activity/` (one's feed).
- `testing.rs`: `ado`, `urls`, `dry_run`, `CODE`, answers (`page`, `pr`).

## Fixtures
`fixtures/world/http/ado.json`, `fixtures/world/facts/ado.md`,
`crates/cli/tests/world_ado.rs`.

## Quirks
- Bad credentials can answer a 203 sign-in page (exit 3). An MSA-backed
  org (tenant all zeros) refuses az tokens: set `AZURE_DEVOPS_EXT_PAT`.
- WIQL answers ids (`$top` at most 20,000), a batch the rows; `--since`
  needs `timePrecision=true`, else dates compare by day. Lists ask `limit + 1`.
- A field's data type is only in `wit/fields`. Points are Story Points,
  Effort or Size by process (`POINTS`). Off a picklist: a
  RuleValidationException, no TF code.
- `workitemsorder` needs the team; a 0 `previousId` (the top) is left out.
  Backlog order is `backlogs/{id}/workItems`, not WIQL.
- `updates` pages by `$top` and `$skip`; a revision's time is its
  ChangedDate (revisedDate: when the next came).
- Mentions are `data-vss-mention` anchors in work item HTML, `@<id>` in PRs.
  Attachments download from the org, upload to the project.

## Never needed
Other crates' sources and fixtures, `docs/plans/`, `docs/reference/`.
