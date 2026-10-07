# Plan: the confluence domain

Design of 2026-10-06, from the API specs (Cloud v2 and v1, Data Center 9.0 to
10.2.19), Atlassian's changelog and docs, anonymous probes of public sites,
and the agent tools already out there (Atlassian's Rovo MCP server and `twg`
CLI, sooperset/mcp-atlassian, a dozen agent CLIs). One instance. Built for
**Cloud**; Data Center is an adapter (phase 3), built only if the work
instance runs it. Reads first; writes once trials show the reads hold up.

## Does it earn a domain

The recurring tasks, in the order prior art converged on them:

1. Find the runbook, design or release page for something (search).
2. Read a page, or one section of it, as Markdown, with who changed it when.
3. Walk the page tree of a space or a section.
4. Read a page's discussion: footer and inline comments, the open ones.
5. Fetch an attachment (a log, a CSV, a diagram) to a file.
6. See what changed: versions, an older version's body.
7. Phase 2: publish a page from Markdown, add or replace a section, comment,
   resolve an inline comment.

What an agent loses without it (trial below):

- **curl** has to cope with two API versions that page differently, a CQL
  dialect with silent traps (§ page list), and attachment downloads through
  a pre-signed redirect.
- **Body size:** storage XHTML bodies run several times the size of their
  Markdown. A 300 KB page breaks the roughly 25,000-token cap Claude Code
  puts on a tool result.
- **Writes:**
  - other tools guess `version + 1`, overwriting what a person saved in the
    meantime;
  - Markdown round trips strip macros;
  - a title-only update that resent a stale body has wiped pages.
- **Atlassian's own tools fall short:**
  - `twg` (2026-05) is Cloud-only and signs in with OAuth 2.1, whose 8-hour
    tokens need refreshing outside the sandbox. Some of its reads cost Rovo
    credits, and it has 115 Confluence commands.
  - `acli` has no Confluence commands.

agent-cli adds:
- API-token sign-in, and Data Center if needed;
- bounded bodies;
- the shared conventions and read-only mode;
- the same shell as the ado, airflow and aisearch commands that runbooks
  point at.

## Config and sign-in

```toml
[confluence]
url = "https://contoso.atlassian.net/wiki"  # Cloud site; Data Center: its base with any context path
email = "jane@contoso.com"                  # Cloud: the account the API token belongs to
token_env = "CONFLUENCE_TOKEN"              # or token_cmd: a Cloud API token, or a Data Center PAT
# cloud_id = "11111111-2222-3333-4444-555555555555"  # scoped API tokens and service accounts only
```

**Credential:** `Credential::from_keys("token", …)`, resolved when the first
request goes out.

**Base URL and host, by kind of credential:**

| Credential | Base and auth | Host the credential may go to |
|---|---|---|
| Classic Cloud API token | the site, `Basic email:token` | exactly the url's host |
| Scoped token or service account (`cloud_id` set) | `https://api.atlassian.com/ex/confluence/{cloud_id}`; Basic, or Bearer when there is no `email` | exactly `api.atlassian.com` |
| Data Center (any host not under `atlassian.net`) | the url, `Bearer` PAT | exactly the url's host |

A suffix like `atlassian.com` would also cover `api.media.atlassian.com`,
where downloads redirect. That is why the gateway host must match exactly.

**Doctor:** call `GET /wiki/rest/api/user/current` and require `type: "known"`.
- A bad token, on a site that allows anonymous access, answers **200 as the
  anonymous user**, on Cloud and Data Center alike. So `anonymous` is a
  failed check: "the token was ignored: expired, or the wrong email".
- Every Cloud API token now expires within a year. A 401 hint names
  id.atlassian.com's API token page.

**Status line:** the site's name (`confluence contoso`).

## How the API is used

- **Cloud v2 (`/wiki/api/v2`) for everything it has.** Use v1 (`/wiki/rest/api`)
  only for: CQL search, the current user, user search, label add and remove,
  attachment upload and download, and move.
  - The v1 content and space endpoints are 17 months past their announced
    removal. They still answer classic tokens, and answer 410 through the
    gateway. Never call them.
