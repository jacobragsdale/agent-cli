# confluence: Confluence Data Center and Server

Spaces, pages (as bounded Markdown), trees, comments, attachments and
versions on a self-hosted Confluence, over v1 (`/rest/api`) under the base;
versions from `/rest/experimental`, inline comments from the plugin's
`/rest/inlinecomments/1.0` (v1 has neither). Checked against 9.2 live.
Cloud is exit 3. https://developer.atlassian.com/server/confluence/rest/v923/

## Config
`[confluence]`: `url` (what the browser shows, context path included;
`check_base_url`), `token`, `token_env`, `token_cmd` or `token_kv` (a
personal access token, Bearer, only under the base: `same_origin`; a
`token_kv` refused with a 401 is read again from Key Vault in
`authorization`). Doctor needs `user/current` to be `known`.

## Ids
Page `1101`, `1101@3`, `KEY:Title` (`~user:Title`), or a web URL
(`ids.rs`: `PageRef`, `tiny` for `/x/`). Space: its key. Comment `5002` or
`focusedCommentId`. Attachment `7001` (or `att7001`). A URL off the base is
exit 2. Mentions are `ri:userkey` in storage, `@<Name>` in Markdown.

## Where things are (`src/`)
- `lib.rs`: `DOMAIN`. `doctor.rs`. `config.rs`: `Section`, `Site`.
- `client.rs`: `Confluence::load`, `api`, `url` (`INLINE`,
  `EXPERIMENTAL`), `web`, `get`, `change`, `read`/`write` (a `Call`),
  `list` (`next` paging), `search` (CQL, `start` paging), `names`,
  `space`, `content` (`PAGE` expands); rows `text`, `person`, `stamp`.
- `storage.rs`: storage tree with byte spans, `sections`, `find`.
  `markdown.rs` (with `lossy`) and `compose.rs`: storage to Markdown and
  back. `syntax.rs`: escapes, fences, links.
- `page/mod.rs`: `Written`, `people` (mentions to user keys), `labels`.
- `testing.rs`: `confluence`, `piped`, `dry_run`, `urls`, `page`, `space`.

## Fixtures
`fixtures/world/http/confluence.json`, `fixtures/world/facts/confluence.md`,
`crates/cli/tests/world_confluence.rs`; queries `search.toml`. Live:
`scripts/confluence-up.sh`, then `AGENT_CLI_TEST_CONFLUENCE=1 cargo test
-p agent-cli --test live_confluence`.

## Quirks
- Missing and not permitted are both a 404.
- `next` is relative to the base; `/search` has none (`start` to
  `totalSize`). Limits drop silently, so a short page never ends a list.
- CQL: `siteSearch` only first, `text ~` its floor; dates as `now("-Nm")`.
  People: `type = user and user.fullname ~ "…"`.
- Section edits splice storage bytes; only root or layout-cell headings
  rule sections.
- A PUT without a body keeps it (titles, moves). Moves stay in the space.
- Inline: the plugin finds the text by count and index and marks the page;
  without `serializedHighlights` it saves no comment. Inline replies go to
  the plugin (v1 makes them footer comments); resolving checks the kind.
- A file name uploads once; versions go to `…/{id}/data`.
- The tree walks `child/page`: no descendants call works.

## Never needed
Other crates' sources, `docs/plans/`, `docs/reference/`.
