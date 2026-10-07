# aisearch — Azure AI Search (16 commands)

| Command | Effect | Summary |
|---|---|---|
| [`aisearch service list`](#aisearch-service-list) | read | List the AI Search services the az login reaches, their tier and sign-in |
| [`aisearch service get`](#aisearch-service-get) | read | Show an AI Search service's storage and object counts against its tier's quotas |
| [`aisearch index list`](#aisearch-index-list) | read | List AI Search indexes with their document counts, storage and vector fields |
| [`aisearch index get`](#aisearch-index-get) | read | Show an AI Search index's schema: filterable fields, vectors, semantic configs |
| [`aisearch index create`](#aisearch-index-create) | write | Create an AI Search index from a JSON definition (never replaces one) |
| [`aisearch index update`](#aisearch-index-update) | write | Change an AI Search index's definition (fields added, semantic, scoring, CORS) |
| [`aisearch index delete`](#aisearch-index-delete) | destructive | Delete an AI Search index and every document in it |
| [`aisearch document list`](#aisearch-document-list) | read | Query an AI Search index: keyword, vector, hybrid or semantic search, filters |
| [`aisearch document get`](#aisearch-document-get) | read | Look up AI Search documents by key: is one in the index, and what it holds |
| [`aisearch document create`](#aisearch-document-create) | write | Upload documents to an AI Search index (insert, or replace whole by key) |
| [`aisearch document update`](#aisearch-document-update) | write | Change fields of AI Search documents by key (merge; --upsert adds missing ones) |
| [`aisearch document delete`](#aisearch-document-delete) | destructive | Delete AI Search documents by key from one index |
| [`aisearch indexer list`](#aisearch-indexer-list) | read | List AI Search indexers with their last run: status, items processed and failed |
| [`aisearch indexer get`](#aisearch-indexer-get) | read | Show an AI Search indexer's last run, its failed items' errors, and history |
| [`aisearch indexer run`](#aisearch-indexer-run) | write | Run an AI Search indexer now (--reset to re-read everything); it runs on its own |
| [`aisearch indexer wait`](#aisearch-indexer-wait) | read | Wait for an AI Search indexer run to end: exit 0 succeeded, 1 failed, 124 going |

### aisearch service list

```text
agent-cli aisearch service list — List the AI Search services the az login reaches, their tier and sign-in
  --limit int  (default 50)
Returns: [{id,endpoint,sku,replicas,partitions,status,auth,semantic,network,subscription,resource_group,location}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli aisearch service list --fields id,sku,auth,endpoint
```

### aisearch service get

```text
agent-cli aisearch service get — Show an AI Search service's storage and object counts against its tier's quotas
 *<service> str  The service: its name, its endpoint or its portal link
Returns: {id,endpoint,sku,replicas,partitions,status,auth,semantic,network,subscription,resource_group,location,usage{documents{used,quota},indexes{used,quota},indexers{used,quota},data_sources{used,quota},skillsets{used,quota},synonym_maps{used,quota},aliases{used,quota},storage{used,quota},vector_storage{used,quota}},limits{fields_per_index,storage_per_index,field_nesting_depth,complex_collections_per_index,complex_objects_per_document,indexer_seconds_per_day}}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli aisearch service get srch-contoso-prod --fields usage,limits
```

### aisearch index list

```text
agent-cli aisearch index list — List AI Search indexes with their document counts, storage and vector fields
  <name> str       Only indexes whose name holds this, ignoring case
  --service str[]  Search services to read (default: every one in reach)
  --limit int      (default 50)
Returns: [{id,service,name,documents,storage,vector_storage,fields,vector_fields[],semantic}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli aisearch index list --fields id,documents,storage,vector_fields
```

### aisearch index get

```text
agent-cli aisearch index get — Show an AI Search index's schema: filterable fields, vectors, semantic configs
 *<index> str    The index: SERVICE/INDEX from index list, a bare name, its URL or portal link
  --service str  The service that holds it; needed when more than one does
  --full         Add the whole definition, every secret as "<unchanged>" (--output FILE saves it)
Returns: {id,service,name,documents,storage,vector_storage,key,fields[{name,type,attrs[],analyzer,dimensions,profile,fields[{name,type,attrs[],analyzer,dimensions,profile,fields[]}]}],semantic{default,configs[{name,title,content[],keywords[]}]},vector{profiles[{name,algorithm,vectorizer,compression}],vectorizers[{name,kind,model}]},scoring_profiles[],suggesters[{name,fields[]}],etag,definition}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli aisearch index get srch-contoso-prod/orders --fields key,fields,semantic
```

### aisearch index create

```text
agent-cli aisearch index create — Create an AI Search index from a JSON definition (never replaces one)
 *<index> str             The new index's name, or SERVICE/INDEX
  --service str           The service to create it on; needed when there is more than one
  --definition str        The index definition: JSON, or - to read it from stdin
  --definition-file path  The index definition from a JSON file
Returns: {id,service,name,documents,storage,vector_storage,key,fields[{name,type,attrs[],analyzer,dimensions,profile,fields[{name,type,attrs[],analyzer,dimensions,profile,fields[]}]}],semantic{default,configs[{name,title,content[],keywords[]}]},vector{profiles[{name,algorithm,vectorizer,compression}],vectorizers[{name,kind,model}]},scoring_profiles[],suggesters[{name,fields[]}],etag,definition}
Write: --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli aisearch index create srch-contoso-dev/orders-v2 --definition-file orders.json
```

### aisearch index update

```text
agent-cli aisearch index update — Change an AI Search index's definition (fields added, semantic, scoring, CORS)
 *<index> str             The index: SERVICE/INDEX from index list, or a bare name
  --service str           The service that holds it; needed when more than one does
  --definition str        The whole new definition: JSON, or - to read it from stdin
  --definition-file path  The new definition from a JSON file, as index get --full --output saves it
  --if-etag str           Change it only if its etag is still this (default: the definition's @odata.etag)
  --allow-downtime        Allow adding analyzers, which takes the index offline for a few seconds
Returns: {id,service,name,documents,storage,vector_storage,key,fields[{name,type,attrs[],analyzer,dimensions,profile,fields[{name,type,attrs[],analyzer,dimensions,profile,fields[]}]}],semantic{default,configs[{name,title,content[],keywords[]}]},vector{profiles[{name,algorithm,vectorizer,compression}],vectorizers[{name,kind,model}]},scoring_profiles[],suggesters[{name,fields[]}],etag,definition}
Write: --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli aisearch index update srch-contoso-prod/orders --definition-file orders.json
```

### aisearch index delete

```text
agent-cli aisearch index delete — Delete an AI Search index and every document in it
 *<index> str    The index: SERVICE/INDEX from index list, or a bare name (not an alias)
  --service str  The service that holds it; needed when more than one does
Returns: {id,deleted}
Destructive: needs --yes; --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli aisearch index delete srch-contoso-dev/orders-v2 --yes
```

### aisearch document list

```text
agent-cli aisearch document list — Query an AI Search index: keyword, vector, hybrid or semantic search, filters
 *<index> str                   The index: SERVICE/INDEX from index list, a bare name or an alias
  <text> str                    What to search for (default: every document)
  --service str                 The service that holds the index; needed when more than one does
  --mode keyword|vector|hybrid  keyword (BM25), vector (TEXT embedded by the field's vectorizer), or hybrid (both, fused by RRF) (default keyword)
  --semantic                    Rerank keyword or hybrid results with the semantic ranker (top 50 only)
  --semantic-config str         The semantic configuration (default: the index's)
  --lucene                      Full Lucene syntax (fields, fuzzy, regex); off, ? : / - " in TEXT stay plain text
  --vector-field str[]          Vector fields to query (default: every one with a vectorizer)
  --filter str                  An OData filter, such as "status eq 'failed' and total gt 100" (field names are case-sensitive)
  --select str                  Fields to return, comma-separated (default: every retrievable one)
  --orderby str                 Sort, such as "created desc" (default: by score)
  --skip int                    Results to skip, for the next page (at most 100000) (default 0)
  --limit int                   Most rows to return (at most 1000) (default 50)
Returns: [{id,score,reranker,caption,highlights,doc}]
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli aisearch document list srch-contoso-prod/orders "late delivery" --filter "status eq 'failed'" --fields id,score,doc
```

### aisearch document get

```text
agent-cli aisearch document get — Look up AI Search documents by key: is one in the index, and what it holds
 *<document> str[]  Documents: SERVICE/INDEX/KEY from document list, a KEY with --index, or its URL
  --index str       The index, for bare keys: SERVICE/INDEX or a name
  --service str     The service that holds the index; needed when more than one does
  --select str      Fields to return, comma-separated (default: every retrievable one)
  --vectors         Print vector fields whole rather than as "[N floats]"
Returns: {id,doc}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli aisearch document get srch-contoso-prod/orders/88123
```

### aisearch document create

```text
agent-cli aisearch document create — Upload documents to an AI Search index (insert, or replace whole by key)
 *<index> str       The index: SERVICE/INDEX from index list, a bare name or an alias
  --service str     The service that holds the index; needed when more than one does
  --docs str        Documents as JSON (an array, one object, or JSON lines; at most 1000), or - to read stdin
  --docs-file path  The documents from a JSON or JSON-lines file
Returns: [{id,key,status,error}]
Write: --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli aisearch document create srch-contoso-dev/orders --docs-file orders.jsonl --fields id,status
```

### aisearch document update

```text
agent-cli aisearch document update — Change fields of AI Search documents by key (merge; --upsert adds missing ones)
 *<index> str       The index: SERVICE/INDEX from index list, a bare name or an alias
  --service str     The service that holds the index; needed when more than one does
  --docs str        The key and the fields to change, as JSON (an array, one object, or JSON lines), or - to read stdin
  --docs-file path  The documents from a JSON or JSON-lines file
  --upsert          Upload a document that is not there yet, rather than fail it (404)
Returns: [{id,key,status,error}]
Write: --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli aisearch document update srch-contoso-dev/orders --docs '{"id": "88123", "status": "fixed"}' --fields id,status
```

### aisearch document delete

```text
agent-cli aisearch document delete — Delete AI Search documents by key from one index
 *<document> str[]  Documents of one index: SERVICE/INDEX/KEY from document list, or KEYs with --index
  --index str       The index, for bare keys: SERVICE/INDEX or a name
  --service str     The service that holds the index; needed when more than one does
Returns: [{id,key,status,error}]
Destructive: needs --yes; --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli aisearch document delete srch-contoso-dev/orders/88123 --yes --fields id,status
```

### aisearch indexer list

```text
agent-cli aisearch indexer list — List AI Search indexers with their last run: status, items processed and failed
  --service str[]  Search services to read (default: every one in reach)
  --failing        Only indexers whose last run failed or had failed items, or whose health is error
  --limit int      (default 50)
Returns: [{id,index,source,skillset,schedule,disabled,health,last_status,last_start,last_end,processed,failed}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli aisearch indexer list --failing --fields id,last_status,failed
```

### aisearch indexer get

```text
agent-cli aisearch indexer get — Show an AI Search indexer's last run, its failed items' errors, and history
 *<indexer> str  The indexer: SERVICE/INDEXER from indexer list, a bare name, its URL or portal link
  --service str  The service that holds it; needed when more than one does
Returns: {id,index,source{name,type,container,query},skillset,schedule,disabled,health,last{status,start,end,processed,failed,error},errors[{key,message,name,status,details}],warnings[{key,message,name}],history[{status,start,end,processed,failed,error}]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli aisearch indexer get srch-contoso-prod/orders-sql --fields last,errors
```

### aisearch indexer run

```text
agent-cli aisearch indexer run — Run an AI Search indexer now (--reset to re-read everything); it runs on its own
 *<indexer> str  The indexer: SERVICE/INDEXER from indexer list, or a bare name
  --service str  The service that holds it; needed when more than one does
  --reset        Reset it first: forget what it has read, so it reads everything again and re-runs every skill (which costs)
Returns: {id,reset,requested}
Write: --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli aisearch indexer run srch-contoso-prod/orders-sql
```

### aisearch indexer wait

```text
agent-cli aisearch indexer wait — Wait for an AI Search indexer run to end: exit 0 succeeded, 1 failed, 124 going
 *<indexer> str  The indexer: SERVICE/INDEXER from indexer list, or a bare name
  --service str  The service that holds it; needed when more than one does
  --since time   Wait for a run that started at or after this (indexer run prints it); default: the current one
A time is 15m, 2h, 7d, 1w (ago), now-15m, 2026-09-29, or RFC 3339.
Returns: {id,index,source{name,type,container,query},skillset,schedule,disabled,health,last{status,start,end,processed,failed,error},errors[{key,message,name,status,details}],warnings[{key,message,name}],history[{status,start,end,processed,failed,error}]}
Read. * required. Globals: --fields --raw --timeout (default 100s) --output
e.g. agent-cli aisearch indexer wait srch-contoso-prod/orders-sql --since 2026-09-29T00:39:00Z
```