- **One paging helper with two link rules:**
  - v2 `_links.next` is host-relative and already starts with `/wiki`.
  - v1 search `next` is relative to `/wiki`, and it drops `excerpt`, so add
    that back.
- **Common to both kinds of link:**
  - stop only when `next` is absent (pages come back short while more
    exist);
  - treat `totalSize` as an estimate;
  - `limit` is at most 250;
  - resolve against the base called, never `_links.base`, which names the
    site even through the gateway.
- **v2 answers 404 for "missing" and "not permitted" alike**, so the hint
  says "or you may not see it".
- **Errors.** Core's `failure_message` needs two additions:
  - v2's `{"errors":[{status, code, title, detail}]}`: read `title`, plus
    `detail`;
  - v1's `message`: strip its leading `com.atlassian….SomeException: `.
- **Request limits:** URLs at most 8 KB (the CDN answers 413), and never a GET
  with a body (403).
- **Throttles:** 429 with `Retry-After` in seconds, which core already
  honours.
  - `X-RateLimit-Reset` is an ISO 8601 time here, but core's `throttle_wait`
    parses only numbers. Teach it the time form; it is only a fallback.
  - The rate limits for API tokens are unpublished.

## Ids

| Thing | `id` | Taken by | Also accepted |
|---|---|---|---|
| page, blog post, live doc | `"1101"` (a decimal string) | `page get`, `tree get`, `comment list`, `attachment list`, `version list`, the writes | its web URLs (see the list below); `KEY:Title` and `/wiki/display/KEY/Title` (looked up by exact title, any case) |
| an older version | `"1101@3"` | `page get` | `page get 1101 --version 3` |
| space | the key, `ENG` | `page list --space`, `tree get`, `page create --space` | `/wiki/spaces/KEY…` |
| comment | `"5002"` | `page comment --reply-to`, `comment update` | `…?focusedCommentId=5002` |
| attachment | `"att7001"` | `attachment get` | `viewpageattachments.action?pageId=P&preview=/P/7001/name`, and `/download/attachments/P/name` (looked up by file name) |

A page's web URLs, all accepted:
- `/wiki/spaces/KEY/pages/ID[/slug]` and `…/pages/edit-v2/ID`;
- `/wiki/spaces/KEY/blog/Y/M/D/ID/…`;
- `viewpage.action?pageId=ID` and `resumedraft.action?draftId=ID`;
- `/wiki/x/CODE`, decoded locally. The code is the id as 8 little-endian
  bytes in base64, with the `=` and trailing `A`s stripped, `/` written as
  `-` and `+` as `_`. For example 123456789 ↔ `Fc1bBw`.

Other parsing rules:
- **Never fetched:** a URL is parsed, never fetched. A URL on another host
  is exit 2. Share links (`/l/cp/…`) can't be decoded, so they are exit 2
  asking for the page's own URL.
- **Space keys:** v2 calls take space ids, so resolve keys with
  `GET /spaces?keys=`. Keys can be renamed, so never keep an id.
- **Names, not account ids:** rows show people by display name, with one
  `POST /users-bulk` per command covering every account id it prints.

## Phase 1: reads

### `confluence space list [TEXT]`

`GET /spaces?type=global&status=current&sort=key&limit=250`, paged.
- `TEXT` keeps spaces whose key or name contains it, any case.
- Flags: `--type global|personal`, `--limit`.
- Returns `[{id, name, type, homepage, url}]`, where `id` is the key.

### `confluence page list [TEXT]`

Search pages and blog posts:
`GET /wiki/rest/api/search?cql=…&expand=content.space,content.version&limit=…`.

**Building the CQL**, all in one function:
- `TEXT` becomes `siteSearch ~ "TEXT" and text ~ "TEXT"`, placed first.
  - `siteSearch` ranks like the web UI, but it is silently dropped when it
    is not the first clause; the `text` clause is the floor.
  - A blank `siteSearch` is a 500 on the server, so it is refused locally.
