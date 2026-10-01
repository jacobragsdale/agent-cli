> Built 2026-09-30. Where the build departed from this plan: core refuses `--lines` and `--context` as synonyms of `--tail` and `--cluster`, so line ranges are `--line A[-B]`, `thread list` takes `--around N` and `diff get` `--unified N`; `pipeline preview` takes the YAML as a positional (`-` for stdin) beside `--yaml-file`; `diff get` rows hold `hunks[]`, each with its own `at`; `file list` has no `size`; `xcom get` cuts at 10,000 bytes, under core's output guard. For the current commands, ask the binary.

# Commands that chain: code, reviews and DAG internals

New commands for ado and airflow, designed around the flows an agent runs
end to end. Each row prints an `id` (or a field holding another command's
id) that the next command takes as it is, so a flow is a sequence of
copy-the-id hops, never a lookup the agent has to reconstruct.

## The flows

**F1. Address the review comments on a pull request**
```
ado thread list 436                                   # unresolved threads, each with the code it is on
ado file get api@<head>:src/Orders/OrderClient.cs:42  # more of that file, at the commit the comment saw
ado diff get 436 --file src/Orders/OrderClient.cs     # or the change itself
ado thread comment 436/7 "Capped at 30 s in 9f1c2e4" --resolve
```
Built for it: thread rows carry the code around the comment (`code`), so
most threads need no second hop; `at` is the `file get` id; `--resolve`
replies and resolves in one call.

**F2. Review a pull request**
```
ado pr get 436
ado diff get 436 --names-only                         # what changed
ado diff get 436 --file '*.cs'                        # the hunks
ado pr comment 436 "Jitter is unbounded here" --at api@<head>:src/Orders/OrderClient.cs:42
ado pr vote 436 suggestions
```
Built for it: each hunk line range carries an `at`, which `pr comment --at`
takes to open a thread on that line.

**F3. Who calls this, and where is it defined**
```
ado code list IOrderClient                            # every repo in the organization
ado file get worker:src/Jobs/Retry.cs:18              # a match's id, as printed
ado file list worker:src/Jobs                         # its neighbours
```

**F4. What changed between two releases**
```
k8s deployment list --fields id,images                # api runs api:v1.4.2
ado diff get api@v1.4.1..v1.4.2 --names-only
ado diff get api@v1.4.1..v1.4.2 --file src/Orders/OrderClient.cs
```

**F5. Fix a pipeline's YAML**
```
ado run get 8809 --fields failed
ado pipeline get api-ci                               # its yaml: api:azure-pipelines.yml
ado file get api:azure-pipelines.yml
ado pipeline preview api-ci --yaml-file azure-pipelines.yml   # the expanded YAML, or the template error
```
Built for it: a preview that fails on a template names `FILE (Line: N`, and
the error's hint is the `ado file get` for that line.

