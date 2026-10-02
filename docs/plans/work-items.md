# Work items, round 2

ado's work items stop at list, get, create, update, comment and a branch
link. This round covers the rest of what a team does with work items:
sprints and capacity, a ranked backlog, other people's tickets, a type's
rules, history, relations, hierarchy roll-ups, saved queries, attachments
and a person's activity. That is fifteen pieces, built in three phases.

## Checked against the binary (2026-10-02)

- `VERBS` is closed: `relate`, `attach`, `rollover`, `tree` and `history`
  are not verbs. The plan uses resources with existing verbs instead
  (`relation create`, `attachment create`, `sprint complete`, `tree get`,
  `history get`). No verb is added.
- `--before` and `--after` are `SYNONYM_FLAGS` (of `--until` and `--since`),
  so ranking uses `--above ID` and `--below ID`.
- `Ado::identity(ctx, who)` already resolves `@me`, a name or an address
  to one identity id, with exit 4 for nobody and exit 2 for several matches.
  It returns the id only.
- `--iteration @current` is resolved in `workitem/list.rs`
  (`iteration_condition`) against `[ado] team`; no command takes `--team`.
- `sql query run` exists, so `ado query run` needs labeled queries for both
  readings ("run the triage query" against "run this SQL").
- `complete` is in the add-a-command table as `Destructive`, which suits a
  bulk move: core asks for `--yes`, and `--dry-run` lists what would move.

## Shared pieces (phase 0, built first)

### Teams and sprints: `src/iteration.rs`