- **Quoting:** every value is quoted and escaped. `title="a" or type=page`
  is an injection otherwise.
- **Type:** pinned to `type in (page, blogpost)`, because an untyped query
  mixes in attachments and comments.
- **Order:** relevance when there is text, otherwise
  `order by lastmodified desc`.

**Flags:**
- `--space KEY` (repeatable).
- `--label NAME` (repeatable; the page carries each of them).
- `--title TEXT` (`title ~`).
- `--parent ID` (its direct children).
- `--author NAME|@me` (creator; `@me` is `currentUser()`).
- `--mentioned` (`mention = currentUser()`).
- `--type page|blogpost`.
- `--since` and `--until`, with `--date changed|created` (default changed).
  They become `lastmodified >= now("-Nm")`, in whole minutes. CQL reads
  absolute dates in the user's profile time zone and refuses RFC 3339.
- `--cql CQL`: raw CQL, ANDed with the rest. It may not hold `order by`.
- `--limit` (default 50).

**Returns** `[{id, type, title, space, updated, updated_by, excerpt}]`.
- The title comes from `content.title`; the row's own `title` is
  HTML-escaped and carries highlight markers.
- The excerpt has its `@@@hl@@@` markers stripped and is cut at 150
  characters.
- The note reads `[50 of ~590; --limit N]`: `totalSize` is an estimate,
  hence the `~`.

The help says the index lags writes by about a minute, so a page made
moments ago may not be found yet.

### `confluence page get PAGE...`

**Calls:** `GET /pages/{id}?body-format=storage&include-labels=true`.
- On 404 it tries `GET /blogposts/{id}`.
- `--version N`, or the id `ID@N`, adds `version=N` (the answer's status is
  `historical`).
- Several pages print an array, in order, as `ado file get` does. Prior art:
  hydrate the few ids that matter in one call, never loop.

**The body is Markdown**, converted locally from storage (see Markdown below).
`--storage` prints the storage XHTML instead.

**It is bounded:** a body that would take the row past the 12 KB guard
prints up to a line boundary, and adds:
- `outline`: `[{line, level, heading}]`, h1 to h3 once it is long;
- the note `[lines 1-142 of 610; --section NAME, --line A-B, or --output FILE for all of it]`.

Choosing the part:
- `--section NAME`: the heading's text, any case. An ambiguous one is exit
  2, naming the matches.
- `--line A-B`: as `ado file get` takes it.
- `--output FILE`: saves the whole body (Markdown, or storage with
  `--storage`), and the row prints without it.

**`lossy`** names what the Markdown view simplified, and is printed only when
non-empty, for example `["layout", "jira ×2", "inline comment marks ×3", "macro drawio"]`.
The update gate (phase 2) refuses to overwrite those with Markdown.

**Returns** `{id, type, title, space, status, version, parent, author, created, updated, updated_by, labels[], url, lines, outline[{line,level,heading}], lossy[], body, saved}`.
Rows carry parent, version, updated and author because agents elsewhere
couldn't tell a stale page or find its parent without them.

### `confluence tree get PAGE|SPACE`

The hierarchy below a page, or a space's top level.

**Calls:**
- A page: `GET /pages/{id}/descendants?depth=N&limit=250`, cursor-paged. It
  comes in tree order and mixes pages, folders, whiteboards, databases and
  embeds.
- A space: its homepage's tree, plus `GET /spaces/{id}/pages?depth=root`
  for the root pages outside that tree.

**Flags:** `--depth N` (1 to 10, default 2).

**Returns** `{id, title, space, nodes[{id, title, type, parent, depth}]}`:
flat rows, which prior art found agents use best.

**Ceiling:** 500 nodes, then the note `[500 of 500+; --depth 1, or a PAGE further down]`.
Marked `ponytail:`.

### `confluence comment list PAGE`

