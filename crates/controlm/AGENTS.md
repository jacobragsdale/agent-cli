# controlm: BMC Control-M (on-prem)

Job runs (search, status, why one waits, output, Control-M's log), job
definitions, rerun and order, over an Enterprise Manager's Automation API
(`https://HOST:8443/automation-api`). Built from BMC's spec (the client
generated from it in GitHub's controlm/ctm-python-client, API 9.22.30)
before meeting a real one: each `VERIFY(work)` comment is an unchecked
assumption, and `docs/plans/controlm.md` is the order to check them in.

## Config
`[[controlm.instance]]`: `name` (`--instance`, `pick`), `base_url` (core's
`check_base_url`), `read_only`, `utc_offset` (the servers' clock; default
UTC). Credential: `username` with `password`, `password_env` or
`password_cmd` (a session token from `POST /session/login`, `Bearer`, once
per command and again after a 401), or `token`, `token_env`, `token_cmd`
(an API token, `x-api-key`). Only under `base_url/` (`same_origin`).

## Ids
A run: `SERVER:ORDER_ID` (`job_id`; words get a hint naming `job list
--name`). A definition: `SERVER/FOLDER/JOB`, sub-folders keeping their `/`
(`DefRef`); a run's row prints it as `definition`, which `job run` takes.

## Where things are (`src/`)
- `lib.rs`: `DOMAIN`. `doctor.rs`: status, and doctor's one live call
  (`config/servers`).
- `client.rs`: `ControlM::load`, `open` for a `Client`: `get`, `text`,
  `change` (checks `writable`); `Call`, the door attaching the credential;
  `refused`; `with_query`, `compact` (a time as the API takes it); row
  helpers `text`, `stamp`, `order_date`, `note_more`.
- `job/mod.rs`: `JobRow`, `JobIdArgs`, `Status`, `FAILED`.
- `definition/mod.rs`: `DefRef`, `DefRow`, `found` (walks a `deploy/jobs`
  answer).
- `testing.rs`: `controlm`, `controlm_with`, `paths`, `dry_run`, `CONFIG`,
  `PASSWORD_CONFIG`, `job`.

## Fixtures
Unit tests only, on answers shaped by the spec; no world recording yet.
Queries: `search.toml`.

## Quirks
- Times come as `YYYYMMDDhhmmss` on the servers' clock, order dates as
  `YYMMDD`; `stamp` and `order_date` turn them into UTC and days.
- Filters take `*` and comma lists; `Status::words` maps failed, ok, running
  and waiting to Control-M's words.
- `job run` orders with `ignoreCriteria`, so a job runs even on a day its
  calendar skips.

## Never needed
Other crates' sources, `docs/reference/`.
