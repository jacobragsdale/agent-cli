# Plan: the aisearch domain

Design of 2026-10-06, from the REST specs (data plane `2026-04-01` GA and
`2026-08-01-preview`, management `2025-05-01`), Microsoft Learn, and the
agent tools already out there (Azure MCP's search tools, community MCP
servers, `az search`).

Azure AI Search becomes a fourth domain in `crates/azure`, beside kv, acr
and aks. It shares their `az login`, their one Resource Graph inventory and
the `[azure]` allowlists, so **several services work from the first
command**. The name can't be `search`, which is the built-in
`agent-cli search`.

There is no live service, so fixtures come from the spec's examples. A
Free-tier service would back live tests (see Live tests).

## Does it earn a domain

Tasks:

1. Which services and indexes exist, how full they are, and the tier's
   limits.
2. An index's schema: which fields are filterable, sortable or facetable;
   the vector fields and their vectorizers; the semantic configurations.
   This is what an agent needs to write a filter or choose a query mode.
3. Query an index the way the app does (keyword, semantic, vector, hybrid)
   to debug retrieval: why doesn't X come back?
4. Look up one document by its key.
5. Why a document is missing or stale: the indexer's state, its last run,
   and the item errors.
6. Phase 2: run or reset an indexer and wait for it; change an index;
   upload or delete documents.

What an agent loses without it:

- **`az search` covers only the management plane.**
- **The data plane means curl with credentials:**
  - an `api-key` the agent must fetch first: `az search admin-key show`
    prints an admin key into the transcript;
  - or a token, which the default `apiKeyOnly` setting refuses.
- **Results carry each vector field whole:** 1,536 floats, about 20 KB a
  document.
- **The service's own words mislead:** an indexer's `status: "running"`
  means healthy, not executing.
- **OData filters have traps:** doubled quotes, `search.in` delimiters,
  case-sensitive field names.
- **Azure MCP doesn't fill the gap.** Its six search tools have no filter,
  select, lookup, stats or indexer tools; they pin `top` at 20 and default
  to full Lucene syntax. Nothing else covers schema, every query mode,
  lookup, and indexer errors.

## Config and sign-in

`[azure]` gains one key. Like `vaults`, it is optional; empty means every
service the login reaches.

```toml
[azure]
search_services = ["srch-contoso-prod", "srch-contoso-dev"]
```

**Discovery:** `graph.rs`'s one Resource Graph query adds
`microsoft.search/searchservices`. It projects:
- `endpoint`, from `properties.endpoint`. ARM appends a trailing slash. With
  no endpoint, use `https://{name}.search.windows.net`. A preview option
  makes hosts like `{name}-{hash}.sg.search.windows.net`, so prefer the
  endpoint when there is one;
- `sku.name`, `replicaCount`, `partitionCount`, `status`,
  `publicNetworkAccess`, `semanticSearch`;
- `authOptions` and `disableLocalAuth`.

It is unconfirmed whether Resource Graph rows carry `endpoint` and
`authOptions`. If they don't, make one ARM
`GET …/searchServices/{name}?api-version=2025-05-01` per service, cached
for `refresh`.

**Auth is decided per service, from its own settings,** and never configured:

| The service's setting | What the CLI sends |
|---|---|
| `disableLocalAuth: true`, or `authOptions.aadOrApiKey` | An `az` token for `https://search.azure.com` (`bearer(ctx, SEARCH)`). |
| `apiKeyOnly`, the default for a new service | An admin key from ARM `POST …/listAdminKeys?api-version=2025-05-01`. It is a `Request::query` (a POST that only reads). The key is held in a `Secret` in memory for this run only (never `ctx.cache`, which persists) and sent as the `api-key` header. |

Rules:
- **Never send both:** when a request carries both, the key wins.
- **A refused token on an `aadOrApiKey` service** (401 or 403) is retried
  once with a key.
- **Otherwise it is exit 3**, naming the role the call needs:

  | Call | Role |
  |---|---|
  | queries | Search Index Data Reader |
  | definitions, stats, indexers | Reader or Search Service Contributor |
  | writes | Search Service Contributor, plus Search Index Data Contributor for documents |
  | fetching keys | Contributor or Search Service Contributor |

  Role assignments take 5 to 10 minutes to apply. Owner and Contributor
  get no data access with a token; they can only fetch keys.