`iteration::resolve(ctx, ado, team: Option<&str>, raw) -> Result<Iteration>`:
`@current`, `@next`, `@previous`, a full path, or a sprint name that is
unique among the team's iterations. `Iteration {id, name, path, start,
finish, timeframe}`. The team is `--team`, else `agent_cli_core::pick` over
`[ado] team` (exit 2 naming them when there are several and the command
needs one). The team's iterations come from
`{team}/_apis/work/teamsettings/iterations`, cached for an hour.
`workitem list`'s `@current` keeps its WIQL macro path for the one-team
case; `@next` and `@previous` go through `resolve`.

### People: `Ado::person` in `client.rs`

`Ado::person(ctx, who) -> Result<Person {id, name, email}>` is a new
method beside `identity`, sharing its search and its errors; `identity`
becomes `person(…)?.id`. A miss hints `agent-cli ado person list`.

### A type's states: `src/types.rs`

`types::states(ctx, ado, type) -> Result<Vec<State {name, category, color}>>`
from `_apis/wit/workitemtypes/{type}/states`, cached for a day.
`types::done(ctx, ado, type, state) -> bool` is `category` in
`Completed`/`Removed`. Sprint totals, the rollover and tree roll-ups all
use it; nothing guesses from state names.

## The fifteen pieces

| # | Command or change | Effect | Phase |
|---|---|---|---|
| 1 | `ado workitem-type list`, `ado workitem-type get TYPE` | Read | 0 |
| 6 | `ado person list --team T --text S` | Read | 0 |
| 3 | `ado sprint list`, `ado sprint get [SPRINT]`, `--team`, `@next`/`@previous` | Read | 1A |
| 13 | `ado sprint complete SPRINT --to SPRINT` | Destructive | 1A |
| 4 | `ado backlog list --level`, `workitem update --above/--below` | Read / Write | 1A |
| 8 | `workitem list --mentioned --following --team` | Read | 1A |
| 2 | `--field NAME=VALUE` on create and update | Write | 1B |
| 7 | `workitem update --comment` (same PATCH) | Write | 1B |
| 9 | `@<Name>` mentions in Markdown | Write | 1B |
| 1 | Hints on bad states and fields naming `workitem-type get` | — | 1B |
| 5 | `ado history get ID` | Read | 1C |
| 10 | `ado relation create|delete ID --KIND OTHER` | Write | 1C |
| 11 | `ado tree get ID` | Read | 1C |
| 12 | `ado query list`, `ado query run QUERY` | Read | 1D |
| 15 | `ado attachment list|get|create` | Read / Write | 1D |
| 14 | `ado activity list --person P --since 1d` | Read | 2 |

### 1. `workitem-type list|get`

- `list`: `[{name, description, states[]}]` from `_apis/wit/workitemtypes`.
- `get Bug` (case-insensitive): `{name, states[{name, category}],
  transitions{from: [to…]}, fields[{name, ref, type, required,
  allowed_values[], default}]}`. Uses `workitemtypes/{type}?$expand=…`,
  `/states` and `/fields?$expand=allowedValues`. An unknown type is exit 4,
  with a hint pointing to `workitem-type list`.

### 2. `--field NAME=VALUE` (repeatable) on `workitem create|update`

NAME is a reference name (`Microsoft.VSTS.Scheduling.StoryPoints`) or a
display name (`Story Points`), resolved through the type's fields (`types`,
cached). It sends a number for numeric fields and a string otherwise. An
empty value removes the field. A NAME the type lacks is exit 2 with the
hint `agent-cli ado workitem-type get TYPE`. A NAME that a typed flag
already sets (`System.State` with `--state`) is exit 2.

### 1, hints. Bad states and fields

When the PATCH comes back 400 with ADO's own words about a state, a
required field or an allowed value (TF401320, TF401326, VS402625 …),
`create` and `update` keep the service's message and add the hint
`agent-cli ado workitem-type get TYPE`.

### 3. Sprints

- `sprint list --team T --limit N`: `[{id, name, path, start, finish,
  timeframe}]` (past, current, future), oldest first. `id` is the path,
  which `sprint get` takes.
- `sprint get [@current|@next|@previous|PATH|NAME] --team T` (default
  `@current`): `{id, name, path, start, finish, timeframe, working_days_left,
  totals{items, by_state{}, points, points_done, remaining_work},
  people[{name, items, points, remaining_work, capacity_per_day, days_off,
  capacity_left}], team_days_off[]}`. Capacity comes from
  `teamsettings/iterations/{id}/capacities`, days off from `/teamdaysoff`,
  and the items from the iteration's work items (`/workitems`, then the
  batch). A person whose remaining work is more than their capacity left
  gets a `[next: agent-cli ado workitem list --iteration … --assignee …]`
  note naming them (one note: the most overloaded person).
- `@next` and `@previous` work wherever `--iteration` does (`list`,
  `create`, `update`), and `--team T` picks the team they resolve against.

### 13. `sprint complete SPRINT --to SPRINT`

This moves every work item in the sprint whose state is not done (by
`types::done`) to `--to` (default `@next`), with one PATCH each carrying a
`test /rev` op. It skips a done parent's children and any item that changed
under it (reporting them in `skipped[{id, reason}]`). It returns `{sprint,
to, moved[{id, type, title, state}], skipped[]}`. Destructive: `--dry-run`
lists `moved` without sending anything.

### 4. Backlog and rank

- `backlog list --level stories|features|epics --team T --limit N`
  (default the team's requirement backlog): the work item rows in backlog
  order with a `rank` (1-based) and `points`. Uses
  `{team}/_apis/work/backlogs` for the level ids, then
  `backlogs/{id}/workItems` for the order, then the batch.
- `workitem update ID --above OTHER | --below OTHER [--team T]`: one
  `PATCH {team}/_apis/work/workitemsorder` (`ReorderOperation`) after any
  field changes. Both flags together are exit 2. `--dry-run` shows the
  operation.

### 8. List filters

`workitem list --mentioned` (`[System.Id] IN (@RecentMentions)`),
`--following` (`@Follows`) and `--team T` (which team `@current`, `@next`
and `@previous` mean).

### 7. `workitem update --comment TEXT` (or `-`, or `--comment-file`)

The comment is Markdown converted to HTML and sent as `System.History` in
the same JSON Patch as the field changes, so it is one revision and one
notification. `update 1207 --assignee @me --comment "Taking this" --if-rev
7` is how an agent takes someone's ticket. `-` follows the long-text rule:
one `-` per command, so with `--description -` it is exit 2.

### 9. Mentions

In any Markdown that `markdown.rs` sends (comments, `--comment`,
descriptions, acceptance criteria), `@<Name or address>` and a bare
`@name@domain.com` become `<a href="#"
data-vss-mention="version:2.0,{id}">@Display Name</a>` through
`Ado::person`. An unknown person is exit 4 (exit 2 when ambiguous), before
anything is sent. Reading HTML back turns mention anchors into
`@<Display Name>`, so a round trip keeps them. `--dry-run` shows the
resolved HTML.

### 6. `person list --team T --text S --limit N`

`[{id, name, email, team}]` from `_apis/projects/{p}/teams/{t}/members`.
`id` is the address, which every `--assignee` and mention takes. With no
`--team` it lists the configured teams' members, deduplicated. `--text`
filters by name or address.

### 5. `history get ID [--field NAME] [--since T]`

`{id, title, states[{state, by, since, until, hours}], changes[{rev, by,
date, fields[{field, old, new}], comment}]}` from `workItems/{id}/updates`
(paged by `$top`/`$skip`). `states` is the time spent in each state, and
the current one has a null `until`. `--field` keeps the changes to that
field (by reference or display name). Rich text in `old`/`new` is Markdown.

### 10. `relation create|delete ID --parent|--child|--related|--blocks|--blocked-by|--duplicate-of OTHER`

Exactly one kind flag is required (exit 2 otherwise). It maps to
`Hierarchy-Reverse`, `Hierarchy-Forward`, `Related`, `Dependency-Forward`
(this item is the predecessor), `Dependency-Reverse` and
`Duplicate-Reverse`. `create` on an existing link returns
`already_linked: true` with exit 0; a second parent is exit 5, with the
hint to delete the old one first. `delete` finds the relation's index and
sends `remove /relations/N` with `test /rev`; a missing link is exit 4. It
returns `{work_item, kind, other, already_linked|removed}`. `workitem get`
gains `blocks[]` and `blocked_by[]` beside `related[]`.

### 11. `tree get ID [--depth N]`

`{id, type, title, state, assignee, points, remaining_work, rollup{items,
done, by_state{}, points, points_done, remaining_work, percent_done},
children[…same…]}`, from a recursive `WorkItemLinks` WIQL query
(`Hierarchy-Forward`, `MODE (Recursive)`) and one batch read. Done is
judged by `types::done`. With `--depth` (default 5), deeper children are
counted in the rollup but not printed.

### 12. `query list|run`

- `query list --text S --limit N`: `[{id, name, path, type, is_public}]`
  from `wit/queries?$depth=2&$expand=minimal` (My Queries and Shared
  Queries). `id` is the GUID.
- `query run QUERY --limit N`: QUERY is the GUID, the path (`Shared
  Queries/Triage`), a name unique among the listed queries, or its web URL.
  A flat query returns work item rows, as `workitem list` does. A tree or
  one-hop query returns those rows with a `parent` (or `source`) id. It uses
  `wiql/{id}` and the batch.

### 15. Attachments

- `attachment list ID`: `[{id, name, size, by, date, comment}]` from the
  work item's `AttachedFile` relations. `id` is the attachment GUID.
- `attachment get GUID|URL [--output FILE]`: text up to 1 MiB prints
  `{id, name, size, content}`. Anything else needs `--output`, which saves
  the bytes (the way `ado file get` handles a binary file) and prints
  `{id, name, size, saved}`.
- `attachment create ID --file PATH [--comment TEXT]`: POST the bytes to
  `wit/attachments?fileName=` (octet-stream), then PATCH an `AttachedFile`
  relation. Write; the cap is 60 MB, as ADO's is.

### 14. `activity list --person P --since 1d --limit N`

This merges, newest first, `[{at, kind, action, id, title}]`, where `kind`
is `workitem|comment|pr|commit|run` and `id` is what that kind's `get`
takes. The sources are:

- Work item revisions by P in the window: WIQL `ChangedDate >= since AND
  EVER ChangedBy = P`, then each item's `updates`. A revision with
  `System.History` counts as a comment.
- PRs P created, completed or voted on.
- Commits P authored, in the code project's repositories.
- Runs requested for P.

`--person` says `@me` works, and is the default. `// ponytail:` it reads
updates for at most 50 work items and commits from at most 20 repositories,
and notes what it skipped.

