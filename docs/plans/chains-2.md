# Chains, round 2

`chains.md` built the commands that let one result feed the next (flows F1
to F7). The chains trial (`docs/trials/chains-2026-09-30.md`) found that a
field is not a signpost but a note naming the next command is: Haiku missed
`task logs`' `at` until a note printed `agent-cli airflow source get …`.
This round carries that lesson to every domain, then closes the chains that
still end at a string: a failed build, a failing test, a production
exception and an import error, each down to a repository line, and the
change behind it.

## Checked against the binary and the world (2026-09-30)

What the discussion assumed, and what is true:

- **Only airflow prints next-step notes.** `task logs` (to `source get`, or
  to `k8s pod logs` when Airflow has no log), `run create` (a paused DAG,
  then `run wait`), `run retry` and `task retry` (`run wait`), `source get`
  (the next range). `ado run create` prints none; no ado, dd or k8s command
  does.
- **An import error's `at` cannot be a `source get` id.** A file that fails
  to import has no DAG, so `dagSources` has nothing to show. Its line goes
  to the repository instead: `repo_file`, through the instance's
  `dags_repo`, as `source get` already builds it.
- **A build error's `sourcepath` is the agent's absolute path**
  (`/home/vsts/work/1/s/src/…`, `D:\a\1\s\src\…`), and a container's stack
  frame is the image's path (`/app/…`, `/src/…`). Neither is a repository
  path, so every chain below needs one mapping (see "Path to repository
  path").
- **Only compiler-style tasks fill `sourcepath`** (MSBuild and `dotnet
  build`, problem matchers, `##vso[task.logissue]`). A test task's issue is
  "3 tests failed": the line comes from the Test Results API, not the
  timeline. In the world, run 8809 fails exactly that way.
- **`dd log list` already returns every attribute** (`--full`), so
  `error.stack` is reachable today; what is missing is the frame picked out
  of it.
- **`--fields id` prints `[{"id":…}]`.** `lines` is a synonym `SYNONYM_FLAGS`
  maps to `--tail`, which is why a bare-value flag cannot be `--lines`.
- **The world already holds most of the answers**: `RetriesOn429` failing
  in 8809, `src/Orders/OrderClient.cs` and the test file, PR 431 and its
  merge `4be1c0d2…`, import error 12 at `customer_sync.py` line 5, monitor
  4712 alerting on the crash-looping worker, and `worker-db-password`
  expired in `kv-contoso-prod`.

## Shared pieces

### Next-step notes

A one-object verb whose answer exits 0 but needs attention prints one note,
`[next: agent-cli …]`, naming the command that answers the next question,
built from the row it just printed. One note, the most useful one; lists
print none (their notes stay about paging). "Printed command lines parse"
already checks every note resolves.

### Path to repository path

`ado`'s `file/mod.rs` gets `resolve(repo, commit, path)`: if `path` is not
in the repository, the longest suffix of it that names exactly one file in
the tree at that commit. One read of
`items?recursionLevel=Full&versionDescriptor.versionType=commit`, cached
for good (a commit's tree never changes). No configuration: the agent's
`/s/`, a multi-repo checkout's `s/<repo>/`, a Dockerfile's `WORKDIR /src`
and a container's `/app` all reduce to the same suffix.

- `ado run get` and `ado test list` resolve before printing, so their `at`
  is clean, and use the tree to tell the repository's frames from the
  framework's.
- `ado file get` resolves on a 404 and notes what it matched
  (`[/app/src/Orders/OrderClient.cs is src/Orders/OrderClient.cs]`), so
  any domain can hand it a frame path as the service printed it.
- Two files ending the same way (`Program.cs` in two projects) are no
  match: no `at` rather than a wrong one.

`// ponytail:` a full tree read is one call but megabytes on a large
monorepo; narrow it with `scopePath` from the frame's last segments if a
live repository makes it slow.

## Phase 1: next-step notes everywhere

No new commands or fields.

| Command | When | Note |
|---|---|---|
| `ado run get` | failed, and the build has failed tests | `ado test list ID` (after phase 2) |
| | failed, an error has an `at` | `ado file get AT` (after phase 2) |
| | failed otherwise | `ado run logs ID --task 'NAME'` (the first failed task) |
| | still running | `ado run wait ID` |
| `ado run create` | always | `ado run wait ID`, as airflow's `run create` |
| `ado pr get` | a policy failed with a `run_id` | `ado run get RUN` |
| `airflow run get` | failed | `airflow task logs DAG/RUN/TASK` (the first failed task) |
| `dd monitor get` | Alert, a group has a `pod` | `dd log list --pod POD --status error --since T` (T is 15 minutes before `triggered`) |
| `k8s pod get` | a container restarted with a last termination | `k8s pod logs ID --previous --tail 50` |

Until phase 2 lands, `ado run get` always takes the `run logs` branch.

Tests: one beside each command. In `world_cross.rs`, a helper that runs a
command and follows its `[next: …]` notes until none is left, and one test
per trace that reaches the answer by notes alone: 4712 to the previous log
saying the password was refused, 8809 to the failing test's log.

## Phase 2: failures to a line

**2a. `airflow import-error get`** gains `line` and `repo_file`: the last
frame in the stack trace that ends with the error's `file` (`failing_line`
from `task logs`, moved to `client.rs`), and the `dags_repo` mapping from
`source get` (also moved). World: 12 prints
`airflow-dags:dags/customer_sync.py:5`. Note: `ado file get REPO_FILE`.

**2b. `ado run get` errors carry their line.** `failed[].errors[]` changes
from strings to `[{message, at}]`; `at` is
`REPO@COMMIT:PATH:LINE` from the issue's `data.sourcepath` and
`data.linenumber`, resolved, present only when both are. The build's own
repository and commit name the file. World: PR 436 gains an `api-ci`
policy (today it has only the reviewer count) whose run, **8814**, fails
compiling `src/Orders/OrderClient.cs` at a line PR 436 changed, at its
`last_merge_source_commit` `a7e3c9f1…`, so `pr get 436` leads to it.

**2c. `ado test list RUN`** (new resource `test`). `--outcome
failed|passed|all` (default failed), `--limit`. Rows: `{name, outcome,
duration_ms, error, at, failing_since, stack}`; `name` is the automated
test name, `failing_since` the run where it began failing (new failure or
old), `stack` cut to 20 lines, `at` the innermost frame in the repository
(.NET `in PATH:line N`, Python `File "PATH", line N`, JS `(PATH:L:C)`).
API: `test/runs?buildUri=vstfs:///Build/Build/ID`, then each run's
`results?outcomes=Failed`. `run get`'s note checks the build's test runs
only when it failed. World: 8809's three failures, `RetriesOn429` at
`src/Orders/OrderClient.cs` inside the test's frame.

## Phase 3: a production exception to the change behind it

**3a. dd rows carry `at`.** `dd log list` and `dd span list` read
`error.stack` and pick the innermost frame with a path that is not a
library's (`site-packages`, `dist-packages`, `node_modules`, `<frozen`,
`/usr/lib`). With `git.repository_url` (an Azure DevOps `_git` URL) and
`git.commit.sha`, `at` is `PROJECT/REPO@SHA:PATH:LINE`; without them it is
`PATH:LINE`, which `ado file get` takes with `--repo` and `--ref` (the
deploy trace gives both: pod, image tag, tag build). `PATH` is the
container's; `file get` resolves it.

