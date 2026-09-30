# Command reference

Generated from the registry by `crates/cli` (`UPDATE_DOCS=1 cargo test -p agent-cli reference`); a test fails when it is stale. Each block is what `agent-cli <domain> <resource> <verb> --help` prints: arguments (`*` required), `Returns:`, the effect, and an example.

93 commands in 8 domains. Every command also takes the globals `--fields a,b.c`, `--raw`, `--dry-run`, `--yes`, `--reveal`, `--timeout S`, `--output FILE` and `--no-cache`. Exit codes: 0 ok, 1 failed, 2 fix the call, 3 needs setup, 4 not found, 5 conflict, 124 timed out.

## ado — Azure DevOps (29 commands)

| Command | Effect | Summary |
|---|---|---|
| [`ado workitem list`](#ado-workitem-list) | read | List work items matching filters (live WIQL) |
| [`ado workitem get`](#ado-workitem-get) | read | Show a work item: fields, description as Markdown, links, latest comments |
| [`ado workitem create`](#ado-workitem-create) | write | Create a work item (bug, task, story …), optionally under a parent |
| [`ado workitem update`](#ado-workitem-update) | write | Change a work item's state, assignee, title, iteration, tags or description |
| [`ado workitem comment`](#ado-workitem-comment) | write | Add a comment to a work item (Markdown, - for stdin, or --text-file) |
| [`ado workitem link`](#ado-workitem-link) | write | Link a work item to a branch, creating the branch when missing |
| [`ado team list`](#ado-team-list) | read | List the project's teams (for [ado] team, which @current needs) |
| [`ado repo list`](#ado-repo-list) | read | List the project's Git repositories |
| [`ado repo get`](#ado-repo-get) | read | Show a repository: its URLs, default branch and branches |
| [`ado pr list`](#ado-pr-list) | read | List pull requests by repo, author, reviewer and their vote, branch or status |
| [`ado pr get`](#ado-pr-get) | read | Show a pull request: reviewers and votes, work items, policies, threads |
| [`ado pr create`](#ado-pr-create) | write | Open a pull request linked to work items, or reuse the one already open |
| [`ado pr vote`](#ado-pr-vote) | write | Record your vote on a pull request: approve, suggest, reject or none |
| [`ado pr update`](#ado-pr-update) | write | Turn auto-complete on or off, mark draft or ready, or retitle a pull request |
| [`ado pr link`](#ado-pr-link) | write | Link a work item to a pull request |
| [`ado pr comment`](#ado-pr-comment) | write | Start a comment thread on a pull request (Markdown, - for stdin, or --text-file) |
| [`ado pr complete`](#ado-pr-complete) | destructive | Complete (merge) a pull request: squash, merge or rebase |
| [`ado pr abandon`](#ado-pr-abandon) | destructive | Abandon (close) a pull request, discarding its changes |
| [`ado pipeline list`](#ado-pipeline-list) | read | List build pipelines with their last run |
| [`ado run list`](#ado-run-list) | read | List pipeline runs (builds), newest first |
| [`ado run get`](#ado-run-get) | read | Show a run: status, commit, timing, what failed, its pull request and work items |
| [`ado run logs`](#ado-run-logs) | read | Print the tail of a run's logs: failed tasks by default, or a job or task |
| [`ado run create`](#ado-run-create) | write | Start a pipeline run on a branch, with template parameters |
| [`ado run wait`](#ado-run-wait) | read | Wait for a run to finish: exit 0 if it succeeded, 1 if not, 124 if still going |
| [`ado run cancel`](#ado-run-cancel) | destructive | Cancel a run that is queued or in progress |
| [`ado run retry`](#ado-run-retry) | write | Retry the failed jobs of a finished run |
| [`ado approval list`](#ado-approval-list) | read | List pending pipeline approvals (deployment gates) |
| [`ado approval approve`](#ado-approval-approve) | destructive | Approve a pending pipeline approval, letting the stage (a deploy) run |
| [`ado approval reject`](#ado-approval-reject) | destructive | Reject a pending pipeline approval, stopping the stage |

### ado workitem list

```text
agent-cli ado workitem list — List work items matching filters (live WIQL)
  --assignee str          Name, email or @me
  --state str[]           Active, "In Progress" … (repeatable)
  --type str[]            Bug, "User Story", Task … (repeatable)
  --iteration str         Iteration path, or @current for the team's sprint
  --area str              Area path (children included)
  --tag str[]             A tag it carries (repeatable)
  --priority int[]        1 (highest) to 4 (repeatable)
  --text str              Words in the title or description
  --since time            Changed (or --date created) after this
  --until time            Changed (or --date created) before this
  --date changed|created  Which date --since and --until compare, and the newest first (default changed)
  --parent int            Children of this work item
  --wiql str              Raw WIQL WHERE clause, ANDed with the rest
  --limit int             Most rows to return (default 50)
A time is 15m, 2h, 7d, 1w (ago), now-15m, 2026-09-29, or RFC 3339.
Returns: [{id,type,title,state,assignee,iteration,area,priority,tags[],changed,rev}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli ado workitem list --assignee @me --state Active --fields id,title,state
```

### ado workitem get

```text
agent-cli ado workitem get — Show a work item: fields, description as Markdown, links, latest comments
 *<id> str        The work item's id: 1207, #1207, AB#1207 or its web URL
  --comments int  How many of the latest comments to include (default 5)
Returns: {id,type,title,state,assignee,iteration,area,priority,tags[],changed,rev,parent,children[],related[],pull_requests[{repo,id}],branches[{repo,name}],description,acceptance_criteria,comment_count,comments[{id,author,date,text}],url}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli ado workitem get 42 --fields id,title,state,description
```

### ado workitem create

```text
agent-cli ado workitem create — Create a work item (bug, task, story …), optionally under a parent
 *--type str                    Bug, Task, "User Story" …
 *--title str                   What it is called
  --parent int                  The work item it goes under
  --state str                   Active, Closed …
  --assignee str                Name, email or @me ("" unassigns)
  --iteration str               Full iteration path
  --area str                    Full area path
  --priority int                1 (highest) to 4
  --tags str                    Comma-separated; replaces the tags it has
  --description str             Markdown, stored as HTML; - reads stdin
  --description-file path       The description from a Markdown file
  --acceptance-criteria str     Markdown, stored as HTML; - reads stdin
  --acceptance-criteria-file path  The acceptance criteria from a Markdown file
Returns: {id,type,title,state,assignee,iteration,area,priority,tags[],changed,rev}
Write: --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli ado workitem create --type Bug --title 'Login fails on Safari' --priority 2
```

### ado workitem update

```text
agent-cli ado workitem update — Change a work item's state, assignee, title, iteration, tags or description
 *<id> str                      The work item's id: 1207, #1207, AB#1207 or its web URL
  --title str                   A new title
  --state str                   Active, Closed …
  --assignee str                Name, email or @me ("" unassigns)
  --iteration str               Full iteration path
  --area str                    Full area path
  --priority int                1 (highest) to 4
  --tags str                    Comma-separated; replaces the tags it has
  --description str             Markdown, stored as HTML; - reads stdin
  --description-file path       The description from a Markdown file
  --acceptance-criteria str     Markdown, stored as HTML; - reads stdin
  --acceptance-criteria-file path  The acceptance criteria from a Markdown file
  --if-rev int                  Refuse unless it is still at this rev (from workitem get)
Returns: {id,type,title,state,assignee,iteration,area,priority,tags[],changed,rev}
Write: --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli ado workitem update 42 --state Active --assignee @me --if-rev 7
```

### ado workitem comment

```text
agent-cli ado workitem comment — Add a comment to a work item (Markdown, - for stdin, or --text-file)
 *<id> str          The work item's id: 1207, #1207, AB#1207 or its web URL
  <text> str        Markdown, or - to read stdin (posted as a code block, 64 KiB max)
  --text-file path  The comment from a Markdown file (64 KiB max)
Returns: {work_item,id,date}
Write: --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli ado workitem comment 42 'Fixed in !17; deploying tomorrow'
```

### ado workitem link

```text
agent-cli ado workitem link — Link a work item to a branch, creating the branch when missing
 *<id> str      The work item's id: 1207, #1207, AB#1207 or its web URL
 *--repo str    The repository, by name
  --branch str  Default {id}-{title-slug}; made from the default branch if missing
Returns: {work_item,repo,branch,branch_created,already_linked}
Write: --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli ado workitem link 42 --repo web --branch 42-fix-login
```

### ado team list

```text
agent-cli ado team list — List the project's teams (for [ado] team, which @current needs)
  --limit int  Most rows to return (default 50)
Returns: [{name,id,description}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli ado team list --fields name
```

### ado repo list

```text
agent-cli ado repo list — List the project's Git repositories
  <pattern> str  Only names containing this
  --limit int    Most rows to return (default 50)
Returns: [{name,id,default_branch,size,is_disabled,web_url}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli ado repo list web --fields name,default_branch
```

### ado repo get

```text
agent-cli ado repo get — Show a repository: its URLs, default branch and branches
 *<name> str  The repository's name or id
Returns: {name,id,project,default_branch,size,is_disabled,remote_url,ssh_url,web_url,branches[]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli ado repo get web --fields remote_url,default_branch
```

### ado pr list

```text
agent-cli ado pr list — List pull requests by repo, author, reviewer and their vote, branch or status
  --repo str                    The repository, by name
  --status active|completed|abandoned|all  Which pull requests (default active)
  --author str                  Who opened it: name, email or @me
  --reviewer str                A reviewer: name, email or @me
  --vote approved|suggestions|waiting|rejected|none[]  The --reviewer's own vote (@me's without one); none is not yet voted (repeatable)
  --target str                  The branch it merges into
  --source str                  The branch it merges from
  --draft true|false            True for drafts only, false for none
  --since time                  Opened after this
  --until time                  Opened before this
  --limit int                   Most rows to return (default 50)
A time is 15m, 2h, 7d, 1w (ago), now-15m, 2026-09-29, or RFC 3339.
Returns: [{id,repo,title,author,status,is_draft,source,target,merge_status,auto_complete,created,reviewers[{name,vote,required}],url}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli ado pr list --vote none --fields id,title,author,repo
```

### ado pr get

```text
agent-cli ado pr get — Show a pull request: reviewers and votes, work items, policies, threads
 *<id> str        The pull request's id: 431, #431 or its web URL
  --comments int  How many of the latest open threads to include (default 5)
Returns: {id,repo,title,author,status,is_draft,source,target,merge_status,auto_complete,created,reviewers[{name,vote,required}],url,description,work_items[],policies[{name,status,run_id}],open_threads,threads[{id,status,author,file,line,date,text,replies}],last_merge_source_commit}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli ado pr get 42 --fields title,status,reviewers,policies
```

### ado pr create

```text
agent-cli ado pr create — Open a pull request linked to work items, or reuse the one already open
 *--repo str               The repository, by name
 *--source str             The branch it merges from
 *--title str              The pull request's title
  --target str             The branch it merges into (default: the repo's default branch)
  --description str        Markdown; - reads stdin
  --description-file path  The description from a Markdown file
  --workitem int[]         A work item to link (repeatable)
  --draft true|false       Open it as a draft
Returns: {id,url,repo,source,target,status,is_draft,created,work_items[]}
Write: --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli ado pr create --repo web --source 42-fix-login --title 'Fix login' --workitem 42
```

### ado pr vote

```text
agent-cli ado pr vote — Record your vote on a pull request: approve, suggest, reject or none
 *<id> str                      The pull request's id: 431, #431 or its web URL
 *<vote> approve|suggest|wait|reject|none  Your vote (none withdraws it)
Returns: {id,vote}
Write: --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli ado pr vote 42 approve
```

### ado pr update

```text
agent-cli ado pr update — Turn auto-complete on or off, mark draft or ready, or retitle a pull request
 *<id> str                 The pull request's id: 431, #431 or its web URL
  --autocomplete on|off    Complete it by itself once policies pass
  --draft true|false       True to make it a draft, false to publish it
  --title str              A new title
  --description str        Markdown, replacing the description; - reads stdin
  --description-file path  The description from a Markdown file
Returns: {id,repo,title,author,status,is_draft,source,target,merge_status,auto_complete,created,reviewers[{name,vote,required}],url}
Write: --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli ado pr update 42 --autocomplete on
```

### ado pr link

```text
agent-cli ado pr link — Link a work item to a pull request
 *<id> str        The pull request's id: 431, #431 or its web URL
 *--workitem int  The work item to link
Returns: {pr,work_item,already_linked}
Write: --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli ado pr link 17 --workitem 42
```

### ado pr comment

```text
agent-cli ado pr comment — Start a comment thread on a pull request (Markdown, - for stdin, or --text-file)
 *<id> str          The pull request's id: 431, #431 or its web URL
  <text> str        Markdown, or - to read stdin (posted as a code block, 64 KiB max)
  --text-file path  The comment from a Markdown file (64 KiB max)
Returns: {pr,thread_id}
Write: --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli ado pr comment 42 'Tests pass locally; ready for review'
```

### ado pr complete

```text
agent-cli ado pr complete — Complete (merge) a pull request: squash, merge or rebase
 *<id> str                      The pull request's id: 431, #431 or its web URL
  --strategy squash|merge|rebase  How it lands on the target (default squash)
  --keep-source                 Keep the source branch instead of deleting it
  --no-transition               Leave the linked work items' states alone
Returns: {id,repo,title,author,status,is_draft,source,target,merge_status,auto_complete,created,reviewers[{name,vote,required}],url}
Destructive: needs --yes; --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli ado pr complete 42 --strategy squash --yes
```

### ado pr abandon

```text
agent-cli ado pr abandon — Abandon (close) a pull request, discarding its changes
 *<id> str  The pull request's id: 431, #431 or its web URL
Returns: {id,repo,title,author,status,is_draft,source,target,merge_status,auto_complete,created,reviewers[{name,vote,required}],url}
Destructive: needs --yes; --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli ado pr abandon 42 --yes
```

### ado pipeline list

```text
agent-cli ado pipeline list — List build pipelines with their last run
  <pattern> str  Only names containing this
  --repo str     Only pipelines that build this repository
  --limit int    Most rows to return (default 50)
Returns: [{id,name,folder,queue_status,last_run{id,status,result,branch,finished},url}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli ado pipeline list --repo web --fields id,name,last_run.result
```

### ado run list

```text
agent-cli ado run list — List pipeline runs (builds), newest first
  --pipeline str                Pipeline name or id
  --branch str                  The branch or tag it built: main, v1.4.2, refs/heads/main, refs/tags/v1.4.2
  --since time                  Queued after this
  --until time                  Queued before this
  --status inProgress|notStarted|cancelling|completed|all  Runs in this state
  --result succeeded|partiallySucceeded|failed|canceled  Finished runs with this result
  --requested-by str            Who queued it: name, email or @me
  --reason manual|individualCI|batchedCI|schedule|pullRequest|buildCompletion|resourceTrigger  Why it ran: a push (individualCI, batchedCI), a PR, a schedule, by hand …
  --limit int                   Most rows to return (default 50)
A time is 15m, 2h, 7d, 1w (ago), now-15m, 2026-09-29, or RFC 3339.
Returns: [{id,pipeline,pipeline_id,build_number,status,result,branch,commit,requested_by,reason,queued,started,finished,url}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli ado run list --branch refs/tags/v1.4.2 --fields id,pipeline,status,result,finished
```

### ado run get

```text
agent-cli ado run get — Show a run: status, commit, timing, what failed, its pull request and work items
 *<id> str  The run's id: 8812, #8812 or its web URL
Returns: {id,pipeline,pipeline_id,build_number,status,result,branch,commit,requested_by,reason,queued,started,finished,url,pr{id,title,status},workitems[{id,type,title,state}],running[],failed[{type,name,log_id,errors[]}]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli ado run get 1234 --fields status,result,commit,pr,workitems,failed
```

### ado run logs

```text
agent-cli ado run logs — Print the tail of a run's logs: failed tasks by default, or a job or task
 *<id> str    The run's id: 8812, #8812 or its web URL
  --job str   A job's log, by name
  --task str  A task's log, by name
  --tail int  Lines from the end of each log (default 200)
Returns: {run,logs[],text}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli ado run logs 1234 --task 'Run tests' --tail 100
```

### ado run create

```text
agent-cli ado run create — Start a pipeline run on a branch, with template parameters
 *--pipeline str  Pipeline name or id
  --branch str    The branch to build (default: the pipeline's)
  --param str[]   A template parameter as name=value (repeatable)
Returns: {id,pipeline,pipeline_id,build_number,status,result,branch,commit,requested_by,reason,queued,started,finished,url}
Write: --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli ado run create --pipeline web-ci --branch 42-fix-login
```

### ado run wait

```text
agent-cli ado run wait — Wait for a run to finish: exit 0 if it succeeded, 1 if not, 124 if still going
 *<id> str  The run's id: 8812, #8812 or its web URL
Returns: {id,pipeline,pipeline_id,build_number,status,result,branch,commit,requested_by,reason,queued,started,finished,url}
Read. * required. Globals: --fields --raw --timeout (default 100s) --output
e.g. agent-cli ado run wait 1234 --fields id,status,result
```

### ado run cancel

```text
agent-cli ado run cancel — Cancel a run that is queued or in progress
 *<id> str  The run's id: 8812, #8812 or its web URL
Returns: {id,pipeline,pipeline_id,build_number,status,result,branch,commit,requested_by,reason,queued,started,finished,url}
Destructive: needs --yes; --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli ado run cancel 1234 --yes
```

### ado run retry

```text
agent-cli ado run retry — Retry the failed jobs of a finished run
 *<id> str  The run's id: 8812, #8812 or its web URL
Returns: {id,pipeline,pipeline_id,build_number,status,result,branch,commit,requested_by,reason,queued,started,finished,url}
Write: --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli ado run retry 1234
```

### ado approval list

```text
agent-cli ado approval list — List pending pipeline approvals (deployment gates)
  --limit int  Most rows to return (default 50)
Returns: [{id,pipeline,run_id,run,instructions,created,approvers[{name,status}]}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli ado approval list --fields id,pipeline,run,approvers
```

### ado approval approve

```text
agent-cli ado approval approve — Approve a pending pipeline approval, letting the stage (a deploy) run
 *<id> str       The approval's id, from approval list
  --comment str  Why, for the record
Returns: {id,status}
Destructive: needs --yes; --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli ado approval approve 1f2e3d4c-0000-4000-8000-000000000001 --comment 'Checked staging' --yes
```

### ado approval reject

```text
agent-cli ado approval reject — Reject a pending pipeline approval, stopping the stage
 *<id> str       The approval's id, from approval list
  --comment str  Why, for the record
Returns: {id,status}
Destructive: needs --yes; --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli ado approval reject 1f2e3d4c-0000-4000-8000-000000000001 --comment 'Not today' --yes
```

## kv — Key Vault (4 commands)

| Command | Effect | Summary |
|---|---|---|
| [`kv secret list`](#kv-secret-list) | read | List Key Vault secrets with expiry and tags (names and metadata, never values) |
| [`kv secret get`](#kv-secret-get) | reveal | Get a Key Vault secret's value (needs --reveal, or --output FILE) |
| [`kv version list`](#kv-version-list) | read | List a secret's versions, newest first (when it was rotated; never values) |
| [`kv vault list`](#kv-vault-list) | read | List the key vaults the az login reaches (within [azure] vaults) |

### kv secret list

```text
agent-cli kv secret list — List Key Vault secrets with expiry and tags (names and metadata, never values)
  <name> str                 Part of the secret name, any case; or a secret's id (vault/name), exactly
  --vault str[]              Only this vault (repeatable; within [azure] vaults)
  --expires-within duration  Only secrets expiring within this long, or already expired
  --expired                  Only secrets whose expiry has passed
  --disabled                 Only disabled secrets
  --tag str[]                Only secrets tagged key, or key=value, any case (repeatable)
  --content-type str         Only secrets whose content type contains this, any case
  --managed true|false       True for certificates' backing secrets only, false for none of them
  --limit int                (default 50)
A duration is 500ms, 30s, 15m, 2h, 7d, 1w.
Returns: [{id,vault,name,enabled,content_type,expires,created,updated,managed,tags}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli kv secret list db --expires-within 30d --fields id,expires
```

### kv secret get

```text
agent-cli kv secret get — Get a Key Vault secret's value (needs --reveal, or --output FILE)
 *<name> str     The secret: its name, its id (vault/name[/version]) or its URI
  --vault str    The vault that holds it; needed when more than one does
  --version str  A version id from `kv version list`; the current one when left out
Returns: {vault,name,version,content_type,value}
Reveals a secret: --reveal prints it; --output FILE saves it (0600) and prints only the path. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli kv secret get kv-contoso-dev/db-password --fields value --output db-password.txt
```

### kv version list

```text
agent-cli kv version list — List a secret's versions, newest first (when it was rotated; never values)
 *<secret> str  The secret: its name, its id (vault/name) or its URI
  --vault str   The vault that holds it; needed when more than one does
  --limit int   (default 50)
Returns: [{id,version,enabled,created,updated,expires}]
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli kv version list kv-contoso-dev/db-password --fields id,created,enabled
```

### kv vault list

```text
agent-cli kv vault list — List the key vaults the az login reaches (within [azure] vaults)
  --limit int  (default 50)
Returns: [{name,subscription,resource_group,location,uri}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli kv vault list --fields name,resource_group,uri
```

## acr — Container Registry (4 commands)

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

## aks — AKS (2 commands)

| Command | Effect | Summary |
|---|---|---|
| [`aks cluster list`](#aks-cluster-list) | read | List the AKS clusters the az login reaches, with version and power state |
| [`aks cluster connect`](#aks-cluster-connect) | write | Fetch an AKS cluster's kubeconfig credentials and print its [[k8s.scope]] |

### aks cluster list

```text
agent-cli aks cluster list — List the AKS clusters the az login reaches, with version and power state
  --limit int  (default 50)
Returns: [{name,resource_group,subscription,location,kubernetes_version,power_state,k8s_scope}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli aks cluster list --fields name,k8s_scope,power_state
```

### aks cluster connect

```text
agent-cli aks cluster connect — Fetch an AKS cluster's kubeconfig credentials and print its [[k8s.scope]]
 *<name> str            The cluster's name, as `aks cluster list` shows it
  --resource-group str  Its resource group; looked up by name when left out
  --subscription str    Its subscription id; the az default, or looked up with the group
Returns: {cluster,resource_group,subscription,context,kubelogin,namespaces[],config}
Write: --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli aks cluster connect aks-contoso-dev --resource-group rg-contoso --fields context,config
```

## k8s — Kubernetes (13 commands)

| Command | Effect | Summary |
|---|---|---|
| [`k8s pod list`](#k8s-pod-list) | read | List pods with status, ready, restarts, age, node and owning deployment |
| [`k8s pod get`](#k8s-pod-get) | read | Describe a pod: containers, images, states, last termination reason, owner |
| [`k8s pod logs`](#k8s-pod-logs) | read | Read the last lines of a pod's log, or of the run before its last restart |
| [`k8s pod delete`](#k8s-pod-delete) | destructive | Delete a pod so its controller replaces it (a bare pod is gone for good) |
| [`k8s event list`](#k8s-event-list) | read | List Kubernetes events, newest first: warnings, back-offs, failed pulls |
| [`k8s deployment list`](#k8s-deployment-list) | read | List deployments: ready pods, images with tag and digest, when they rolled out |
| [`k8s deployment restart`](#k8s-deployment-restart) | destructive | Rollout-restart a deployment, replacing its pods one at a time |
| [`k8s deployment scale`](#k8s-deployment-scale) | destructive | Scale a deployment to a number of replicas |
| [`k8s configmap list`](#k8s-configmap-list) | read | List configmaps and their keys |
| [`k8s configmap get`](#k8s-configmap-get) | read | Show a configmap's keys and values (binary keys show their size only) |
| [`k8s secret list`](#k8s-secret-list) | read | List Kubernetes secrets: type, key names and sizes (never values) |
| [`k8s secret get`](#k8s-secret-get) | reveal | Decode one key of a Kubernetes secret (needs --reveal, or --output FILE) |
| [`k8s context list`](#k8s-context-list) | read | List the configured cluster scopes and whether kubectl knows each context |

### k8s pod list

```text
agent-cli k8s pod list — List pods with status, ready, restarts, age, node and owning deployment
  <name> str       Part of the pod name
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
  --label str[]    Only pods with this label: app=api, or a key alone (repeatable, all must hold)
  --limit int      (default 50)
Returns: [{id,name,namespace,status,ready,restarts,age,node,owner}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s pod list --cluster qa --namespace dev --fields id,status,restarts,owner
```

### k8s pod get

```text
agent-cli k8s pod get — Describe a pod: containers, images, states, last termination reason, owner
 *<pod> str        The pod: its id (cluster/namespace/name), namespace/name, or name
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
  --yaml           The manifest as YAML text instead
Returns: {id,name,namespace,status,ready,restarts,age,node,ip,owner,containers[{name,image,digest,ready,restarts,state,last_termination}],secret_refs[{via,secret,keys[],class,kv[]}],conditions[{type,status,reason,message}],labels,text}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s pod get qa/dev/orders-api-7d9f5b-abc12 --fields status,containers,secret_refs
```

### k8s pod logs

```text
agent-cli k8s pod logs — Read the last lines of a pod's log, or of the run before its last restart
 *<pod> str        The pod: its id (cluster/namespace/name), namespace/name, or name
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
  --container str  Which container; kubectl picks the default one when left out
  --tail int       How many of the last lines (default 200)
  --previous       The run before the last restart: where a crash loop says why
  --since time     Only lines logged after this
A time is 15m, 2h, 7d, 1w (ago), now-15m, 2026-09-29, or RFC 3339.
Returns: {pod,namespace,container,lines,text}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s pod logs qa/dev/orders-worker-5c4d3e-q8zt --previous --tail 50
```

### k8s pod delete

```text
agent-cli k8s pod delete — Delete a pod so its controller replaces it (a bare pod is gone for good)
 *<pod> str        The pod: its id (cluster/namespace/name), namespace/name, or name
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
Returns: {cluster,namespace,object,said,replicas,previous}
Destructive: needs --yes; --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s pod delete orders-api-7d9f5b-abc12 --cluster qa --namespace dev
```

### k8s event list

```text
agent-cli k8s event list — List Kubernetes events, newest first: warnings, back-offs, failed pulls
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
  --pod str        Only events about this pod: its id, namespace/name or name
  --since time     Only events last seen after this
  --limit int      (default 50)
A time is 15m, 2h, 7d, 1w (ago), now-15m, 2026-09-29, or RFC 3339.
Returns: [{type,reason,object,message,count,last_seen,namespace}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s event list --pod qa/dev/orders-worker-5c4d3e-q8zt --since 1h --fields type,reason,message,last_seen
```

### k8s deployment list

```text
agent-cli k8s deployment list — List deployments: ready pods, images with tag and digest, when they rolled out
  <name> str       Part of the deployment name
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
  --since time     Only deployments that rolled out after this
  --limit int      (default 50)
A time is 15m, 2h, 7d, 1w (ago), now-15m, 2026-09-29, or RFC 3339.
Returns: [{id,name,namespace,ready,replicas,images[{container,image,digest}],updated}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s deployment list --cluster prod --namespace web --fields id,ready,images,updated
```

### k8s deployment restart

```text
agent-cli k8s deployment restart — Rollout-restart a deployment, replacing its pods one at a time
 *<name> str       A deployment's name or id (cluster/namespace/name), or statefulset/NAME, daemonset/NAME
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
Returns: {cluster,namespace,object,said,replicas,previous}
Destructive: needs --yes; --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s deployment restart orders-api --cluster qa --namespace dev
```

### k8s deployment scale

```text
agent-cli k8s deployment scale — Scale a deployment to a number of replicas
 *<name> str       A deployment's name or id (cluster/namespace/name), or statefulset/NAME, replicaset/NAME
 *--replicas int   How many pods it should run
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
Returns: {cluster,namespace,object,said,replicas,previous}
Destructive: needs --yes; --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s deployment scale orders-api --replicas 3 --cluster qa --namespace dev
```

### k8s configmap list

```text
agent-cli k8s configmap list — List configmaps and their keys
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
  --limit int      (default 50)
Returns: [{id,name,namespace,keys[],age}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s configmap list --cluster qa --namespace dev --fields id,keys
```

### k8s configmap get

```text
agent-cli k8s configmap get — Show a configmap's keys and values (binary keys show their size only)
 *<name> str       The configmap: its id (cluster/namespace/name), namespace/name, or name
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
Returns: {id,name,namespace,age,data}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s configmap get qa/dev/orders-config
```

### k8s secret list

```text
agent-cli k8s secret list — List Kubernetes secrets: type, key names and sizes (never values)
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
  --limit int      (default 50)
Returns: [{id,name,namespace,type,keys,age}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s secret list --cluster qa --namespace dev --fields id,type,keys
```

### k8s secret get

```text
agent-cli k8s secret get — Decode one key of a Kubernetes secret (needs --reveal, or --output FILE)
 *<name> str       The secret: its id (cluster/namespace/name), namespace/name, or name
 *<key> str        Which key to decode
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
Returns: {secret,key,namespace,value,encoding}
Reveals a secret: --reveal prints it; --output FILE saves it (0600) and prints only the path. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s secret get orders-db password --cluster qa --namespace dev --fields value --output orders-db.txt
```

### k8s context list

```text
agent-cli k8s context list — List the configured cluster scopes and whether kubectl knows each context
Returns: [{name,context,namespaces[],known}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s context list --fields name,context,namespaces,known
```

## sql — SQL Server/Oracle (6 commands)

| Command | Effect | Summary |
|---|---|---|
| [`sql query run`](#sql-query-run) | read or write | Run SQL on a connection and return every result set as JSON |
| [`sql query bench`](#sql-query-bench) | read or write | Time a query over several runs: connect, first row and total latency |
| [`sql object list`](#sql-object-list) | read | List tables and views, procedures, functions, packages and sequences |
| [`sql object get`](#sql-object-get) | read | Show a view, procedure, function or package's source, or a table's columns |
| [`sql schema list`](#sql-schema-list) | read | List the schemas (owners) of a database |
| [`sql connection list`](#sql-connection-list) | read | List the configured database connections (never their passwords) |

### sql query run

```text
agent-cli sql query run — Run SQL on a connection and return every result set as JSON
  <sql> str        The SQL, or - to read it from stdin. SQL Server splits at GO lines, Oracle at ; and / lines
  --conn str       Connection name from `sql connection list`; defaults to the only one
  --max-rows int   Keep at most this many rows of each result set (default 1000)
  --sql-file path  The SQL from a file
Returns: {results[{columns[],types[],rows[],rows_affected,truncated}],elapsed_ms}
Read or write, decided by the input: a write honours --dry-run and may need --yes. Globals: --fields --raw --timeout --output
e.g. agent-cli sql query run --conn local-mssql 'select top 5 id, name from bench.customers'
```

### sql query bench

```text
agent-cli sql query bench — Time a query over several runs: connect, first row and total latency
  <sql> str        The SQL, or - to read it from stdin
  --conn str       Connection name from `sql connection list`; defaults to the only one
  --runs int       How many times to run it on one connection (default 20)
  --max-rows int   Keep at most this many rows of each result set, ending the read there (default: read every row each run)
  --sql-file path  The SQL from a file
Returns: {runs,requested,rows,phases[{phase,min_ms,p50_ms,p95_ms,max_ms}]}
Read or write, decided by the input: a write honours --dry-run and may need --yes. Globals: --fields --raw --timeout --output
e.g. agent-cli sql query bench --conn local-mssql --runs 5 'select count(*) from bench.orders'
```

### sql object list

```text
agent-cli sql object list — List tables and views, procedures, functions, packages and sequences
  <pattern> str                 Only names containing this, any case
  --conn str                    Connection name from `sql connection list`; defaults to the only one
  --schema str                  Only this schema (owner on Oracle)
  --kind table|view|procedure|function|package|sequence  Only this kind
  --limit int                   (default 50)
Returns: [{id,schema,kind,name,modified}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli sql object list --conn local-mssql customer --kind table --fields schema,name
```

### sql object get

```text
agent-cli sql object get — Show a view, procedure, function or package's source, or a table's columns
 *<object> str  schema.name; either part may be quoted as [x] or "x"
  --conn str    Connection name from `sql connection list`; defaults to the only one
Returns: {kind,schema,name,text,columns[{name,type,nullable,pk}],ddl}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli sql object get --conn local-mssql bench.customers --fields kind,columns,ddl
```

### sql schema list

```text
agent-cli sql schema list — List the schemas (owners) of a database
  --conn str   Connection name from `sql connection list`; defaults to the only one
  --limit int  (default 50)
Returns: [str]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli sql schema list --conn local-mssql
```

### sql connection list

```text
agent-cli sql connection list — List the configured database connections (never their passwords)
Returns: [{name,kind,host,port,database,service,user,read_only}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli sql connection list --fields name,kind,host,read_only
```

## airflow — Apache Airflow (15 commands)

| Command | Effect | Summary |
|---|---|---|
| [`airflow instance list`](#airflow-instance-list) | read | List the configured Airflow instances (from config, no network) |
| [`airflow dag list`](#airflow-dag-list) | read | List DAGs with their schedule, next run and whether they are paused |
| [`airflow dag get`](#airflow-dag-get) | read | Show a DAG's status: paused, next run, params, schedule and its last five runs |
| [`airflow dag update`](#airflow-dag-update) | write | Pause or unpause a DAG |
| [`airflow run list`](#airflow-run-list) | read | List DAG runs, newest first, across DAGs or for one |
| [`airflow run get`](#airflow-run-get) | read | Show a DAG run's state, task counts by state, and the tasks that failed |
| [`airflow run create`](#airflow-run-create) | write | Trigger a DAG run, with a conf and optionally a logical date |
| [`airflow run wait`](#airflow-run-wait) | read | Wait for a DAG run: exit 0 if it succeeded, 1 if it failed, 124 if still going |
| [`airflow run retry`](#airflow-run-retry) | write | Retry the failed and upstream_failed tasks of a DAG run |
| [`airflow task list`](#airflow-task-list) | read | List a DAG run's task instances with their state, try, times and host |
| [`airflow task get`](#airflow-task-get) | read | Show a task instance: its tries, its pod, and what blocks it if it is stuck |
| [`airflow task logs`](#airflow-task-logs) | read | Read the tail of a task instance's log, with the exception that failed it |
| [`airflow task retry`](#airflow-task-retry) | destructive | Clear task instances in any state, and their downstream, so they run again |
| [`airflow import-error list`](#airflow-import-error-list) | read | List DAG files that fail to import, newest first: why a DAG is missing |
| [`airflow import-error get`](#airflow-import-error-get) | read | Show an import error's full stack trace |

### airflow instance list

```text
agent-cli airflow instance list — List the configured Airflow instances (from config, no network)
Returns: [{name,base_url,auth,read_only,k8s_scope,k8s_namespace}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli airflow instance list --fields name,base_url,read_only
```

### airflow dag list

```text
agent-cli airflow dag list — List DAGs with their schedule, next run and whether they are paused
  <pattern> str                 Only DAG ids containing this
  --tag str[]                   Only DAGs with this tag (repeatable; any of them)
  --paused true|false           true for paused DAGs only, false for active ones only
  --last-state queued|running|success|failed  Only DAGs whose last run ended in this state
  --instance str                The [[airflow.instance]] name; defaults to the only one
  --limit int                   (default 50)
Returns: [{id,paused,schedule,next_run,tags[],owners[],file,import_errors}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli airflow dag list --last-state failed --fields id,schedule,next_run
```

### airflow dag get

```text
agent-cli airflow dag get — Show a DAG's status: paused, next run, params, schedule and its last five runs
 *<dag> str       The DAG: its id, or its Airflow UI URL
  --instance str  The [[airflow.instance]] name; defaults to the only one
Returns: {id,paused,schedule,schedule_text,next_run,next_logical_date,catchup,max_active_runs,owners[],tags[],file,bundle,version,last_parsed,import_errors,description,params[{name,default,description}],recent_runs[{id,state,type,run_after,duration}]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli airflow dag get etl_nightly --fields paused,next_run,recent_runs
```

### airflow dag update

```text
agent-cli airflow dag update — Pause or unpause a DAG
 *<dag> str            The DAG: its id, or its Airflow UI URL
 *--paused true|false  true pauses the DAG, false unpauses it
  --instance str       The [[airflow.instance]] name; defaults to the only one
Returns: {id,paused,next_run}
Write: --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli airflow dag update etl_nightly --paused true
```

### airflow run list

```text
agent-cli airflow run list — List DAG runs, newest first, across DAGs or for one
  --dag str                     Only this DAG's runs (default: every DAG)
  --state queued|running|success|failed[]  Only runs in this state (repeatable)
  --type scheduled|manual|backfill|asset_triggered[]  Only runs of this type (repeatable)
  --since time                  Only runs due at or after this (their run_after)
  --until time                  Only runs due at or before this
  --instance str                The [[airflow.instance]] name; defaults to the only one
  --limit int                   (default 50)
A time is 15m, 2h, 7d, 1w (ago), now-15m, 2026-09-29, or RFC 3339.
Returns: [{id,state,type,run_after,logical_date,start,end,duration,triggered_by}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli airflow run list --dag etl_nightly --state failed --since 1d --fields id,state,end
```

### airflow run get

```text
agent-cli airflow run get — Show a DAG run's state, task counts by state, and the tasks that failed
 *<run> str       The run: DAG/RUN (DAG/latest for the newest), or its Airflow UI URL
  --dag str       The DAG, when RUN is a bare run id
  --instance str  The [[airflow.instance]] name; defaults to the only one
Returns: {id,state,type,run_after,logical_date,start,end,duration,triggered_by,user,conf,note,dag_version,tasks,failed[]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli airflow run get etl_nightly/latest --fields id,state,tasks,failed
```

### airflow run create

```text
agent-cli airflow run create — Trigger a DAG run, with a conf and optionally a logical date
 *<dag> str           The DAG: its id, or its Airflow UI URL
  --conf str          The run's conf: a JSON object, or - to read it from stdin
  --conf-file path    The run's conf from a JSON file
  --logical-date str  RFC 3339 or now; a DAG that templates {{ ds }} needs one (none by default)
  --run-id str        A run id of your own; Airflow makes a manual__ one otherwise
  --note str          A note on the run
  --instance str      The [[airflow.instance]] name; defaults to the only one
Returns: {id,state,run_after,logical_date,conf}
Write: --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli airflow run create etl_nightly --conf '{"day":"2026-09-28"}'
```

### airflow run wait

```text
agent-cli airflow run wait — Wait for a DAG run: exit 0 if it succeeded, 1 if it failed, 124 if still going
 *<run> str       The run: DAG/RUN (DAG/latest for the newest), or its Airflow UI URL
  --dag str       The DAG, when RUN is a bare run id
  --instance str  The [[airflow.instance]] name; defaults to the only one
Returns: {id,state,done,waited_s,duration,failed[]}
Read. * required. Globals: --fields --raw --timeout (default 100s) --output
e.g. agent-cli airflow run wait etl_nightly/latest
```

### airflow run retry

```text
agent-cli airflow run retry — Retry the failed and upstream_failed tasks of a DAG run
 *<run> str       The run: DAG/RUN (DAG/latest for the newest), or its Airflow UI URL
  --dag str       The DAG, when RUN is a bare run id
  --instance str  The [[airflow.instance]] name; defaults to the only one
Returns: {id,cleared[]}
Write: --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli airflow run retry etl_nightly/latest
```

### airflow task list

```text
agent-cli airflow task list — List a DAG run's task instances with their state, try, times and host
 *<run> str                     The run: DAG/RUN (DAG/latest for the newest), or its Airflow UI URL
  --dag str                     The DAG, when RUN is a bare run id
  --state none|removed|scheduled|queued|running|success|restarting|failed|up_for_retry|up_for_reschedule|upstream_failed|skipped|…[]  Only task instances in this state (repeatable)
  --instance str                The [[airflow.instance]] name; defaults to the only one
  --limit int                   (default 50)
Returns: [{id,state,try_number,max_tries,start,end,duration,operator,hostname}]
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli airflow task list etl_nightly/latest --state failed --fields id,end,hostname
```

### airflow task get

```text
agent-cli airflow task get — Show a task instance: its tries, its pod, and what blocks it if it is stuck
 *<task> str      The task instance: DAG/RUN/TASK[:MAP][/TRY], or its Airflow UI URL
  --dag str       The DAG, when the id leaves it out
  --run str       The run id, when the id leaves it out
  --instance str  The [[airflow.instance]] name; defaults to the only one
  --rendered      Add the rendered template fields (large)
Returns: {id,state,try_number,max_tries,start,end,duration,operator,executor,queue,pool,hostname,pod,note,tries[{try_number,state,start,end,hostname}],blocked_by[{name,reason}],rendered_fields}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli airflow task get etl_nightly/latest/load_orders --fields state,blocked_by,tries,pod
```

### airflow task logs

```text
agent-cli airflow task logs — Read the tail of a task instance's log, with the exception that failed it
 *<task> str      The task instance: DAG/RUN/TASK[:MAP][/TRY], or its Airflow UI URL
  --dag str       The DAG, when the id leaves it out
  --run str       The run id, when the id leaves it out
  --instance str  The [[airflow.instance]] name; defaults to the only one
  --tail int      How many of the last lines (0 for all) (default 200)
Returns: {id,state,error,lines,kept,complete,sources[],text}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli airflow task logs etl_nightly/latest/load_orders --tail 50
```

### airflow task retry

```text
agent-cli airflow task retry — Clear task instances in any state, and their downstream, so they run again
 *<tasks> str[]    Task instances of one run: DAG/RUN/TASK[:MAP] (repeat for more)
  --dag str        The DAG, when the ids leave it out
  --run str        The run id, when the ids leave it out
  --no-downstream  Clear only these tasks, not the tasks downstream of them
  --instance str   The [[airflow.instance]] name; defaults to the only one
Returns: {run,cleared[]}
Destructive: needs --yes; --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli airflow task retry etl_nightly/latest/load_orders --yes
```

### airflow import-error list

```text
agent-cli airflow import-error list — List DAG files that fail to import, newest first: why a DAG is missing
  --instance str  The [[airflow.instance]] name; defaults to the only one
  --limit int     (default 50)
Returns: [{id,file,bundle,timestamp,error}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli airflow import-error list --fields id,file,error
```

### airflow import-error get

```text
agent-cli airflow import-error get — Show an import error's full stack trace
 *<id> int        The import error's id, from import-error list
  --instance str  The [[airflow.instance]] name; defaults to the only one
Returns: {id,file,bundle,timestamp,error,stack_trace}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli airflow import-error get 12
```

## dd — Datadog (20 commands)

| Command | Effect | Summary |
|---|---|---|
| [`dd log list`](#dd-log-list) | read | Search Datadog logs in a time window, newest first, as compact rows |
| [`dd log-count list`](#dd-log-count-list) | read | Count Datadog logs grouped by facets such as service or status |
| [`dd metric list`](#dd-metric-list) | read | Find Datadog metric names reporting recently, by part of the name |
| [`dd metric get`](#dd-metric-get) | read | Query a Datadog metric: min, max, avg, last and a few points per series |
| [`dd monitor list`](#dd-monitor-list) | read | List Datadog monitors alerting or not, by state or tag, with when each triggered |
| [`dd monitor get`](#dd-monitor-get) | read | Show a Datadog monitor: its query, state, the groups that triggered, downtimes |
| [`dd downtime list`](#dd-downtime-list) | read | List Datadog downtimes: which monitors are muted, for what scope and until when |
| [`dd downtime create`](#dd-downtime-create) | destructive | Mute a Datadog monitor for a bounded time (--for, at most 7 days) |
| [`dd downtime cancel`](#dd-downtime-cancel) | write | Unmute a Datadog monitor now, ending its downtime before it runs out |
| [`dd event list`](#dd-event-list) | read | List Datadog events: deploys, monitor alerts, kubernetes events kept past 1h |
| [`dd service list`](#dd-service-list) | read | List the APM services that send traces to Datadog |
| [`dd service get`](#dd-service-get) | read | Show an APM service's health: request count, error rate, p50/p95/p99 latency |
| [`dd span list`](#dd-span-list) | read | Search APM spans: failing or slow requests, or every span of one trace |
| [`dd incident list`](#dd-incident-list) | read | List Datadog incidents, active and stable by default |
| [`dd incident get`](#dd-incident-get) | read | Show a Datadog incident: state, severity, impact, timeline stamps and fields |
| [`dd host list`](#dd-host-list) | read | List hosts reporting to Datadog: up, last reported, apps and cluster |
| [`dd container list`](#dd-container-list) | read | List containers Datadog sees, with state, image and the k8s pod id |
| [`dd slo list`](#dd-slo-list) | read | List Datadog SLOs with their target and timeframe |
| [`dd slo get`](#dd-slo-get) | read | Show an SLO's measured SLI and error budget left over its window |
| [`dd dashboard list`](#dd-dashboard-list) | read | Find Datadog dashboards by words in the title, with their links |

### dd log list

```text
agent-cli dd log list — Search Datadog logs in a time window, newest first, as compact rows
  <query> str                   Datadog log search syntax, ANDed with the flags: '@http.status_code:502 timeout'
  --since time                  From when (default 15m before --until)
  --until time                  Until when (default now)
  --status emergency|alert|critical|error|warn|notice|info|debug|ok[]  Log status (repeatable)
  --service str                 Datadog service tag
  --env str                     env tag (logs, spans: default [datadog] env)
  --cluster str                 kube_cluster_name tag, e.g. prod (a filter; never defaulted)
  --namespace str               kube_namespace tag
  --pod str                     A pod as k8s prints it: CLUSTER/NAMESPACE/POD, NAMESPACE/POD or POD
  --deployment str              A deployment: CLUSTER/NAMESPACE/NAME, NAMESPACE/NAME or NAME
  --index str[]                 Log indexes to search (default: every index)
  --full                        Whole messages and every attribute
  --limit int                   At most 1000 (one page); count more with dd log-count list (default 50)
A time is 15m, 2h, 7d, 1w (ago), now-15m, 2026-09-29, or RFC 3339.
Returns: [{id,time,status,service,host,message,error{kind,message},trace_id,pod,attributes}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli dd log list --service api --status error --since 1h --fields time,message,pod
```

### dd log-count list

```text
agent-cli dd log-count list — Count Datadog logs grouped by facets such as service or status
  <query> str                   Datadog log search syntax, ANDed with the flags
  --by str[]                    Facets to group by: service, status, kube_namespace, pod_name, @http.status_code … (repeatable) (default service)
  --since time                  From when (default 1h before --until)
  --until time                  Until when (default now)
  --status emergency|alert|critical|error|warn|notice|info|debug|ok[]  Log status (repeatable)
  --service str                 Datadog service tag
  --env str                     env tag (logs, spans: default [datadog] env)
  --cluster str                 kube_cluster_name tag, e.g. prod (a filter; never defaulted)
  --namespace str               kube_namespace tag
  --pod str                     A pod as k8s prints it: CLUSTER/NAMESPACE/POD, NAMESPACE/POD or POD
  --deployment str              A deployment: CLUSTER/NAMESPACE/NAME, NAMESPACE/NAME or NAME
  --limit int                   Groups to return, largest first (default 50)
A time is 15m, 2h, 7d, 1w (ago), now-15m, 2026-09-29, or RFC 3339.
Returns: [{group,count}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli dd log-count list --status error --by service,kube_namespace --fields group,count
```

### dd metric list

```text
agent-cli dd metric list — Find Datadog metric names reporting recently, by part of the name
  <pattern> str     Part of the metric name: kubernetes.memory, latency
  --since time      Reporting since when (default 1h ago)
  --service str     Datadog service tag
  --env str         env tag (logs, spans: default [datadog] env)
  --cluster str     kube_cluster_name tag, e.g. prod (a filter; never defaulted)
  --namespace str   kube_namespace tag
  --pod str         A pod as k8s prints it: CLUSTER/NAMESPACE/POD, NAMESPACE/POD or POD
  --deployment str  A deployment: CLUSTER/NAMESPACE/NAME, NAMESPACE/NAME or NAME
  --limit int       (default 50)
A time is 15m, 2h, 7d, 1w (ago), now-15m, 2026-09-29, or RFC 3339.
Returns: [str]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli dd metric list kubernetes.memory --namespace web
```

### dd metric get

```text
agent-cli dd metric get — Query a Datadog metric: min, max, avg, last and a few points per series
 *<query> str          A metric query: 'avg:kubernetes.cpu.usage.total{*} by {pod_name}'; arithmetic and .rollup() work
  --since time         From when (default 1h before --until)
  --until time         Until when (default now)
  --service str        Datadog service tag
  --env str            env tag (logs, spans: default [datadog] env)
  --cluster str        kube_cluster_name tag, e.g. prod (a filter; never defaulted)
  --namespace str      kube_namespace tag
  --pod str            A pod as k8s prints it: CLUSTER/NAMESPACE/POD, NAMESPACE/POD or POD
  --deployment str     A deployment: CLUSTER/NAMESPACE/NAME, NAMESPACE/NAME or NAME
  --points int         Points per series after downsampling by bucket mean (0: stats only) (default 12)
  --limit int          Series to return, ordered by --sort (default 20)
  --sort max|avg|last  The stat that orders the series, largest first (default max)
A time is 15m, 2h, 7d, 1w (ago), now-15m, 2026-09-29, or RFC 3339.
Returns: [{series,group,pod,unit,min,max,max_at,avg,last,points[],gaps}]
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli dd metric get 'avg:kubernetes.cpu.usage.total{*} by {pod_name}' --namespace web --since 3h --fields group,max,last
```

### dd monitor list

```text
agent-cli dd monitor list — List Datadog monitors alerting or not, by state or tag, with when each triggered
  <query> str    Monitor search syntax: 'service:api', 'type:metric', 'title:"error rate"'
  --state str[]  Alert, Warn, "No Data", OK (repeatable; any case)
  --tag str[]    Monitor tags: team:web (repeatable, all must hold)
  --limit int    (default 50)
Returns: [{id,name,state,type,priority,triggered,tags[]}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli dd monitor list --state Alert,Warn --tag team:web --fields id,name,state,triggered
```

### dd monitor get

```text
agent-cli dd monitor get — Show a Datadog monitor: its query, state, the groups that triggered, downtimes
 *<id> str      The monitor: its id (4711) or its https://app.<site>/monitors/4711 URL
  --all-groups  Every group, not only those that are not OK
Returns: {id,name,state,type,priority,query,message,tags[],triggered,groups[{name,state,triggered,resolved,pod}],downtimes[{scope[],start,end}],url}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli dd monitor get 4711 --fields state,triggered,groups
```

### dd downtime list

```text
agent-cli dd downtime list — List Datadog downtimes: which monitors are muted, for what scope and until when
  --monitor int  Only downtimes that name this monitor id
  --all          Include ended and canceled downtimes
  --limit int    (default 50)
Returns: [{id,status,monitor,monitor_tags[],scope,start,end,message}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli dd downtime list --monitor 4711 --fields id,status,end
```

### dd downtime create

```text
agent-cli dd downtime create — Mute a Datadog monitor for a bounded time (--for, at most 7 days)
 *--for duration       How long, at most 7d: 30m, 2h, 1d
  --monitor int        The monitor to mute
  --monitor-tag str[]  Mute every monitor with these tags instead (repeatable)
  --scope str          The groups to mute: '*' (every group), 'pod_name:api-1', 'env:prod' (default *)
  --message str        Why, as Datadog shows it
A duration is 500ms, 30s, 15m, 2h, 7d, 1w.
Returns: {id,status,monitor,monitor_tags[],scope,start,end,message}
Destructive: needs --yes; --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli dd downtime create --monitor 4711 --for 30m --message 'deploy v1.4.2' --dry-run
```

### dd downtime cancel

```text
agent-cli dd downtime cancel — Unmute a Datadog monitor now, ending its downtime before it runs out
 *<id> str  The downtime id, from dd downtime list
Returns: {id,canceled}
Write: --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli dd downtime cancel 00000000-0000-4000-8000-00000000d001 --dry-run
```

### dd event list

```text
agent-cli dd event list — List Datadog events: deploys, monitor alerts, kubernetes events kept past 1h
  <query> str       Datadog event search syntax: 'source:kubernetes', 'tags:deployment'
  --since time      From when (default 1h before --until)
  --until time      Until when (default now)
  --service str     Datadog service tag
  --env str         env tag (logs, spans: default [datadog] env)
  --cluster str     kube_cluster_name tag, e.g. prod (a filter; never defaulted)
  --namespace str   kube_namespace tag
  --pod str         A pod as k8s prints it: CLUSTER/NAMESPACE/POD, NAMESPACE/POD or POD
  --deployment str  A deployment: CLUSTER/NAMESPACE/NAME, NAMESPACE/NAME or NAME
  --limit int       (default 50)
A time is 15m, 2h, 7d, 1w (ago), now-15m, 2026-09-29, or RFC 3339.
Returns: [{id,time,title,source,message,service,pod}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli dd event list 'source:kubernetes' --namespace web --since 2h --fields time,title,pod
```

### dd service list

```text
agent-cli dd service list — List the APM services that send traces to Datadog
  --env str    env tag (default [datadog] env, else every env)
  --limit int  (default 50)
Returns: [str]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli dd service list --env prod
```

### dd service get

```text
agent-cli dd service get — Show an APM service's health: request count, error rate, p50/p95/p99 latency
 *<service> str  The service, as dd service list prints it
  --env str      env tag (default [datadog] env, else every env)
  --since time   From when (default 1h before --until)
  --until time   Until when (default now)
A time is 15m, 2h, 7d, 1w (ago), now-15m, 2026-09-29, or RFC 3339.
Returns: {id,env,since,until,spans,errors,error_rate,p50_ms,p95_ms,p99_ms,resources[{resource,spans,errors,p95_ms}]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli dd service get api --env prod --fields error_rate,p95_ms,resources
```

### dd span list

```text
agent-cli dd span list — Search APM spans: failing or slow requests, or every span of one trace
  <query> str              Datadog trace search syntax, ANDed with the flags: 'resource_name:"GET /orders"'
  --since time             From when (default 15m before --until)
  --until time             Until when (default now)
  --status ok|error[]      Span status (repeatable)
  --min-duration duration  Only spans slower than this: 500ms, 2s
  --trace str              Every span of this trace id (from a log's or span's trace_id)
  --service str            Datadog service tag
  --env str                env tag (logs, spans: default [datadog] env)
  --cluster str            kube_cluster_name tag, e.g. prod (a filter; never defaulted)
  --namespace str          kube_namespace tag
  --pod str                A pod as k8s prints it: CLUSTER/NAMESPACE/POD, NAMESPACE/POD or POD
  --deployment str         A deployment: CLUSTER/NAMESPACE/NAME, NAMESPACE/NAME or NAME
  --limit int              At most 1000 (one page) (default 50)
A time is 15m, 2h, 7d, 1w (ago), now-15m, 2026-09-29, or RFC 3339.
A duration is 500ms, 30s, 15m, 2h, 7d, 1w.
Returns: [{trace_id,span_id,time,service,resource,operation,status,duration_ms,error{type,message},pod}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli dd span list --service api --status error --since 30m --fields time,resource,error,trace_id
```

### dd incident list

```text
agent-cli dd incident list — List Datadog incidents, active and stable by default
  <query> str       Incident search syntax, ANDed with the flags: 'customer_impacted:true'
  --state str[]     active, stable, resolved (repeatable; default active,stable)
  --severity str[]  SEV-1, SEV-2 … (repeatable; 1 means SEV-1)
  --limit int       (default 50)
Returns: [{id,title,state,severity,created,resolved}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli dd incident list --state active --fields id,title,severity
```

### dd incident get

```text
agent-cli dd incident get — Show a Datadog incident: state, severity, impact, timeline stamps and fields
 *<id> str  The incident: its number (982), its UUID, or its https://app.<site>/incidents/982 URL
Returns: {id,uuid,title,state,severity,customer_impacted,created,detected,resolved,fields,url}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli dd incident get 982 --fields title,state,fields
```

### dd host list

```text
agent-cli dd host list — List hosts reporting to Datadog: up, last reported, apps and cluster
  <query> str    Host name or tag to match: 'aks-nodepool1', 'env:prod'
  --cluster str  kube_cluster_name tag
  --limit int    (default 50)
Returns: [{id,up,last_reported,apps[],cluster,muted}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli dd host list --cluster prod --fields id,up,last_reported
```

### dd container list

```text
agent-cli dd container list — List containers Datadog sees, with state, image and the k8s pod id
  --service str     Datadog service tag
  --env str         env tag (logs, spans: default [datadog] env)
  --cluster str     kube_cluster_name tag, e.g. prod (a filter; never defaulted)
  --namespace str   kube_namespace tag
  --pod str         A pod as k8s prints it: CLUSTER/NAMESPACE/POD, NAMESPACE/POD or POD
  --deployment str  A deployment: CLUSTER/NAMESPACE/NAME, NAMESPACE/NAME or NAME
  --limit int       (default 50)
Returns: [{name,state,image,pod,host,started}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli dd container list --namespace web --fields name,state,image,pod
```

### dd slo list

```text
agent-cli dd slo list — List Datadog SLOs with their target and timeframe
  <query> str  Words in the SLO name
  --tag str[]  SLO tags: team:web (repeatable, all must hold)
  --limit int  (default 50)
Returns: [{id,name,type,target,timeframe,tags[]}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli dd slo list --tag team:web --fields id,name,target
```

### dd slo get

```text
agent-cli dd slo get — Show an SLO's measured SLI and error budget left over its window
 *<id> str      The SLO: its id, or its https://app.<site>/slo?slo_id=… URL
  --since time  From when (default: the SLO's own timeframe before --until)
  --until time  Until when (default now)
A time is 15m, 2h, 7d, 1w (ago), now-15m, 2026-09-29, or RFC 3339.
Returns: {id,name,type,target,timeframe,sli,budget_remaining,breaching,since,until,url}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli dd slo get 0c3fe5a1b2c34d5e8f9a0b1c2d3e4f50 --fields sli,target,budget_remaining
```

### dd dashboard list

```text
agent-cli dd dashboard list — Find Datadog dashboards by words in the title, with their links
  <words> str[]  Words the title must contain, any case
  --limit int    (default 50)
Returns: [{id,title,modified,url}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli dd dashboard list kubernetes --fields id,title,url
```