## Phases

Each builder works in its own worktree. Phase 1 adds only unit fixture tests
(`testing.rs` answers on a fake transport). World recordings come in phase
2, so `fixtures/world/http/ado.json` is edited once.

- **Phase 0** (one builder, on `main`): `iteration`, `Ado::person`, `types`,
  `workitem-type list|get`, `person list`.
- **Phase 1** (four builders in parallel worktrees, merged in order A, B, C,
  D):
  - A: sprints, rollover, backlog and rank, list filters.
  - B: `--field`, `--comment`, mentions, hints.
  - C: history, relations, tree.
  - D: queries, attachments.
- **Phase 2** (one builder on the merged tree): `activity list`; then world
  recordings and `facts/ado.md` for the trial-reachable reads (sprint get,
  backlog list, workitem-type get, person list, history get, tree get, query
  run) and a `world_ado` test each; the ado card; the reference;
  `scripts/check.sh --all`.

## Done when

- Every command above passes `check_registry`, has a fixture test, and a
  dry-run test when it writes. It has at least two labeled queries in
  `crates/ado/search.toml`, and the search gate holds (top-1 80%, top-5 95%).
- `scripts/check.sh --all` passes and the reference is regenerated.
- The ado card names the new modules (`iteration`, `types`, `person`) and
  quirks (`workitemsorder` needs the team; the backlog order comes from
  `backlogs/{id}/workItems`, not WIQL).