Footer and inline comments, newest first, in one read for both flavors:
`GET /wiki/rest/api/content/search?cql=container=ID and type=comment order by created desc&expand=body.storage,version,history,ancestors,extensions.inlineProperties,extensions.resolution`.
This is CQL's non-deprecated endpoint, and it returns at most 50 rows a page
when it carries bodies.

**Flags:** `--kind footer|inline`, `--open` (inline comments not resolved),
`--since`, `--limit`.

**Returns** `[{id, kind, author, date, parent, selection, resolution, body}]`:
- `parent` is the comment it replies to;
- `selection` is the text an inline comment highlights;
- `resolution` is open, resolved or dangling;
- the body is Markdown, cut at 1,000 characters.

**Verify on the sandbox** that these expands carry the resolution and the
selection. The fallback is v2: `/pages/{id}/footer-comments` and
`/inline-comments`, plus `…/children` for each root comment.

### `confluence attachment list PAGE`

`GET /pages/{id}/attachments`, paged.
- Flags: `--name TEXT` (part of the file name), `--limit`.
- Returns `[{id, page, name, media_type, size, version, created, author, comment}]`.

### `confluence attachment get ATTACHMENT`

Works as `ado attachment get` does: text up to 1 MiB prints, and anything
else needs `--output FILE`.

**Calls**, in order:
1. `GET /attachments/{id}`: its page, name, type and size.
2. `GET /wiki/rest/api/content/{pageId}/child/attachment/{id}/download`,
   with the credential and `keep_redirect()`.
3. Cloud answers that with a 30x to
   `https://api.media.atlassian.com/file/{fileId}/binary?token=<JWT>&…`.
   Fetch that location **without** the credential, and only over https.
   Data Center answers step 2 with the file itself.

**Rules:**
- **The location is secret.** It is a bearer URL that stays valid for about
  23 hours. Never print it, and rewrite a failure of that hop so it names
  the attachment, not the URL; core's failures print the URL.
- **Never use the legacy `/download/attachments/…` form of `downloadLink` on
  Cloud.** It has refused API tokens since 2026-04-14.

**Returns** `{id, page, name, media_type, size, text, saved}`.

**Ceiling:** core keeps at most 32 MiB of an answer, as for ado.

### `confluence version list PAGE`

`GET /pages/{id}/versions`, newest first.
- Returns `[{id, version, author, date, message, minor}]`.
- `id` is `"1101@7"`, which `page get` takes.

## Phase 2: writes

**Long text.** Every body and comment reads stdin for `-` and has a
`--…-file` sibling (the long-text rule). Prior art: bodies passed through
the model as arguments time out or get truncated.

### `confluence page create`

Write. Its args: `--title T` and `--space KEY` (or `--parent ID`, whose space
it takes), plus `--body TEXT|-`, `--body-file F`, `--storage` and `--labels a,b`.

- **Calls:** `POST /pages {spaceId, parentId, title, status: "current", body: {representation: "storage", value}}`.
  It answers 200, not 201. Labels go on with v1
  `POST /wiki/rest/api/content/{id}/label`.
- **A duplicate title in the space is a 400**, and archived pages keep their
  titles. It becomes exit 5 with the hint
  `agent-cli confluence page get KEY:TITLE`.
- **Returns** `{id, title, space, version, url}`.

### `confluence page update PAGE`

Write. It takes one kind of change per call:

| Flags | What it does |
|---|---|
| `--title T` alone | `PUT /pages/{id}/title`. It never sends the body: a title-only update that resent a stale body wiped pages in another tool. |
| `--body TEXT\|-` or `--body-file F`, optionally with `--section NAME` | Replaces the whole body, or one section. A section runs from its heading to the next heading of the same or higher level; only headings at the root or directly inside a layout cell count. Everything else is spliced back byte for byte, never re-serialized. |
| `--append TEXT\|-` or `--append-file F`, optionally with `--section NAME` | Adds at the end of the page, or of the section. |
| `--parent ID` | Moves the page: v1 `PUT /wiki/rest/api/content/{id}/move/append/{target}`. It sends no body, and works across spaces. |
| `--labels a,b` | Replaces the labels, with v1 add and remove. No new version. |
| `--message TEXT` | The version comment. |
| `--storage` | The body or appended text is storage XHTML, sent as it is. |

