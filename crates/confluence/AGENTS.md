# confluence: Confluence Cloud

Spaces, pages (as bounded Markdown), trees, comments, attachments and
versions over v2 (`/wiki/api/v2`), v1 (`/wiki/rest/api`) only where v2 has
no call: search, `user/current`, user search, labels, upload, download,
move. Cloud only: a host not under `atlassian.net` with no `cloud_id` is
exit 3. https://developer.atlassian.com/cloud/confluence/rest/v2/

## Config
`[confluence]`: `url` (the site), `email`, `token`, `token_env` or
`token_cmd` (an API token, sent as Basic email:token to exactly the site's
host), `cloud_id` (scoped tokens: calls go to exactly `api.atlassian.com`,
Bearer when there is no email). Doctor needs `user/current` to be `known`:
a bad token on an open site reads as anonymous, never 401.

## Ids
Page `1101`, `1101@3` (a version), `KEY:Title`, or any web URL (`ids.rs`:
`PageRef`; tiny `/x/` links decoded by `tiny`). Space: its key. Comment
`5002` or `focusedCommentId`. Attachment `att7001`, or its URLs. A URL on
another host is exit 2; share links can't be read.

## Where things are (`src/`)
- `lib.rs`: `DOMAIN`. `doctor.rs`. `config.rs`: `Section`, `Site` (the
  Cloud check). `client.rs`: `Confluence::load`, `v2`,
  `v1`, `get`, `query`, `change`, `read`/`write` (a `Call`), `list` (v2
  paging), `search` (v1 paging), `names` (users-bulk), `space`,
  `space_key`, `content` (a page, else a blog post); rows `text`, `stamp`,
  `note_more`.
- `storage.rs`: a lenient storage tree with byte spans, `sections`, `find`.
  `markdown.rs`: storage to Markdown with `lossy`. `compose.rs`: Markdown
  to storage (pulldown-cmark). `syntax.rs`: escapes, fences, links.
- `page/mod.rs`: `Written`, `people` (mentions to account ids), `labels`.
- `testing.rs`: `confluence`, `piped`, `dry_run`, `urls`, `page`, `space`.

## Fixtures
`fixtures/world/http/confluence.json`, `fixtures/world/facts/confluence.md`,
`crates/cli/tests/world_confluence.rs`; queries `search.toml`.

## Quirks
- v2 answers 404 for missing and for not permitted alike.
- v2 `next` links start with `/wiki`; v1's are relative to it and drop
  parameters, which `search` adds back. Stop only when `next` is absent.
- CQL: `siteSearch` only counts first; `text ~` is its floor; dates are
  `now("-Nm")`, since absolute ones read in the caller's time zone.
- Section edits splice storage bytes: what is kept is never re-serialized.
  Only headings at the root or in a layout cell rule sections.
- A download redirects to a pre-signed media URL: fetched with no
  credential, never printed (`attachment/get.rs`).
- Comment resolution comes from v2's inline list, not CQL's expand, which
  needs profile visibility.

## Never needed
Other crates' sources, `docs/plans/`, `docs/reference/`.
