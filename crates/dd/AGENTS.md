# dd: Datadog

Logs, metrics, monitors, downtimes, events, APM services and spans,
incidents, hosts, containers, SLOs and dashboards over Datadog's REST API:
https://docs.datadoghq.com/api/latest/

## Config
`[datadog]` (or `[dd]`): `site` (`datadoghq.eu`, …; else `DD_SITE`, else
`datadoghq.com`), `env` (the default `env:` tag). Credential, first found:
`token_env` or `token_cmd` (a bearer), the pair `api_key_*` with
`app_key_*`, `DD_ACCESS_TOKEN`, `DD_API_KEY` with `DD_APP_KEY`. A literal
key or token in the file is refused. It goes only to `api.<site>`
(`host_under`).

## Ids
Numeric ids (`4711`) or the web URL that shows the thing (`Dd::web_id`: the
`<marker>_id` query parameter, else the path segment after the marker; a URL
on another site is exit 2). Rows name pods as the k8s id
`cluster/namespace/pod` (`pod_ref`, from `kube_*` tags), and `--pod` takes
one back (`kube_tags`).

## Where things are (`src/`)
A command is `<resource>/<verb>.rs`: its args, rows, handler, `command!`
and tests (`log_count/list.rs` is `dd log-count list`). Copy a sibling.
- `lib.rs`: `DOMAIN`, whose `commands` registers every command (its order
  is the listing's). `doctor.rs`: status and doctor.
- `client.rs`: `Dd::load(ctx)`, then `get(ctx, path, query)` a read,
  `search(ctx, path, body)` a POST that only reads, `change(...)` a write,
  `url`, `app` (web links). `Window` (`--since`/`--until` as ms or seconds),
  tag filters (`tags`, `kube_tags`, `any_of`, `search_query`), and row
  helpers `text`, `strings`, `tag`, `pod_ref`, `utc_ms`, `epoch`, `cut`,
  `limited`.
- `search.rs`: what the log, span and event searches share (`LogStatus`,
  paging, `MESSAGE_MAX`).
- `<resource>/mod.rs`: what its verbs share (`slo/mod.rs`: `SloRow`).
- `testing.rs`: `dd`, `dd_with`, `CONFIG`, `TOKEN`, sample rows.

## Fixtures
`fixtures/world/http/dd.json` (`api.datadoghq.eu`); facts
`fixtures/world/facts/dd.md`; world checks `crates/cli/tests/world_dd.rs`
and the alert trace in `world_cross.rs`; queries `crates/dd/search.toml`.

## Quirks
- Key headers (`DD-API-KEY`, `DD-APPLICATION-KEY`) attach at send time in
  `Call::perform`, after the host check; plans show them as `***`, and
  `--dry-run` never resolves a `token_cmd`.
- A downtime (`--for`) lasts at most 7 days, and needs `--yes`.
- A 403 names the missing scope in Datadog's words; the hint says where
  scopes are granted.
- Throttles wait out `X-RateLimit-Reset` (core).

## Never needed
Other crates' sources, `PLAN.md`, `docs/plans/`, `docs/reference/`.