**F6. Why a DAG task failed, in its code and its input**
```
airflow task logs etl_nightly/latest/load_orders/2 --fields error,at
airflow source get etl_nightly:42                     # the DAG file around the failing line
airflow xcom list etl_nightly/latest/extract_orders
airflow xcom get etl_nightly/latest/extract_orders@return_value
ado file get airflow-dags:dags/etl_nightly.py:42      # the same line in the repo, to fix it
```
Built for it: `task logs` prints `at`, the `source get` id of the failing
line in the DAG's own file; `source get` prints `repo_file`, the `ado file
get` id, when the instance names the repo its DAGs deploy from.

**F7. Why tasks sit queued, and where a DAG's data goes**
```
airflow task get etl_nightly/latest/load_orders --fields pool,blocked_by
airflow pool list                                     # open, running and queued slots
airflow connection list --fields id,type,host,sql_conn
sql query run --conn reporting 'select …'             # sql_conn: the configured sql connection on that host
```

## Ids

| Thing | Id | Printed by | Taken by |
|---|---|---|---|
| A file at a ref, a line or a range | `[PROJECT/]REPO[@REF]:PATH[:LINE[-LINE]]` | `code list`, `thread list` (`at`), `diff get` (`at`), `file list`, `pipeline get` (`yaml`), airflow `source get` (`repo_file`) | `file get`, `file list` (a folder), `pr comment --at`, `diff get --file` (the path) |
| A review thread | `PR/THREAD` (`436/7`) | `thread list`, `pr get` (`threads[].id`) | `thread comment`, `thread update` |
| A comparison | a PR id, or `REPO@BASE..HEAD` | — | `diff get` |
| A DAG's source line | `DAG[:LINE[-LINE]]` | `task logs` (`at`) | `source get` |
| An XCom | `DAG/RUN/TASK[:MAP]@KEY` | `xcom list` | `xcom get` |

- `REF` is a branch, a tag or a commit (7 to 40 hex digits); none is the
  repository's default branch. Git refs cannot hold `:`, so the first `:`
  ends the repository part. `PROJECT/` defaults to `[ado] code_project`.
- Every taker also accepts the web URL: a file's
  (`…/_git/api?path=/src/x.cs&version=GBmain&line=42`, `GT` a tag, `GC` a
  commit), a thread's (`…/pullrequest/436?discussionId=7`), a DAG's.
- The pieces as flags, as everywhere: `--ref`, `--lines A-B`, `--pr`,
  `--key`. A piece in the id and a flag that disagree is exit 2.
- `pr get` threads change from `id: 7` to `id: "436/7"` and gain `at`, so
  both listings hand the thread verbs the same thing.

## ado commands

| Command | Effect | API (7.1) | Notes |
|---|---|---|---|
| `thread list PR` | Read | `GET git/repositories/{repo}/pullRequests/{pr}/threads`; the latest iteration for its head commit; `items` per file for `code` | Default: active and pending, no system threads; `--status all\|active\|fixed\|wontFix\|closed\|byDesign\|pending`; `--author` (`@me`); `--file GLOB`. Row: `id`, `status`, `at`, `file`, `line`, `author`, `date`, `text` (first comment), `replies` (author, date, text; the last 5), `code` (the lines around it: `--context N`, default 3; at most 20 files fetched). `--limit` 50 |
| `thread comment ID TEXT` | Write | `POST …/threads/{t}/comments` `{parentCommentId: 1, content, commentType: "text"}`; with `--resolve` also `PATCH …/threads/{t}` `{status: "fixed"}` | Long text as `pr comment` takes it (`-`, `--text-file`, 64 KiB) |
| `thread update ID --status S` | Write | `PATCH …/threads/{t}` `{status}` | Reopen with `--status active` |
| `pr comment --at FILE_ID` (a flag on the existing command) | Write | `POST …/threads` with `threadContext {filePath, rightFileStart, rightFileEnd}` | Without `--at`, as today |
| `diff get PR_OR_RANGE` | Read | PR: its latest iteration's `commonRefCommit` and `sourceRefCommit`. Range: the two refs. `GET git/repositories/{repo}/diffs/commits?baseVersion…&targetVersion…` for the files; `items?…&includeContent=true` at both commits for hunks | One row per file: `at`, `path`, `change` (add, edit, delete, rename), `added`, `removed`, `diff` (unified, `--context N` default 3, at most 400 lines a file). `--file GLOB` (repeatable) filters before fetching; `--names-only` makes one call; `--limit` 50 files |
| `code list TEXT` | Read | `POST https://almsearch.dev.azure.com/{org}/_apis/search/codesearchresults` `{searchText, $top, filters: {Project, Repository, Path, Branch}, includeSnippet: true}` (a POST that only reads) | `--repo` (repeatable), `--project`, `--path`, `--branch`, `--limit` 50. Row: `id` (`REPO:PATH:LINE` of the first match), `repo`, `path`, `line`, `text` (the matching line), `matches`. Search syntax passes through (`ext:cs`, `class:`). Exit 3 naming the Code Search extension when the organization lacks it |
| `file get FILE_ID` | Read | `GET git/repositories/{repo}/items?path&versionDescriptor.version&versionDescriptor.versionType&includeContent=true&$format=json` | Row: `id`, `repo`, `path`, `ref`, `commit`, `lines` (`A-B` of `total`), `text`. A `:LINE` shows 20 lines either side; no line, the first 400 lines with a note. A bare ref tries branch, then tag. Binary files are refused with their size |
| `file list FOLDER_ID` | Read | `items?scopePath&recursionLevel=OneLevel` | Rows `id`, `path`, `kind` (file, folder), `size`; `--limit` 50; `--recursive` |
| `pipeline get PIPELINE` | Read | `GET build/definitions/{id}` | By id or name (`Ado::pipeline_id`). Row: `id`, `name`, `folder`, `repo`, `default_branch`, `yaml` (the `file get` id), `queue_status`, `url` |
| `pipeline preview PIPELINE` | Read | `POST pipelines/{id}/preview` `{previewRun: true, yamlOverride, resources.repositories.self.refName}` (a POST that only reads) | `--yaml TEXT` (`-` for stdin) and `--yaml-file PATH`; `--branch`. Row: `pipeline`, `branch`, `yaml` (the expanded text, at most 2,000 lines). A template error is exit 2 with the service's words and, when it names `PATH (Line: N`, the hint `agent-cli ado file get REPO:PATH:N` |

New verb: `preview`, added to `VERBS`. New synonyms (ado):
`review comment`, `review comments`, `comment thread` → `thread`; `source
code`, `code search` → `code`; `changes`, `changed files`, `compare` →
`diff`; `yaml`, `template` → `pipeline`.