- **Hosts:** a token or key goes only to `*.search.windows.net`
  (`host_under`). Key fetches go to ARM.
- **Versions:** data plane `api-version=2026-04-01`, where every phase-1
  read is GA. ARM `2025-05-01`. Preview stays out: list paging changed
  twice in 2026's previews.

**Doctor** (azure's `doctor.rs`), for the first three reachable services: the
auth mode, and `GET /servicestats`, the cheapest call that proves auth works.

**Status line:** `aisearch all services`, or `aisearch 2 services`.

## Ids

| Thing | `id` | Taken by | Also accepted |
|---|---|---|---|
| service | `srch-contoso-prod` | `--service`, `service get` | its endpoint URL, its portal URL |
| index | `srch-contoso-prod/orders` | `index get`, `document list`, the writes | its forms in the list below |
| document | `srch-contoso-prod/orders/88123` | `document get`, `document delete` | a bare key with `--index`; `…/indexes/orders/docs/88123`; `docs('88123')` |
| indexer | `srch-contoso-prod/orders-sql` | `indexer get`, `indexer run`, `indexer wait` | a bare name with `--service`; `…/indexers/orders-sql[/status]`; the portal's `IndexerJsonEditor…%23orders-sql` |

An index can also be given as:
- a bare `orders`: the one service that holds it, and two holders is exit
  2, as with acr's bare repositories. Or name it with `--service`;
- `https://{svc}.search.windows.net/indexes/orders`, or `indexes('orders')`;
- a portal link, `#view/Microsoft_Azure_Search/Index.ReactView/id/…%23orders`;
- an alias name, for document reads and `index get`.

Rules:
- **`/` is safe as the separator.** A key holds only letters, digits, `-`,
  `_` and `=`, and the service enforces it.
- **Keys are carried verbatim.** Indexer keys are UrlTokenEncoded and end in
  a padding digit, so never re-encode one.
- **Portal links:**
  1. percent-decode the fragment;
  2. find `Microsoft.Search/searchServices/{svc}`, ignoring case;
  3. the `#{name}` after it names the object.

  URLs are parsed, never fetched.

## Phase 1: reads

### `aisearch service list`

From the inventory alone; no data-plane call. Takes `--limit`.

**Returns** `[{id, endpoint, sku, replicas, partitions, status, auth, semantic, network, subscription, resource_group, location}]`.
`auth` is `token` or `key`, the mode the CLI will use.

### `aisearch service get SERVICE`

`GET /servicestats`.

**Returns** `{id, endpoint, sku, replicas, partitions, status, auth, semantic, network, usage{…}, limits{…}}`:
- `usage` is `{documents, indexes, indexers, data_sources, skillsets, synonym_maps, aliases, storage, vector_storage}`,
  each `{used, quota}` (bytes for storage);
- `limits` is `{fields_per_index, storage_per_index, …}`.

It answers "why was that create refused with a 429": the tier's quota.

### `aisearch index list [NAME]`

**Calls, per service:**
- `GET /indexes?$select=name,fields,semantic,vectorSearch`. GA lists aren't
  paged; `$select` keeps the answer small.
- `GET /indexes/{name}/stats` for each index, in parallel.

**Ceiling:** one stats call per index, until `GET /indexstats` leaves
preview. Marked `ponytail:`.

**Flags:** `--service` (repeatable, within `[azure] search_services`),
`--limit`.

**Returns** `[{id, service, name, documents, storage, vector_storage, fields, vector_fields, semantic}]`.
`semantic` is the default semantic configuration.

### `aisearch index get INDEX`

`GET /indexes/{name}` and `/stats`.

**Returns** `{id, service, name, documents, storage, vector_storage, key, fields[{name, type, attrs[], analyzer, dimensions, profile, fields[]}], semantic{default, configs[{name, title, content[], keywords[]}]}, vector{profiles[{name, algorithm, vectorizer, compression}], vectorizers[{name, kind, model}]}, scoring_profiles[], suggesters[], etag}`.
`attrs` lists only the true ones of key, searchable, filterable, sortable,
facetable, retrievable and stored, so there is no `false` noise.

**`--full` adds `definition`:** the whole JSON, with every secret replaced by
`"<unchanged>"`, the value a PUT keeps. The service redacts unevenly
(`null`, `"<redacted>"`, or unknown), and its own examples hold whole
`AccountKey=` strings, so the CLI scrubs regardless.

What it scrubs:
- any `apiKey`;
- a `key` under `amlParameters`;
- every `httpHeaders` value;
- `?code=` in a `uri`;
- `applicationSecret`;
- any `connectionString`;
- by pattern, `AccountKey=`, `SharedAccessSignature=`, `sig=` and
  `Password=` in any string.

It never touches booleans (`key: true`) or document keys.

With `--output FILE`, it saves just the definition, ready for `index update`.

### `aisearch document list INDEX [TEXT]`

The query: `POST /indexes/{i}/docs/search` with `count: true`. It is always a
POST: vector queries are POST-only, and a URL is capped at 8 KB.

**Query modes:**

| Flag | What it sends |
|---|---|
| `--mode keyword` (the default) | `search: TEXT` (`*` when blank), with `queryType: simple`. |
| `--mode vector` | `vectorQueries: [{kind: "text", text: TEXT, k: limit, fields}]`, and no `search`. |
| `--mode hybrid` | both, fused by RRF. |
| `--semantic` | Reranks keyword or hybrid results: `queryType: semantic`, `captions: extractive`, with `--semantic-config NAME` or the index's default. |
| `--lucene` | Full Lucene syntax: fields, fuzzy, regex. Off by default, because `full` turns `? : / - "` in plain text into operators. |

Semantic ranking, in the service's own words where it refuses:
- only the top 50 are reranked;
- combined with `--orderby` it is refused;
- 206 means a partial result (a note gives the reason);
- 402 means the free semantic quota is spent (a hint).

**`--vector-field F`** (repeatable) defaults to every vector field that has a
vectorizer.

**Other flags:** `--filter ODATA`, `--select a,b`, `--orderby "f desc"`,
`--skip N` (at most 100,000), `--limit` (default 50: `top`, at most 1,000 a
request).

**The index definition:**
- **Read and cached:** it is read once and cached for `[azure] refresh`
  (only its field list, which holds no secrets).
- **What it gives:** the key field, which builds each row's `id`, since
  results don't mark it; the vector fields and their vectorizers; and the
  default semantic configuration.
- **When it can't be read** (Search Index Data Reader can't): rows use a
  field named `id` if there is one, and a note names the role that reads
  definitions. Vector modes then need `--vector-field`.

