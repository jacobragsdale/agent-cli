# ado: Azure DevOps

Work items, sprints, PRs, repos, pipelines, runs and approvals over REST:
https://learn.microsoft.com/rest/api/azure/devops/

## Config
`[ado]`: `org`, `project`, `code_project` (repos and pipelines, if
elsewhere), `team` (one or a list: `@current`, sprints, people). Unset org
and project fall back to `az devops configure`. Credential:
`AZURE_DEVOPS_EXT_PAT` (Basic), else an `az` token; only to `trusted` hosts.

## Ids
Work item, PR, run: `8812`, `AB#8812` or its web URL (`Ado::id(Kind, raw)`).
Repos by name; approvals and queries by GUID; pipelines by id or name. In
`ids.rs`: a file `[PROJECT/]REPO[@REF]:PATH[:LINE[-LINE]]` (`FileId`),
`REPO@BASE..HEAD` (`Range`), a thread `436/7`. A bare ref is a branch, else
a tag. `each` runs a verb over `ID…`.

## Where things are (`src/`)
A command is `<resource>/<verb>.rs`; `<resource>/mod.rs` is what they share.
- `lib.rs`: `DOMAIN` (the listing's order). `doctor.rs`.
- `client.rs`: `Ado::load`; `get`, `query` (a POST that reads), `change`,
  `patch_work_item`; URLs `api`, `work`, `code`, `team`; cached `me`,
  `person` (`@me`, a name or an address), `identity`, `repo`, `pipeline_id`;
  row helpers `text`, `stamp` (UTC), `list`, `segment`.
- `iteration.rs`: `team`, `iterations` (cached an hour), `resolve`.
- `types.rs`: a type's `states`, `fields` (a day), `done` (by category).
- `work_items.rs`: batch rows, `POINTS`, artifact links. `markdown.rs`:
  HTML to Markdown and back. `compose.rs`: mentions resolved to people.
- Work: `sprint/`, `backlog/`, `history/`, `relation/`, `tree/`, `query/`,
  `attachment/`, `workitem_type/`, `person/`, `activity/` (a person's feed).
- `testing.rs`: `ado`, `urls`, `dry_run`, `CODE`, answers (`page`, `pr`).

## Fixtures
`fixtures/world/http/ado.json`, `fixtures/world/facts/ado.md`,
`crates/cli/tests/world_ado.rs`.

## Quirks
- Bad credentials can answer a 203 sign-in page (exit 3). An org backed by
  a Microsoft account (resource tenant all zeros) refuses az tokens: set
  `AZURE_DEVOPS_EXT_PAT`.
- WIQL answers ids (`$top` at most 20,000), a batch the rows; `--since`
  needs `timePrecision=true`, else dates compare by day. Lists ask `limit + 1`.
- A field's data type is only in `wit/fields`. Points are Story Points,
  Effort or Size by process (`POINTS`). A value outside a picklist is a
  RuleValidationException with no TF code.
- `workitemsorder` needs the team. Backlog order is
  `backlogs/{id}/workItems`, not WIQL; stories are the `requirement` level.
- `updates` pages by `$top` and `$skip`; a revision's time is its
  ChangedDate (revisedDate is when the next one replaced it).
- Mentions are `data-vss-mention` anchors in work item HTML, `@<id>` in PR
  Markdown. Attachments download from the org's URL, upload to the project.

## Never needed
Other crates' sources and fixtures, `docs/plans/`, `docs/reference/`.
