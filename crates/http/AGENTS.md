# http: REST calls, OpenAPI specs, TLS

Calls to any URL as curl makes them, OpenAPI specs found from a base URL or
docs page, queried and compared, and the certificate chains servers present.
What it adds to curl: a 4xx or 5xx exits by its status with the answer
printed, bodies are bounded and masked, sign-in comes from the environment
and stays on its origin, writes are guarded, the proxy is core's.

## Config
None. `[http] read_only = true` refuses every write. The proxy is `[core]`'s
(`proxy`, `no_proxy`, `proxy_user`); sign-in is per call: `--bearer-env`,
`--az`, `--basic USER:VAR`, `--header-env`.

## Ids
A request is its URL. A spec is the URL it was found at, or a file's path.
An operation is `SPEC#METHOD PATH` or `SPEC#operationId`, a schema
`SPEC#Name`; `--spec` takes the part before `#`. A certificate is
`HOST:PORT`.

## Where things are (`src/`)
- `call.rs`: `Call` (the shared flags), `Prepared::send` (the one door: the
  sign-in held to the first origin, redirects followed by hand), `body`.
- `answer.rs`: `Answer` (the row), `trace_id` (traceparent to Datadog's
  decimal), `finish` (status to exit code and hint).
- `url.rs`: `checked`, `with_query`, `origin`, `join`, `host_and_port`.
- `openapi/mod.rs`: `Spec`, `Operation`; `openapi/read.rs` reads 2.0, 3.0
  and 3.1 alike;
  `openapi/find.rs`: discovery, `WELL_KNOWN` (each framework's paths, the
  first eight confirmed by the harness); `openapi/load.rs`: a URL or file,
  `split`, `operation`; `openapi/fields.rs`: `Node`, `flatten` (field
  paths); `openapi/example.rs`; `openapi/check.rs` (`--spec`);
  `openapi/compare.rs`: `RULES`, `row` and the breaking classifier for
  `spec diff`; `openapi/yaml.rs`.
- `der.rs`: the few X.509 fields a probe prints, and `sha1` (thumbprints).
- `certificate/mod.rs`: `Probe`, `CertInfo`, `refusal`.
- `doctor.rs`: the proxy, and whether TLS is intercepted.
- `testing.rs`: `spec_v1`, `spec_v2`, the test CA and leaf in `testdata/`.

## Fixtures
`fixtures/world/http/http.json` (api and api-staging, Swashbuckle) and
`fixtures/world/tls/` (chains by `HOST_PORT.pem`); facts
`fixtures/world/facts/http.md`; world checks `crates/cli/tests/world_http.rs`
and the trace in `crates/cli/tests/world_cross.rs`; live,
`crates/cli/tests/live_http.rs` against `scripts/openapi-up.sh` (with
AGENT_CLI_TEST_HTTP=1), whose `scripts/openapi/FINDINGS.md` is what each
framework serves.

## Quirks
- ureq tunnels every proxied call by CONNECT, plain http too; a proxy that
  tunnels only to 443 refuses it, so internal hosts go in `no_proxy`.
- A 503 is retried only when it says when (`Retry-After`).
- Swashbuckle writes no `required`, .NET 9 writes every field required; the
  spec is read as written.

## Never needed
Other crates' sources, `docs/plans/`, `docs/reference/`.
