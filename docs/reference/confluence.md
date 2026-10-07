# confluence — Confluence Cloud (14 commands)

| Command | Effect | Summary |
|---|---|---|
| [`confluence space list`](#confluence-space-list) | read | List Confluence spaces, or find one by key or name |
| [`confluence page list`](#confluence-page-list) | read | Search Confluence pages (runbooks, designs) by words, space, label, title, date |
| [`confluence page get`](#confluence-page-get) | read | Show a page as Markdown with its version, author, labels and outline |
| [`confluence page create`](#confluence-page-create) | write | Publish a new page from Markdown (or storage) in a space, under a parent |
| [`confluence page update`](#confluence-page-update) | write | Edit a page: replace or append to its body or a section, retitle, move, relabel |
| [`confluence page comment`](#confluence-page-comment) | write | Comment on a page: at its foot, inline on some text (--on), or as a reply |
| [`confluence page delete`](#confluence-page-delete) | destructive | Move a page to its space's trash, from which it can be restored |
| [`confluence tree get`](#confluence-tree-get) | read | Show the pages below a page, or at the top of a space, as flat rows |
| [`confluence comment list`](#confluence-comment-list) | read | List a page's footer and inline comments, newest first, with what each is on |
| [`confluence comment update`](#confluence-comment-update) | write | Resolve an inline comment on a page, or reopen it |
| [`confluence attachment list`](#confluence-attachment-list) | read | List the files attached to a page, newest first |
| [`confluence attachment get`](#confluence-attachment-get) | read | Show a page attachment's text, or save the file with --output |
| [`confluence attachment create`](#confluence-attachment-create) | write | Attach a file to a page, or add a new version of one with its name |
| [`confluence version list`](#confluence-version-list) | read | List a page's versions, newest first: who changed it, when and why |

### confluence space list

```text
agent-cli confluence space list — List Confluence spaces, or find one by key or name
  <text> str              Only spaces whose key or name holds this, any case
  --type global|personal  global (team spaces) or personal (default global)
  --limit int             Most rows to return (default 50)
Returns: [{id,name,type,homepage,url}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli confluence space list eng --fields id,name,homepage
```

### confluence page list

```text
agent-cli confluence page list — Search Confluence pages (runbooks, designs) by words, space, label, title, date
  <text> str              Words to search for, ranked as the web search ranks them (the index trails edits by about a minute)
  --space str[]           Only in this space (its key or URL); repeatable
  --label str[]           Only pages carrying this label; repeatable, each one required
  --title str             Words in the title
  --parent int            Only the direct children of this page id
  --author str            Created by this person: part of their name, or @me
  --mentioned             Only pages that mention you
  --type page|blogpost    page or blogpost (default: both)
  --since time            Changed (or created, with --date created) since: 15m, 2h, 7d, a date
  --until time            Changed (or created) before this time
  --date changed|created  What --since and --until look at (default changed)
  --cql str               More CQL, ANDed with the rest (no order by)
  --limit int             Most rows to return (default 50)
A time is 15m, 2h, 7d, 1w (ago), now-15m, 2026-09-29, or RFC 3339.
Returns: [{id,type,title,space,updated,updated_by,excerpt}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli confluence page list 'etl_nightly runbook' --space ENG --fields id,title,updated
```

### confluence page get

```text
agent-cli confluence page get — Show a page as Markdown with its version, author, labels and outline
 *<page> str[]   The page: its id, ID@VERSION, KEY:Title or its URL. Several print an array, in order
  --version str  An older version (version list shows them); ID@VERSION says the same
  --section str  Only this section: its heading's text, any case, through the next heading of its level
  --line str     Only these lines of the body: A-B, or one line with 20 either side
  --storage      The body as storage XHTML, not Markdown: what page update --storage takes back
Returns: {id,type,title,space,status,version,parent,author,created,updated,updated_by,labels[],url,lines,total,outline[{line,level,heading}],lossy[],body,saved}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli confluence page get 1101 --section 'An order without customer_id'
```

### confluence page create

```text
agent-cli confluence page create — Publish a new page from Markdown (or storage) in a space, under a parent
 *--title str       The new page's title (unique in its space)
  --space str       The space, by key or URL (default: the parent's)
  --parent int      The parent page's id (default: the space's homepage)
  --body str        The body: Markdown, or - to read stdin
  --body-file path  The body from a Markdown file
  --storage         The body is storage XHTML, sent as it is
  --labels str      Labels to add, comma-separated
Returns: {id,title,space,version,url}
Write: --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli confluence page create --title 'Postmortem: etl_nightly' --parent 1100 --body-file postmortem.md
```

### confluence page update

```text
agent-cli confluence page update — Edit a page: replace or append to its body or a section, retitle, move, relabel
 *<page> str          The page: its id, KEY:Title or its URL
  --title str         A new title (alone, it never touches the body)
  --body str          The new body, Markdown or - to read stdin: the whole page (needs --if-version), or --section's from its heading
  --body-file path    The new body from a file
  --append str        Markdown to add at the end of the page, or of --section; - reads stdin
  --append-file path  What to append, from a file
  --section str       The section --body replaces or --append extends: its heading's text, any case
  --parent int        Move the page under this page id, in any space
  --labels str        The labels it should have, comma-separated (replaces them; "" removes all)
  --message str       The version comment
  --storage           --body or --append is storage XHTML, sent as it is
  --if-version int    Refuse unless the page is still at this version (page get prints it)
Returns: {id,title,space,version,url}
Write: --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli confluence page update 1101 --append-file rollback.md --message 'Add the rollback steps'
```

### confluence page comment

```text
agent-cli confluence page comment — Comment on a page: at its foot, inline on some text (--on), or as a reply
 *<page> str        The page: its id, KEY:Title or its URL
  <text> str        Markdown, or - to read stdin (64 KiB max)
  --text-file path  The comment from a Markdown file (64 KiB max)
  --reply-to str    Reply to this comment (its id or URL), footer or inline
  --on str          Comment inline on this text of the page, as it reads
  --match int       Which occurrence of --on's text, from 1, when the page has several
Returns: {id,page,kind,url}
Write: --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli confluence page comment 1201 'Rolled out to prod at 21:34' --on 'v1.4.2'
```

### confluence page delete

```text
agent-cli confluence page delete — Move a page to its space's trash, from which it can be restored
 *<page> str  The page: its id, KEY:Title or its URL
Returns: {id,title,status}
Destructive: needs --yes; --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli confluence page delete 1102 --yes
```

### confluence tree get

```text
agent-cli confluence tree get — Show the pages below a page, or at the top of a space, as flat rows
 *<target> str  A page (id, KEY:Title or URL) for what is below it, or a space (its key or URL) for its top
  --depth int   How many levels down, 1 to 10 (default 2)
Returns: {id,title,space,nodes[{id,title,type,parent,depth}]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli confluence tree get 1100 --depth 1 --fields nodes
```

### confluence comment list

```text
agent-cli confluence comment list — List a page's footer and inline comments, newest first, with what each is on
 *<page> str            The page: its id, KEY:Title or its URL
  --kind footer|inline  footer (below the page) or inline (on its text)
  --open                Only inline comments not yet resolved
  --since time          Only comments made since: 15m, 2h, 7d, a date
  --limit int           Most rows to return (default 50)
A time is 15m, 2h, 7d, 1w (ago), now-15m, 2026-09-29, or RFC 3339.
Returns: [{id,kind,author,date,parent,selection,resolution,body}]
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli confluence comment list 1201 --open --fields id,author,selection,body
```

### confluence comment update

```text
agent-cli confluence comment update — Resolve an inline comment on a page, or reopen it
 *<comment> str           The inline comment: its id, or a URL with focusedCommentId
 *--status resolved|open  resolved, or open to reopen it
Returns: {id,page,resolution}
Write: --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli confluence comment update 5002 --status resolved
```

### confluence attachment list

```text
agent-cli confluence attachment list — List the files attached to a page, newest first
 *<page> str   The page: its id, KEY:Title or its URL
  --name str   Only files whose name holds this, any case
  --limit int  Most rows to return (default 50)
Returns: [{id,page,name,media_type,size,version,created,author,comment}]
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli confluence attachment list 1201 --fields id,name,size
```

### confluence attachment get

```text
agent-cli confluence attachment get — Show a page attachment's text, or save the file with --output
 *<attachment> str  The attachment: its id (att7001), as attachment list prints it, or its URL. Text up to 1 MiB prints; anything else needs --output FILE
Returns: {id,page,name,media_type,size,text,saved}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli confluence attachment get att7001
```

### confluence attachment create

```text
agent-cli confluence attachment create — Attach a file to a page, or add a new version of one with its name
 *<page> str     The page: its id, KEY:Title or its URL
 *--file str     The file to attach; one of the same name on the page gets a new version
  --comment str  A note shown beside the attachment
Returns: {id,page,name,size,version}
Write: --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli confluence attachment create 1201 --file changes-v1.4.3.txt --comment 'Changelog'
```

### confluence version list

```text
agent-cli confluence version list — List a page's versions, newest first: who changed it, when and why
 *<page> str   The page: its id, KEY:Title or its URL
  --limit int  Most rows to return (default 50)
Returns: [{id,version,author,date,message,minor}]
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli confluence version list 1201 --limit 5 --fields id,author,date,message
```