**3b. `ado commit list REPO[:PATH]`** (new resource `commit`). `--ref`
(default the default branch), `--since`, `--until`, `--author` (`@me`
works), `--limit`. Rows: `{commit, author, date, message, pr{id,title},
diff}`; `message` is the first line, `diff` is `REPO@PARENT..SHA` for
`diff get`, `pr` comes from one `pullrequestquery` for the whole page
(already used for tag builds). API:
`commits?searchCriteria.itemPath=…&searchCriteria.itemVersion.version=…`.
This makes `run list --commit` and `pr list --commit` (TODO.md) unneeded.

The chain: `dd log list` → `at` → `ado file get` → `ado commit list
api:src/Orders/OrderClient.cs --until <log time>` → `pr.id` → `ado pr get`
→ `work_items`. Azure DevOps has no blame API, so "which commit changed line
42" is the agent reading each commit's `diff` with `--file`.

World: api's 503 logs get an `error.stack` through
`/app/src/Orders/OrderClient.cs` and the git tags at `4be1c0d2…`; commits
on `OrderClient.cs` are `4be1c0d2…` (PR 431) and `2d8b6f4a…`.

## Phase 4: reverse lookups

- **`k8s pod list --kv VAULT/NAME`**: pods whose CSI volume's
  SecretProviderClass maps that secret (the `kv secret list` id). One
  `kubectl get pods,secretproviderclasses -o json`, filtered with
  `pod get`'s class reading. Rows keep `owner`, so the next hop is `k8s
  deployment restart`. World: `kv-contoso-prod/worker-db-password` finds the
  worker pod.
- **`k8s deployment list --image IMAGE`**: deployments running an image,
  by `repo:tag`, tag alone, or digest. A filter on rows already read; the
  scope rules stay (one scope per call).

## Phase 5: deploy and verify

**`k8s deployment wait ID`** (`timeout: 100`): `kubectl rollout status
--timeout=<remaining>s`, then one read for the row plus `rolled_out`. Exit
0 when rolled out, 1 when Kubernetes gave up (`ProgressDeadlineExceeded`),
124 at the deadline. Note: `dd service get NAME --since ROLLED_OUT`. The
fake kubectl learns `rollout status`.

The chain: `ado run create` → `ado run wait` → `k8s deployment wait` →
`dd service get --since <rolled_out>`. A GitOps deployment updated after
the run would need `--image` to wait for the new image first; add it when a
live pipeline deploys that way.

## Phase 6: several ids in one call

Every call re-sends the agent's whole context, so a call saved is worth
more than a file not read.

- `ado file get AT…`: a build with five errors is one call, rows in order.
- `ado thread update ID… --status fixed`: resolve several threads at once.

`thread comment` stays one thread per call: each reply has its own text.

## The flows

Each becomes a world test; F8, F9 and F11 also walk by notes alone.

- **F8** A PR's build broke: `ado pr get 436` → `ado run get 8814` →
  `ado file get api@…:src/Orders/OrderClient.cs:N`.
- **F9** A failing test: `ado run get 8809` → `ado test list 8809` →
  `ado file get AT`.
- **F10** A production exception to its change: `dd log list --service api
  --status error --since …` → `ado file get AT` → `ado commit list
  api:src/Orders/OrderClient.cs` → `ado pr get 431` → its work items.
- **F11** An expired secret to the restarted pods: `dd monitor get 4712` →
  `k8s pod logs prod/web/worker-5c4d3e9f1-q8zt1 --previous` → `kv secret
  list` → `k8s pod list --kv kv-contoso-prod/worker-db-password` → `k8s
  deployment restart prod/web/worker` → `k8s deployment wait
  prod/web/worker`.
- **F12** An import error to its line: `airflow import-error list` →
  `import-error get 12` → `ado file get airflow-dags:dags/customer_sync.py:5`.
- **F13** Ship and verify: `ado run create` → `ado run wait` → `k8s
  deployment wait prod/web/api` → `dd service get api --since …`.

## Trials

After phases 1 and 2, the chains trial again with F1 to F9 and F12: fresh
Sonnet and Haiku, three runs per cell, calls and tokens per run against
the last round. After phases 3 to 5, F10, F11 and F13. A flow Haiku still
needs 15 or more calls for is a candidate for one command that does the
chain (below).

## Not now

- **Commands that run a whole chain** (an `airflow task triage` returning
  the error, its source lines, the upstream XCom and `repo_file`): only for
  a flow the trial after phase 2 shows still costs Haiku 15 or more calls.
  Not F1: its next step depends on what the comment asks.
- **A bare-value output mode** for `for id in $(…)`: no trial or script has
  asked for it, and agents have `jq`. If one does, it is a global flag (not
  `--lines`) that prints one scalar per line when `--fields` names one.
- **A `[datadog]` service-to-repository map**: only if the live org lacks
  the source code integration and the `PATH:LINE` form proves too little.
- **Several clusters per `--image` lookup**: one call per scope until
  someone runs more than two.

## Live checks (add to TODO.md as they are built)

- ADO timeline issues: `data.sourcepath`, `data.linenumber`,
  `data.columnnumber`, `data.code`, `data.logFileLineNumber`; which tasks
  fill them (`DotNetCoreCLI`, `VSBuild`, `npm` with a problem matcher).
- Test Results: `test/runs?buildUri=` without date bounds; results
  carrying `errorMessage`, `stackTrace` and `failingSince.build.id` without
  `detailsToInclude`; the `vstmr.dev.azure.com` host if `dev.azure.com`
  redirects there.
- Items `recursionLevel=Full` size and paging on the largest live repo.
- Commits `searchCriteria.itemPath` with `itemVersion` at a commit;
  `pullrequestquery` `type: commit` against `lastMergeCommit` for a commit
  inside a PR.
- Datadog: whether logs carry `git.commit.sha` and `git.repository_url`
  or only spans do (the Agent may tag a container's telemetry from its
  image's `org.opencontainers.image.revision` and `.source` labels); the
  attribute path of `error.stack` in a span search result.
- `kubectl rollout status` exit code and message for
  `ProgressDeadlineExceeded`.

## Open questions

1. `ado run get`'s `errors[]` becomes objects, which breaks the shape for
   anyone reading strings. Nothing outside this repository reads it; the
   alternative, one `at` per failed task, loses which error is where.
2. Is the `PATH:LINE` form of a dd `at` enough without the source code
   integration, or is the service map worth a config key?
3. Should `deployment list --image` read every scope when `--cluster` is
   left out? It answers "is this build deployed anywhere?" in one call, but
   breaks the scope-default rule `pick` enforces.

## Done when

- Every phase's commands are in the reference, with queries in their
  crate's `search.toml` and the search gate passing.
- F8 to F13 pass in the world; F8, F9 and F11 by notes alone.
- The trial after phase 2 gets every answer right with fewer calls than
  the last round, for both models.