**Output is bounded without trusting the schema:**
- a numeric array longer than 16 prints as `"[1536 floats]"`;
- strings over 200 characters are cut (`document get` shows one whole);
- arrays keep their first 5 items.

**Returns** `[{id, score, reranker, caption, highlights, doc}]`:
- `score` is BM25 for keyword and RRF for hybrid. RRF scores are small:
  about 0.03 is a strong match;
- `reranker` (0 to 4) is there with `--semantic`;
- `doc` holds the fields.

The note reads `[50 of 1,234; --limit N or --skip 50]`, from `@odata.count`.
No existing agent tool reports that count.

### `aisearch document get DOCUMENT...`

`GET /indexes/{i}/docs/{key}`, with the key percent-encoded.
- **Flags:** `--select`, and `--vectors`, which prints vector fields whole.
- **A 404** is exit 4, with the hint
  `agent-cli aisearch indexer list --service SERVICE --failing`. A missing
  document is most often an indexer's failed item.
- **Returns** `{id, doc}`, or an array for several documents.

### `aisearch indexer list`

**Calls, per service:**
- `GET /indexers?$select=name,targetIndexName,dataSourceName,skillsetName,schedule,disabled`;
- `GET /indexers/{n}/status` for each, in parallel.

**Flags:**
- `--failing`: the last run did not succeed, or it had failed items, or the
  indexer's status is `error`;
- `--service`, `--limit`.

**Returns** `[{id, index, source, skillset, schedule, disabled, health, last_status, last_start, last_end, processed, failed}]`:
- `index` is the index's id;
- `health` is the service's `status`, but `running` prints as `ok`: it means
  healthy, not executing;
- `last_status` is the last run's own: `inProgress`, `success`,
  `transientFailure`, `persistentFailure` or `reset`. An unknown value is
  treated as a failure.

### `aisearch indexer get INDEXER`