**Versions:**
- **The write:** read the page first, then `PUT /pages/{id}` with
  `version.number = current + 1`.
- **`--if-version N` refuses unless the page is still at version N**, as ado's
  `--if-rev` does.
- **A whole-body replace requires it**, exit 2 naming
  `agent-cli confluence page get ID --fields version`. A blind "current + 1"
  overwrites whatever a person saved in between, which is the most common
  bug in prior art.
- **A 409 whose version differs** is exit 5: "changed since version N (now
  M)", with the hint `agent-cli confluence page get ID`.
- **A 409 in a space that requires publishing approval** (announced
  2026-09-28) is exit 5 in the service's own words.

**The loss gate:** replacing a part whose storage holds what the Markdown view
can't carry is exit 2, listing it (the `lossy` of that part). That covers
layouts, macros outside the table below, and inline comment marks, which
would leave comments dangling. The way out is one of:
- a smaller `--section`;
- `--append`;
- editing storage: `page get ID --storage --output page.xml`, then
  `page update ID --storage --body-file page.xml --if-version N`.

**Returns** `{id, title, space, version, url}`.

### `confluence page comment PAGE [TEXT|-]`

Write. Its args: `--text-file F` (Markdown, 64 KiB at most),
`--reply-to COMMENT`, and `--on TEXT` with `--match N`.

It makes one of three things:
- **A footer comment:** `POST /footer-comments {pageId, body}`, which answers
  201.
- **A reply:** the same call with `parentCommentId`.
- **With `--on`, an inline comment on that text:** `POST /inline-comments`
  with `inlineCommentProperties {textSelection, textSelectionMatchCount, textSelectionMatchIndex}`.
  The CLI counts the text's occurrences in the page first:
  - none is exit 4;
  - several need `--match N`, and without it the call is exit 2, naming
    the count.

**Returns** `{id, page, kind, url}`.

### `confluence comment update COMMENT --status resolved|open`

Write. `PUT /inline-comments/{id} {version: {number: n + 1}, resolved}`.
Only inline comments can be resolved.

### `confluence attachment create PAGE --file F [--comment TEXT]`

Write. v1 `PUT /wiki/rest/api/content/{id}/child/attachment`, which creates
the file or adds a new version of the same name.
- It is multipart, with the header `X-Atlassian-Token: no-check` and
  `minorEdit=true`. The client builds the multipart body as bytes.
- **`--file` is `str`** in the registry, as ado's is.
- **Returns** `{id, page, name, size, version}`.

### `confluence page delete PAGE`

Destructive. `DELETE /pages/{id}` moves the page to the trash, from which
the space restores it. It never purges.

## Markdown

**Reading** converts storage to Markdown locally:
- **Not ADF:** it is Cloud-only, while storage works on Data Center too.
- **Not the server's undocumented `body-format=markdown`:** it drops images,
  panel types and expand titles, garbles status lozenges, and may go away
  without notice.

**Writing** converts Markdown to storage locally. v2 writes take storage, ADF
or wiki markup, but never Markdown.

**Start from ado's converter**, `crates/ado/src/markdown.rs`:
- lift its lenient tokenizer (`walk` and entity decoding) into core;
- ADO's rules stay in ado, and Confluence's go in this crate;
- it lacks GFM tables, task lists, CDATA and the `ac:`/`ri:` elements.

**If its hand-written Markdown parser proves too weak** for page-sized
documents (nested lists inside tables, say), Markdown → storage switches to
`pulldown-cmark`. That is one dependency, with its reason in the commit
message.

**The mapping**, tested in both directions (storage → Markdown → storage
gives back the same storage for every row but the lossy ones):