## airflow commands

| Command | Effect | API (`/api/v2`) | Notes |
|---|---|---|---|
| `source get DAG_LINE` | Read | `GET dagSources/{dag_id}`; `GET dags/{dag_id}` for `relative_fileloc` | Row: `id`, `dag`, `file`, `version`, `lines`, `text`, `repo_file`. A `:LINE` shows 20 lines either side; none, the first 400 |
| `xcom list TASK_ID` | Read | `GET dags/{d}/dagRuns/{r}/taskInstances/{t}/xcomEntries` | Rows `id` (`…@KEY`), `key`, `map`, `time`. Keys only |
| `xcom get XCOM_ID` | Read | `GET …/xcomEntries/{key}?deserialize=true&stringify=false` | Row: `id`, `key`, `time`, `value` (JSON, cut at 16 KiB with a note). A task id with `--key` (default `return_value`) works too |
| `pool list` | Read | `GET pools` | Rows `id`, `slots`, `open`, `running`, `queued`, `scheduled`, `deferred`, `description` |
| `variable list [PATTERN]` | Read | `GET variables?variable_key_pattern` | Rows `id` (the key), `description`, `encrypted`. **Never the value**: some are secrets, and no row type has a field one could go in |
| `connection list [PATTERN]` | Read | `GET connections?connection_id_pattern` | Rows `id`, `type`, `host`, `port`, `schema`, `description`, `sql_conn`. **Never** password, login or `extra`. `sql_conn` names the `[[sql.connection]]` with the same host (and database, when both say), read from config: the id `sql query run --conn` takes |

Changes to existing airflow commands:

- `task logs` gains `at`: the `source get` id of the innermost frame in the
  DAG's own file (`etl_nightly:42`), when the traceback has one.
- `[[airflow.instance]]` gains an optional `dags_repo = "REPO[:FOLDER]"`
  (`"airflow-dags:dags"`): where the DAG files live in Azure DevOps, so
  `source get` can print `repo_file` (`airflow-dags:dags/etl_nightly.py:42`).
  No request goes to Azure DevOps; it is a string join.

New synonyms (airflow): `dag code`, `dag file` → `source`; `return value`,
`task output` → `xcom`; `slots` → `pool`.

## Every command also gets

- At least two labeled queries in its crate's `search.toml`, and queries for
  the flows' first hops ("address the review comments on PR 436", "who calls
  IOrderClient", "what changed between v1.4.1 and v1.4.2", "show the code at
  the line load_orders failed").
- A fixture test, and a dry-run test for `thread comment`, `thread update`
  and `pr comment --at`.
- Its exchanges in the contoso world and a line in the domain's facts:
  - **ado:** PR 436's threads, one of them unresolved on
    `src/Orders/OrderClient.cs:42`, and its iteration and two file versions;
    `IOrderClient` found in `api` and `worker`; `api-ci`'s definition and
    preview; `azure-pipelines.yml`; and the v1.4.1..v1.4.2 diff, which
    holds PR 431's change.
  - **airflow:** `etl_nightly.py` raising at line 42; `extract_orders`'
    `return_value` holding order 88123 with no `customer_id`;
    `default_pool`; variables; connections. Plus `dags_repo` in the world's
    config, and the `airflow-dags` file in `ado.json`.
- A world test per flow: F1 to F5 in `crates/cli/tests/world_ado.rs`; F6 and
  F7's airflow half in a new `world_airflow.rs`; F6's hop to Azure DevOps in
  `world_cross.rs`.
- Its crate's card updated (ids, where things are), and the reference
  regenerated.

## To confirm live

The shapes above come from Microsoft's and Apache's documentation, not a
live organization; record what differs in `TODO.md`:

- Code search's `includeSnippet`, and whether `matches.content[].line` is
  filled.
- The host for `*.visualstudio.com` organizations
  (`{org}.almsearch.visualstudio.com`).
- `diffs/commits` paging past 100 changes.
- The preview's error wording, and whether `(Line: N, Col: M)` is always
  there.
- Airflow 3's `dagSources` by `dag_id` and `version_number`, and xcom
  `deserialize`.

## Build order

1. ado (one builder): `preview` in `VERBS`; ids and the `file get` / `file
   list` parser first, since `code list`, `thread list`, `diff get` and
   `pipeline get` print them; then threads, diff, code, pipeline.
2. airflow (another builder, in parallel): source, xcom, pool, variable,
   connection, then `task logs`' `at` and `dags_repo`.
3. Then the cross-domain test (F6's hop to ado), the cards, the reference,
   and a short agent trial of F1, F3 and F6 against the world.