**Calls:**
- `GET /indexers/{n}`;
- `/status`;
- its data source, `GET /datasources/{ds}`, for the type and container. Its
  connection string is always `null` on a GET.

**Returns** `{id, index, source{name, type, container, query}, skillset, schedule, disabled, health, last{status, start, end, processed, failed, error}, errors[{key, message, name, status, details}], warnings[{key, message, name}], history[{status, start, end, processed, failed}]}`:
- `errors` and `warnings` come from the last run, the first 20 of each,
  with a note when there are more. Failed items under a "success" still
  show;
- `history` is the last 10 of the up to 50 runs the service keeps;
- times are printed in whole seconds (the service sends milliseconds).

When the last run has failed items, the note is
`[next: agent-cli aisearch document get SERVICE/INDEX/KEY]` for the first
error's key: is it missing, or there but stale?

## Phase 2: writes

Every write goes through `ctx.write`.
- `--dry-run` prints the request. The `api-key` header is masked, because
  core masks `*-key` headers.
- Read-only mode refuses it.

### `aisearch indexer run INDEXER [--reset]`

Write.
- **Calls:** `POST /indexers/{n}/run`, which answers 202.
- **With `--reset`**, it first calls `POST /reset` (204). That forgets the
  high-water mark, so the run re-reads everything and re-runs every skill,
  which costs money.
- **Already running** answers 409, which becomes exit 5 with the hint
  `agent-cli aisearch indexer wait ID`. Unconfirmed: Microsoft's own sample
  expects a 429.
- **A run can't be stopped** once it starts, so there is no `cancel`.
- **Returns** `{id, reset, requested}`, with the note
  `[next: agent-cli aisearch indexer wait ID --since TIME]`, where TIME is
  the moment before the run.

### `aisearch indexer wait INDEXER [--since T]`

Read; declares `timeout: 100`.

It polls `/status` every 5 seconds until `lastResult.startTime >= since` and
`status != inProgress`. The service offers nothing else to poll.

**Exit:**
- **success:** prints `indexer get`'s row;
- **a failed run:** `Failure::…with_data(row)`;
- **the deadline:** exit 124, with the last status as data.

### `aisearch index create INDEX`

Write. Its args: `--definition JSON|-`, or `--definition-file F`.

- **Calls:** `PUT /indexes/{name}` with `If-None-Match: *` and
  `Prefer: return=representation`. A 412 is exit 5: it already exists.
  Unconfirmed: that the service honours `If-None-Match` here.
- **Returns** `index get`'s summary.

### `aisearch index update INDEX`

Write. Its args: `--definition JSON|-`, or `--definition-file F`, plus
`--if-etag E` and `--allow-downtime`.

- **Calls:** `PUT` with `If-Match` (a 412 is exit 5) and
  `Prefer: return=representation`. Without that header, an update answers
  204 with no body.
- **Changes the service refuses:** types, attributes, dimensions, removed
  fields. They answer `CannotChangeExistingField`, which is exit 2 with the
  hint: create a new index, then point an alias at it.
- **Verify on the Free service** that `"<unchanged>"` keeps a vectorizer's
  `apiKey`.

### `aisearch index delete INDEX`

Destructive. `DELETE /indexes/{name}`. The service refuses (400) an index an
alias points at.

### Documents: `document create`, `update` and `delete`

| Command | Effect | Action sent |
|---|---|---|
| `aisearch document create INDEX --docs JSON\|-` (or `--docs-file F`) | Write | `upload`: insert or replace. |
| `aisearch document update INDEX --docs …` | Write | `merge`, or with `--upsert`, `mergeOrUpload`. |
| `aisearch document delete DOCUMENT...` | Destructive | `delete`. |

All three go through `POST /indexes/{i}/docs/index`.
- **Batches:** at most 1,000 documents or about 16 MB; the input is a JSON
  array or JSON lines.
- **200** means every item succeeded.
- **207** means some failed. It becomes `Failure::…with_data([{key, status, error}])`,
  exit 1, with the failed keys. Items that failed with 409, 422 or 503 can
  be retried; 400 and 404 can't.

## Later, when asked

- **Knowledge bases (agentic retrieval).** GA since `2026-04-01`, but
  extractive only, and still churning:
  - `knowledge-base list|get`;
  - `knowledge-source list|get`, with its sync status;
  - a retrieve, `POST /knowledgebases/{kb}/retrieve`, which is a read.