| Storage | Markdown |
|---|---|
| `h1`–`h6`, `p`, `strong`, `em`, `s`, `code`, `blockquote`, `hr`, nested `ul`/`ol` | the same |
| `table` with plain cells | GFM table. A cell holding blocks is lossy |
| `code` and `noformat` macros | a fenced block with the language |
| `info`, `tip`, `note`, `warning` | `> [!NOTE]`, `> [!TIP]`, `> [!IMPORTANT]`, `> [!WARNING]` |
| `expand` | `<details><summary>Title</summary>…</details>` |
| `ac:task-list` | `- [ ]` and `- [x]` |
| `ac:link` with `ri:page` (pages are linked by title) | `[text](<KEY:Title>)`: the ref `page get` takes |
| `ac:link` with `ri:user ri:account-id` | `@<Display Name>`, as ado writes mentions. Back: the page's own mentions first, then user search; two matches is exit 2 |
| `ac:image` with `ri:attachment` | `![alt](attachment:name.png)` |
| `ac:emoticon`, `time` | `:shortname:`, the date |
| `ac:adf-extension` | its `ac:adf-node`; the `ac:adf-fallback` duplicates it and is skipped |
| `ac:layout` | its cells in order (lossy) |
| `ac:inline-comment-marker` | its text (lossy: the anchor) |
| `status`, `jira`, `toc`, `children`, `include`, `details`, app macros | a visible marker, `⟦jira PROJ-12⟧` or `⟦toc⟧` (lossy) |

The way up, if trials show agents need to rewrite macro-heavy pages whole:
make the markers placeholders. An update re-reads the page anyway, numbers
its fragments the same way, and swaps each marker still in the new Markdown
back to its original storage. A deleted marker deletes its macro.

## Phase 3: Data Center, only if the work instance runs it

Data Center has only v1 (`/rest/api`) and personal access tokens, no ADF,
storage everywhere, and reaches end of life on 2029-03-28. Atlassian stopped
selling it to new customers on 2026-03-30.

