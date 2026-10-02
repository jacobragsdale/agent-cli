# ado — Azure DevOps (43 commands)

| Command | Effect | Summary |
|---|---|---|
| [`ado workitem list`](#ado-workitem-list) | read | List work items matching filters (live WIQL) |
| [`ado workitem get`](#ado-workitem-get) | read | Show a work item: fields, description as Markdown, links, latest comments |
| [`ado workitem create`](#ado-workitem-create) | write | Create a work item (bug, task, story …), optionally under a parent |
| [`ado workitem update`](#ado-workitem-update) | write | Change a work item's state, assignee, title, iteration, tags or description |
| [`ado workitem comment`](#ado-workitem-comment) | write | Add a comment to a work item (Markdown, - for stdin, or --text-file) |
| [`ado workitem link`](#ado-workitem-link) | write | Link a work item to a branch, creating the branch when missing |
| [`ado workitem-type list`](#ado-workitem-type-list) | read | List the project's work item types (Bug, User Story, Task …) and their states |
| [`ado workitem-type get`](#ado-workitem-type-get) | read | Show a work item type's states, moves and fields (required, allowed values) |
| [`ado team list`](#ado-team-list) | read | List the project's teams (for [ado] team, which @current needs) |
| [`ado person list`](#ado-person-list) | read | List a team's members with the address --assignee and @mentions take |
| [`ado repo list`](#ado-repo-list) | read | List the project's Git repositories |
| [`ado repo get`](#ado-repo-get) | read | Show a repository: its URLs, default branch and branches |
| [`ado diff get`](#ado-diff-get) | read | Show what changed in a pull request or between two refs, as hunks per file |
| [`ado commit list`](#ado-commit-list) | read | List the commits on a repository, file or folder, with the pull request of each |
| [`ado file get`](#ado-file-get) | read | Show a file in a repository at a branch, tag or commit, around a line |
| [`ado file list`](#ado-file-list) | read | List the files and folders in a repository folder at a branch, tag or commit |
| [`ado code list`](#ado-code-list) | read | Search code in every repository: where a symbol is defined and who calls it |
| [`ado pr list`](#ado-pr-list) | read | List pull requests by repo, author, reviewer, vote (approved …), branch, status |
| [`ado pr get`](#ado-pr-get) | read | Show a pull request: reviewers and votes, work items, policies, threads |
| [`ado pr create`](#ado-pr-create) | write | Open a pull request linked to work items, or reuse the one already open |
| [`ado pr vote`](#ado-pr-vote) | write | Record your vote on a pull request: approve, suggest, reject or none |
| [`ado pr update`](#ado-pr-update) | write | Turn auto-complete on or off, mark draft or ready, or retitle a pull request |
| [`ado pr link`](#ado-pr-link) | write | Link a work item to a pull request |
| [`ado pr comment`](#ado-pr-comment) | write | Start a comment thread on a pull request (Markdown, - for stdin, or --text-file) |
| [`ado pr complete`](#ado-pr-complete) | destructive | Complete (merge) a pull request: squash, merge or rebase |
| [`ado pr abandon`](#ado-pr-abandon) | destructive | Abandon (close) a pull request, discarding its changes |
| [`ado thread list`](#ado-thread-list) | read | List a pull request's review threads with the code each comment is on |
| [`ado thread comment`](#ado-thread-comment) | write | Reply to a pull request review thread, and resolve it with --resolve |
| [`ado thread update`](#ado-thread-update) | write | Resolve, reopen or close pull request review threads |
| [`ado pipeline list`](#ado-pipeline-list) | read | List build pipelines, each with its last result |
| [`ado pipeline get`](#ado-pipeline-get) | read | Show a pipeline's definition: the YAML file it runs and its default branch |
| [`ado pipeline preview`](#ado-pipeline-preview) | read | Expand a pipeline's YAML, or an edit of it, without queuing anything |
| [`ado run list`](#ado-run-list) | read | List pipeline runs, newest first |
| [`ado run get`](#ado-run-get) | read | Show a run (build): status, commit, what failed, its pull request and work items |
| [`ado run logs`](#ado-run-logs) | read | Print the tail of a run's logs: failed tasks by default, or a job or task |
| [`ado run create`](#ado-run-create) | write | Start a pipeline run on a branch, with template parameters |
| [`ado run wait`](#ado-run-wait) | read | Wait for a run to finish: exit 0 if it succeeded, 1 if not, 124 if still going |
| [`ado run cancel`](#ado-run-cancel) | destructive | Cancel a run that is queued or in progress |
| [`ado run retry`](#ado-run-retry) | write | Retry the failed jobs of a finished run |
| [`ado test list`](#ado-test-list) | read | List a run's failing tests: message, stack and the repository line |
| [`ado approval list`](#ado-approval-list) | read | List pending pipeline approvals (deployment gates) |
| [`ado approval approve`](#ado-approval-approve) | destructive | Approve a pending pipeline approval, letting the stage (a deploy) run |
| [`ado approval reject`](#ado-approval-reject) | destructive | Reject a pending pipeline approval, stopping the stage |

### ado workitem list

```text
agent-cli ado workitem list — List work items matching filters (live WIQL)
  --assignee str          Name, email or @me
  --state str[]           Active, "In Progress" … (repeatable)
  --type str[]            Bug, "User Story", Task … (repeatable)
  --iteration str         Iteration path, or @current, @next or @previous for the team's sprint
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

### ado workitem-type list

```text
agent-cli ado workitem-type list — List the project's work item types (Bug, User Story, Task …) and their states
Returns: [{name,description,states[]}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli ado workitem-type list --fields name,states
```

### ado workitem-type get

```text
agent-cli ado workitem-type get — Show a work item type's states, moves and fields (required, allowed values)
 *<type> str  Bug, "User Story", Task … (any case), from workitem-type list
Returns: {name,description,states[{name,category}],transitions,fields[{name,ref,type,required,allowed_values[],default}]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli ado workitem-type get Bug --fields states,transitions
```

### ado team list

```text
agent-cli ado team list — List the project's teams (for [ado] team, which @current needs)
  --limit int  Most rows to return (default 50)
Returns: [{name,id,description}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli ado team list --fields name
```

### ado person list

```text
agent-cli ado person list — List a team's members with the address --assignee and @mentions take
  --team str   A team's name (default: every team in [ado] team)
  --text str   Words in the name or address
  --limit int  Most rows to return (default 50)
Returns: [{id,name,team}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli ado person list --text sam --fields id,name
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

### ado diff get

```text
agent-cli ado diff get — Show what changed in a pull request or between two refs, as hunks per file
 *<compare> str  A pull request (436, #436 or its URL), or two refs of a repository: REPO@BASE..HEAD
  --file str[]   Only files matching this glob or path (repeatable): *.cs, src/Orders/OrderClient.cs
  --names-only   Only which files changed and how, in one call
  --unified int  Unchanged lines shown around each change (default 3)
  --limit int    Most files to return (default 50)
Returns: [{at,path,change,from,added,removed,hunks[{at,diff}]}]
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli ado diff get 436 --file '*.cs' --fields at,path,hunks
```

### ado commit list

```text
agent-cli ado commit list — List the commits on a repository, file or folder, with the pull request of each
 *<repo> str    The repository, or a file or folder in it: REPO[@REF][:PATH], as file get takes it (a line is ignored)
  --path str    The file or folder in the repository, when REPO names none
  --ref str     The branch, tag or commit to read history back from (default: the default branch)
  --since time  Committed after this
  --until time  Committed before this
  --author str  Who wrote it: name, email or @me
  --limit int   Most rows to return (default 50)
A time is 15m, 2h, 7d, 1w (ago), now-15m, 2026-09-29, or RFC 3339.
Returns: [{commit,author,date,message,pr{id,title},diff}]
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli ado commit list api:src/Orders/OrderClient.cs --fields commit,date,message,pr
```

### ado file get

```text
agent-cli ado file get — Show a file in a repository at a branch, tag or commit, around a line
 *<file> str[]  The file: REPO[@REF]:PATH[:LINE[-LINE]] as code list, thread list and diff get print it, its web URL, or a path with --repo. Several print an array, in order
  --repo str    The repository, when FILE is a bare path
  --ref str     The branch, tag or commit (default: the repository's default branch)
  --line str    The line, or lines A-B, to show (one line shows 20 either side)
Returns: {id,repo,path,ref,commit,lines,total,text}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli ado file get api@main:src/Program.cs:42
```

### ado file list

```text
agent-cli ado file list — List the files and folders in a repository folder at a branch, tag or commit
 *<folder> str  The folder: REPO[@REF][:PATH] (the root without a path), as file list prints it, its web URL, or a path with --repo
  --repo str    The repository, when FOLDER is a bare path
  --ref str     The branch, tag or commit (default: the repository's default branch)
  --recursive   Everything under the folder, not only what is in it
  --limit int   Most rows to return (default 50)
Returns: [{id,path,kind}]
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli ado file list worker:src/Jobs --fields id,kind
```

### ado code list

```text
agent-cli ado code list — Search code in every repository: where a symbol is defined and who calls it
 *<text> str     What to find; Code Search syntax passes through (ext:cs, class:Name, "a phrase")
  --repo str[]   Only this repository (repeatable)
  --project str  Only this project (default: every project, or the code project with --repo)
  --path str     Only under this folder (src/Orders)
  --branch str   Only this branch (default: each repository's default branch)
  --limit int    Most rows to return (default 50)
Returns: [{id,repo,path,line,text,matches}]
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli ado code list IOrderClient --fields id,text
```

### ado pr list

```text
agent-cli ado pr list — List pull requests by repo, author, reviewer, vote (approved …), branch, status
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
Returns: {id,repo,title,author,status,is_draft,source,target,merge_status,auto_complete,created,reviewers[{name,vote,required}],url,description,work_items[],policies[{name,status,run_id}],open_threads,threads[{id,status,author,at,file,line,date,text,replies}],last_merge_source_commit}
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
  --at str          Open it on a file's lines: REPO[@REF]:PATH[:LINE[-LINE]], as diff get prints a hunk's at
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

### ado thread list

```text
agent-cli ado thread list — List a pull request's review threads with the code each comment is on
 *<pr> str                      The pull request's id: 436, #436 or its web URL
  --status all|active|fixed|wontFix|closed|byDesign|pending  Only threads in this state, or all (default: active and pending)
  --author str                  Only threads this person started: a name, a sign-in address or @me
  --file str                    Only threads on files matching this glob (*.cs, src/Orders/*)
  --around int                  Lines of code shown either side of each comment (default 3)
  --limit int                   Most rows to return (default 50)
Returns: [{id,status,at,file,line,author,date,text,replies[{author,date,text}],code}]
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli ado thread list 436 --fields id,at,author,text,code
```

### ado thread comment

```text
agent-cli ado thread comment — Reply to a pull request review thread, and resolve it with --resolve
 *<id> str          The thread: PR/THREAD (436/7) as thread list and pr get print it, or its web URL
  <text> str        Markdown, or - to read stdin (posted as a code block, 64 KiB max)
  --text-file path  The reply from a Markdown file (64 KiB max)
  --pr str          The pull request, when the id is the thread's number alone
  --resolve         Also resolve the thread (status fixed)
Returns: {id,comment_id,status}
Write: --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli ado thread comment 436/7 'Capped at 30 s in 9f1c2e4' --resolve
```

### ado thread update

```text
agent-cli ado thread update — Resolve, reopen or close pull request review threads
 *<id> str[]                    The thread: PR/THREAD (436/7) as thread list and pr get print it, or its web URL. Several print an array, in order
 *--status active|fixed|wontFix|closed|byDesign|pending  fixed resolves it, active reopens it
  --pr str                      The pull request, when the id is the thread's number alone
Returns: {id,status}
Write: --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli ado thread update 436/7 --status fixed
```

### ado pipeline list

```text
agent-cli ado pipeline list — List build pipelines, each with its last result
  <pattern> str  Only names containing this
  --repo str     Only pipelines that build this repository
  --limit int    Most rows to return (default 50)
Returns: [{id,name,folder,queue_status,last_run{id,status,result,branch,finished},url}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli ado pipeline list --repo web --fields id,name,last_run.result
```

### ado pipeline get

```text
agent-cli ado pipeline get — Show a pipeline's definition: the YAML file it runs and its default branch
 *<pipeline> str  The pipeline's id or name
Returns: {id,name,folder,repo,default_branch,yaml,queue_status,url}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli ado pipeline get api-ci --fields repo,yaml
```

### ado pipeline preview

```text
agent-cli ado pipeline preview — Expand a pipeline's YAML, or an edit of it, without queuing anything
 *<pipeline> str    The pipeline's id or name
  <yaml> str        YAML to expand instead of the pipeline's own file, or - to read stdin
  --yaml-file path  The YAML to expand, from a file
  --branch str      The branch its file and templates come from (default: the pipeline's default branch)
Returns: {pipeline,branch,yaml}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli ado pipeline preview api-ci --yaml-file azure-pipelines.yml
```

### ado run list

```text
agent-cli ado run list — List pipeline runs, newest first
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
agent-cli ado run get — Show a run (build): status, commit, what failed, its pull request and work items
 *<id> str  The run's id: 8812, #8812 or its web URL
Returns: {id,pipeline,pipeline_id,build_number,status,result,branch,commit,requested_by,reason,queued,started,finished,url,pr{id,title,status},workitems[{id,type,title,state}],running[],failed[{type,name,log_id,errors[{message,at}]}]}
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

### ado test list

```text
agent-cli ado test list — List a run's failing tests: message, stack and the repository line
 *<run> str                    The run (build) whose tests to show: 8809, #8809 or its web URL
  --outcome failed|passed|all  Which results (default failed)
  --limit int                  Most rows to return (default 50)
Returns: [{name,outcome,duration_ms,error,at,failing_since,stack}]
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli ado test list 8809 --fields name,error,at,failing_since
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