- **Facets** (`facet list INDEX --field F`), for when trials show agents
  guessing filter values.
- **The analyzer** (`token list INDEX TEXT --analyzer A`).
- **Smaller pieces:** synonym maps, aliases, skillsets in detail, writes to
  data sources and skillsets, and `resetdocs` (preview).

## Errors and throttling

- **The error body** is `{"error":{"code":"","message":"…"}}`, and the code is
  often empty. Core's `failure_message` reads it already.
- **503 is the throttle.** No `Retry-After` is documented, so core's one
  retry for reads applies.
- **429 is a quota, not a rate:** too many objects for the tier, or the
  storage is full. The client replaces core's "still throttling" hint with
  "an AI Search 429 is the tier's quota: agent-cli aisearch service get
  SERVICE shows its usage". Core still retries a 429 once first; making it
  not retry would be a per-domain switch in core.
- **The rest, core already maps:** 409 and 412 are exit 5, 404 is 4, 400 is
  2, and a 401 is exit 3.

## The contoso world

The Resource Graph answer in `fixtures/world/http/azure.json` gains the
services. Their data-plane hosts are recorded in the same file. The facts go
in `fixtures/world/facts/aisearch.md`.

**The services:**
- `srch-contoso-prod`: standard, `aadOrApiKey`, so a token;
- `srch-contoso-dev`: basic, `apiKeyOnly`, so a key from `listAdminKeys`.

Between them they cover two services and both auth paths.

**Index `orders` on prod:**

| Field | Attributes |
|---|---|
| `id` | key |
| `customer_id` | filterable |
| `status` | filterable, facetable |
| `total` | sortable |
| `created` | sortable |
| `summary` | searchable |
| `summary_vector` | 1,536 dimensions, vectorizer `aoai-embed`; retrievable, so the stripping shows |

Its default semantic configuration is `orders-semantic`, and it holds 88,120
documents.

**Indexer `orders-sql`:** data source `orders-sql` (`azuresql`, table
`dbo.orders`), hourly.
- Its last run (2026-09-29T00:40Z) succeeded with one failed item, key
  `88123`.
- That item's error names the null `customer_id`. It is the same order that
  failed `etl_nightly`'s `load_orders`.

**The cross-domain trace** (`world_cross.rs`):

```sh
agent-cli aisearch document get srch-contoso-prod/orders/88123           # exit 4
agent-cli aisearch indexer list --failing --fields id,failed
agent-cli aisearch indexer get srch-contoso-prod/orders-sql --fields errors
agent-cli confluence page list customer_id --label runbook --fields id,title
agent-cli confluence page get 1101 --section "An order without customer_id"
agent-cli airflow task logs etl_nightly/latest/load_orders/2 --tail 20
```

## Live tests

There is no work service to test against. A Free service would serve, in any
subscription where the user can make one. It costs $0, with limits:
- one per subscription;
- 50 MB, 3 indexes and 3 indexers;
- an indexer can run at most once every 180 seconds;
- semantic ranking works only in some regions;
- Microsoft may delete it after long inactivity.

The suite is opt-in, behind `AGENT_CLI_TEST_AISEARCH=1`. It makes its own
index, uploads documents, and runs a small indexer.

## Open questions

1. Do Resource Graph rows carry `properties.endpoint` and `authOptions`? If
   not, use one ARM GET per service.
2. Do the work services take tokens (`aadOrApiKey`)? Does the user hold
   Reader and Search Index Data Reader, or only Contributor, which fetches
   keys?
3. Is a concurrent `indexer run` refused with 409 or 429?
4. Does `"<unchanged>"` keep a vectorizer's `apiKey` on an index PUT?
5. Is there a subscription where a Free service can be made for live tests?

## Build order

1. `[azure] search_services`, the inventory, auth, `service list|get` and
   `index list|get`, plus the world's services.
   - The azure card must stay at most 3 KB: trim it as aisearch's lines go
     in.
   - `check_registry` checks the new resources (`index`, `document`,
     `indexer`) against the other domains' synonyms.
2. `document list|get` and `indexer list|get`; the cross-domain trace;
   trials against the world.
3. The writes, then live tests on a Free service.