**Already shared, nothing to add:**
- CQL search (DC's `/rest/api/search` has no `next`: page by `totalSize`);
- the current user;
- label add and remove;
- attachment upload and download (DC serves the file without a redirect);
- `comment list` (CQL).

**What the adapter adds:** v1 request builders, and one v1 → row mapper, for:
- page get (`/rest/api/content/{id}?expand=body.storage,version,space,ancestors,metadata.labels`);
- children (`/child/page`);
- versions (`/rest/experimental/content/{id}/version`, undocumented, 9.2 and
  later);
- an old version (`?status=historical&version=N`);
- the attachment list (`/child/attachment`);
- spaces (`/rest/api/space`);
- create, update and delete (`/rest/api/content`);
- the footer comment.

**Deltas:**
- The base carries a context path (`/confluence`).
- People are `username` and `userKey`; mentions are `ri:userkey`.
- Descendants come from CQL `ancestor = ID`, which has no tree order.
- `limit` is capped silently: 500 bare, 200 with the version, 50 with a
  body.
- Times may carry offsets (`+1100`).
- Always send `Accept: application/json`.
- A bad PAT answers 200 as anonymous.

**Telling them apart:** the host. Cloud's REST API is only ever on
`*.atlassian.net`.

**Live test:** `atlassian/confluence:10.2.x` with Postgres, on Atlassian's
3-hour timebomb licence.
- The setup wizard runs once, by hand.
- After that, the volumes are snapshotted and restored for each run.
- A personal access token is minted with `POST /rest/pat/latest/tokens`.

## Joins

- **ado:** work item and pull request descriptions link Confluence pages, and
  `page get` takes those URLs. Page bodies keep ADO links as URLs, which
  `ado workitem get` and `ado pr get` take.
- **Runbooks:** they name commands in other domains (`agent-cli airflow task
  retry …`, `agent-cli aisearch indexer run …`) as plain text.
- **No id fields:** no field holds another domain's id, because Confluence
  models none.

## The contoso world

`fixtures/world/http/confluence.json` (`https://contoso.atlassian.net`), and
the facts in `fixtures/world/facts/confluence.md`:

**Space `ENG`, "Engineering"** (homepage 1000):
- "Runbooks" (1100):
  - **"Runbook: etl_nightly"** (1101, labels `runbook` and `airflow`). Its
    section "An order without customer_id" says to fix the order in the
    CRM, then retry `load_orders` and re-run the `orders-sql` indexer. Sam
    Lee's open inline comment on "retry the load" reads "only once the CRM
    fix has synced".
  - **"Runbook: worker crash loop"** (1102): rotate
    `kv-contoso-prod/worker-db-password`, linking ADO 1219.
- "Release notes" (1200):
  - **"Release notes v1.4.2"** (1201):
    - lists PR 431 and work items 1207 and 1210 as ADO URLs;
    - version 3, by Jane Doe;
    - a footer comment by Priya Patel;
    - attachments `changes-v1.4.2.txt` (text) and `rollout.png`.
- **"Orders search"** (1300): the AI Search design page. It holds a `jira`
  macro and a `toc`, so its `lossy` is non-empty.

**Recordings:**
- the searches trials make (`runbook etl_nightly`, `label = runbook`,
  `--since 7d`);
- those pages, their trees, comments, attachments (the redirect and the
  media host) and versions;
- `users-bulk` and `user/current`.

**World checks:** `world_confluence.rs`, plus the missing-order trace in
`world_cross.rs` (see the aisearch plan).

## Trials

**The sandbox:** a free Cloud site (10 users; API tokens work; no anonymous
access), seeded by `scripts/confluence-sandbox.py`, as ado's sandbox is.

**The arms:** agent-cli against curl (and against `twg`, if its OAuth works
in WSL), with Sonnet and Haiku.

**The tasks:**
1. Find the runbook for the failed `etl_nightly` load and list its steps.
2. What changed on the release notes page this week, and who changed it?
3. List the open inline comments on the release notes and the text each
   points at.
4. Save the changelog attachment from the release notes to a file.
5. Phase 2: publish a postmortem under "Incidents" from a Markdown file.
6. Phase 2: add a "Rollback" section to a runbook that has macros, without
   touching them.

It earns its place if agent-cli wins at least half the tasks, by 20% of
tokens or on correctness (the raw-tools rule in `TODO.md`).

## Search

`crates/confluence/search.toml`.

**Synonyms:**
- `wiki`, `doc`, `docs`, `runbook`, `article`, `blog`, `kb` → page;
- `children`, `hierarchy` → tree;
- `revision`, `history` → version. `history` is in `SHARED_WORDS`, so it
  needs labeled queries for this third reading.
- **Not `document`:** that is aisearch's resource.

**Queries, at least two per command:**
- "find the confluence runbook for etl_nightly" → `page list`
- "read the release notes page in confluence" → `page get`
- "what pages are under Runbooks" → `tree get`
- "unresolved comments on the release notes page" → `comment list`
- "download the file attached to the wiki page" → `attachment get`
- "who edited the runbook this week" → `version list`

## Open questions

1. Does work run Cloud or Data Center (is the host under `atlassian.net`)?
   Classic or scoped tokens, and is it SSO-only?
2. Are heading sections enough for edits? Storage has no node ids;
   Atlassian's own tools edit ADF nodes by `localId`. Trial task 6 decides.
3. Do `content/search`'s comment expands carry the resolution and the
   inline selection on Cloud? If not, use v2's calls per root comment.
4. The exact text of v2's 400s for a duplicate title and for malformed
   storage, and of the approval-space 409.
5. How many ids `users-bulk` takes, and whether v2 updates notify watchers
   (v2 has no `minorEdit`).

## Build order

1. The crate, config, client (both bases, paging, errors) and doctor; then
   `space list`, `page list` and `page get` with the converter; the world;
   trials 1 and 2.
2. `tree get`, `comment list`, `attachment list|get`, `version list`; trials
   3 and 4.
3. The writes; trials 5 and 6.
4. Data Center, if needed.
