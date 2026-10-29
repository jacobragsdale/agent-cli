# http — REST calls, OpenAPI specs, TLS (13 commands)

| Command | Effect | Summary |
|---|---|---|
| [`http request get`](#http-request-get) | read | GET a URL, as curl does: status, headers, the body bounded; a 4xx or 5xx fails |
| [`http request run`](#http-request-run) | read or write | Send a request of any method to a URL, with a body, as curl -X does |
| [`http request diff`](#http-request-diff) | read | Compare two GETs (staging's URL and prod's): status, headers and the JSON body |
| [`http request wait`](#http-request-wait) | read | Poll a URL until it answers the status and the JSON fields wanted |
| [`http spec get`](#http-spec-get) | read | Find an API's OpenAPI spec from its base URL or docs page, and summarise it |
| [`http spec list`](#http-spec-list) | read | List the OpenAPI specs an API publishes: its versions and groups |
| [`http spec diff`](#http-spec-diff) | read | What changed between OpenAPI specs, and which changes break their clients |
| [`http operation list`](#http-operation-list) | read | List a spec's operations (endpoints): by words, tag, path, a parameter taken |
| [`http operation get`](#http-operation-get) | read | Show an operation (endpoint): its parameters, body fields, answers, an example |
| [`http schema list`](#http-schema-list) | read | List a spec's schemas (models, DTOs), or those with a field so named |
| [`http schema get`](#http-schema-get) | read | Show a spec's schema (model) as field paths: types, required, enums, users |
| [`http certificate get`](#http-certificate-get) | read | Show the TLS certificate a server presents: names, expiry, chain, trusted here |
| [`http certificate list`](#http-certificate-list) | read | Probe several hosts' TLS certificates, soonest expiry first, or those due soon |

### http request get

```text
agent-cli http request get — GET a URL, as curl does: status, headers, the body bounded; a 4xx or 5xx fails
 *<url> str           The URL: https://HOST/PATH?QUERY
  --head              HEAD instead of GET: the status and headers only
  --follow            Follow redirects; the sign-in stays on the first origin
  --limit int         Items kept per JSON array (default 50)
  --query str[]       A query parameter K=V (repeatable)
  --header str[]      A header 'K: V' (repeatable)
  --header-env str[]  A header from a variable, K=VAR: X-Api-Key=API_KEY
  --bearer-env str    Bearer token from this variable
  --az str            Bearer token from the az login for this resource (api://APP-ID)
  --basic str         Basic sign-in as USER:VAR, the password from variable VAR
  --cacert path       Trust these CAs (PEM) instead of the system store
  --cert path[]       mTLS: a PEM with the client certificate and its key, or CERT,KEY
  --insecure          Accept any server certificate
Returns: {method,url,status,reason,ms,proxy,headers,trace_id,content_type,size,hops[{status,url,location,unsigned}],spec_problems[],body,saved}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli http request get https://api.contoso.example/orders/42 --bearer-env API_TOKEN
```

### http request run

```text
agent-cli http request run — Send a request of any method to a URL, with a body, as curl -X does
 *<method> str        GET, HEAD, OPTIONS, POST, PUT, PATCH or DELETE
 *<url> str           The URL: https://HOST/PATH?QUERY
  --body str          The body: JSON, or - to read it from stdin
  --body-file path    The body from a file
  --content-type str  The body's type (default application/json)
  --form str[]        A form field K=V (repeatable), urlencoded
  --reads             The POST only reads (a search): no --dry-run stop
  --follow            Follow redirects; the sign-in stays on the first origin
  --spec str          Check the call against this OpenAPI spec first
  --limit int         Items kept per JSON array (default 50)
  --query str[]       A query parameter K=V (repeatable)
  --header str[]      A header 'K: V' (repeatable)
  --header-env str[]  A header from a variable, K=VAR: X-Api-Key=API_KEY
  --bearer-env str    Bearer token from this variable
  --az str            Bearer token from the az login for this resource (api://APP-ID)
  --basic str         Basic sign-in as USER:VAR, the password from variable VAR
  --cacert path       Trust these CAs (PEM) instead of the system store
  --cert path[]       mTLS: a PEM with the client certificate and its key, or CERT,KEY
  --insecure          Accept any server certificate
Returns: {method,url,status,reason,ms,proxy,headers,trace_id,content_type,size,hops[{status,url,location,unsigned}],spec_problems[],body,saved}
Read or write, decided by the input: a write honours --dry-run and may need --yes. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli http request run POST https://api.contoso.example/orders --body '{"customer_id": 7}' --bearer-env API_TOKEN
```

### http request diff

```text
agent-cli http request diff — Compare two GETs (staging's URL and prod's): status, headers and the JSON body
 *<left> str          The left URL (staging's)
 *<right> str         The right URL (prod's)
  --as str[]          Treat a word on the left as one on the right, A=B (repeatable); none turns off the names inferred from the two ids
  --ignore str[]      Leave out a path as the changes print it, and all under it (repeatable; * is one segment)
  --all               Keep what this resource's diff leaves out as noise (ids, times, counts)
  --limit int         Most differences to print (default 50)
  --exact             Texts compare as they are: whitespace, case and comments count
  --query str[]       A query parameter K=V (repeatable)
  --header str[]      A header 'K: V' (repeatable)
  --header-env str[]  A header from a variable, K=VAR: X-Api-Key=API_KEY
  --bearer-env str    Bearer token from this variable
  --az str            Bearer token from the az login for this resource (api://APP-ID)
  --basic str         Basic sign-in as USER:VAR, the password from variable VAR
  --cacert path       Trust these CAs (PEM) instead of the system store
  --cert path[]       mTLS: a PEM with the client certificate and its key, or CERT,KEY
  --insecure          Accept any server certificate
Returns: {left,right,same,as[],changes[{path,left,right,redacted,same,lines[],breaking}]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli http request diff https://api-staging.contoso.example/orders/42 https://api.contoso.example/orders/42
```

### http request wait

```text
agent-cli http request wait — Poll a URL until it answers the status and the JSON fields wanted
 *<url> str             The URL to GET until it answers as wanted
  --expect-status int   The status to wait for (default: any 2xx)
  --expect-field str[]  A JSON field's value to wait for, FIELD=VALUE (repeatable): version=1.4.2
  --every int           Seconds between tries (default 2)
  --limit int           Items kept per JSON array (default 50)
  --query str[]         A query parameter K=V (repeatable)
  --header str[]        A header 'K: V' (repeatable)
  --header-env str[]    A header from a variable, K=VAR: X-Api-Key=API_KEY
  --bearer-env str      Bearer token from this variable
  --az str              Bearer token from the az login for this resource (api://APP-ID)
  --basic str           Basic sign-in as USER:VAR, the password from variable VAR
  --cacert path         Trust these CAs (PEM) instead of the system store
  --cert path[]         mTLS: a PEM with the client certificate and its key, or CERT,KEY
  --insecure            Accept any server certificate
Returns: {tries,waited_s,method,url,status,reason,ms,proxy,headers,trace_id,content_type,size,hops[{status,url,location,unsigned}],spec_problems[],body,saved}
Read. * required. Globals: --fields --raw --timeout (default 100s) --output
e.g. agent-cli http request wait https://api.contoso.example/healthz --expect-field version=1.4.2
```

### http spec get

```text
agent-cli http spec get — Find an API's OpenAPI spec from its base URL or docs page, and summarise it
 *<spec> str          The spec: its URL, its docs page, the API's base URL, or a file
  --header str[]      A header 'K: V' (repeatable)
  --header-env str[]  A header from a variable, K=VAR: X-Api-Key=API_KEY
  --bearer-env str    Bearer token from this variable
  --az str            Bearer token from the az login for this resource (api://APP-ID)
  --basic str         Basic sign-in as USER:VAR, the password from variable VAR
  --cacert path       Trust these CAs (PEM) instead of the system store
  --cert path[]       mTLS: a PEM with the client certificate and its key, or CERT,KEY
  --insecure          Accept any server certificate
Returns: {id,found,framework,docs,fetches,format,title,version,description,servers[],base,sign_in[{name,type,detail}],security[],operations,schemas,tags[{name,operations}],others[{name,url}]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli http spec get https://api.contoso.example
```

### http spec list

```text
agent-cli http spec list — List the OpenAPI specs an API publishes: its versions and groups
 *<url> str           The API's base URL, its docs page, or one of its specs
  --limit int         Most specs to read (default 50)
  --header str[]      A header 'K: V' (repeatable)
  --header-env str[]  A header from a variable, K=VAR: X-Api-Key=API_KEY
  --bearer-env str    Bearer token from this variable
  --az str            Bearer token from the az login for this resource (api://APP-ID)
  --basic str         Basic sign-in as USER:VAR, the password from variable VAR
  --cacert path       Trust these CAs (PEM) instead of the system store
  --cert path[]       mTLS: a PEM with the client certificate and its key, or CERT,KEY
  --insecure          Accept any server certificate
Returns: [{id,name,format,title,version,operations,problem}]
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli http spec list https://api.contoso.example --fields id,name,version,operations
```

### http spec diff

```text
agent-cli http spec diff — What changed between OpenAPI specs, and which changes break their clients
 *<left> str          The left spec, which clients were written against: a URL or a file
 *<right> str         The right spec, the one replacing it
  --breaking          Only the changes that break a client of the left spec
  --as str[]          Treat a word on the left as one on the right, A=B (repeatable); none turns off the names inferred from the two ids
  --ignore str[]      Leave out a path as the changes print it, and all under it (repeatable; * is one segment)
  --all               Keep what this resource's diff leaves out as noise (ids, times, counts)
  --limit int         Most differences to print (default 50)
  --exact             Texts compare as they are: whitespace, case and comments count
  --header str[]      A header 'K: V' (repeatable)
  --header-env str[]  A header from a variable, K=VAR: X-Api-Key=API_KEY
  --bearer-env str    Bearer token from this variable
  --az str            Bearer token from the az login for this resource (api://APP-ID)
  --basic str         Basic sign-in as USER:VAR, the password from variable VAR
  --cacert path       Trust these CAs (PEM) instead of the system store
  --cert path[]       mTLS: a PEM with the client certificate and its key, or CERT,KEY
  --insecure          Accept any server certificate
Returns: {left,right,same,as[],changes[{path,left,right,redacted,same,lines[],breaking}]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli http spec diff https://api-staging.contoso.example/openapi.json ./openapi.yaml --breaking
```

### http operation list

```text
agent-cli http operation list — List a spec's operations (endpoints): by words, tag, path, a parameter taken
 *<spec> str          The spec: its URL, its docs page, the API's base URL, or a file
  <words> str[]       Words its method, path, summary, id or tags hold
  --tag str           Only operations with this tag
  --method str        Only this method: GET, POST…
  --path str          Only paths matching this, * for any run: /orders/*
  --param str         Only operations taking a parameter or body field so named
  --uses str          Only operations using this schema (component)
  --deprecated        Only deprecated operations
  --anonymous         Only operations that need no sign-in
  --limit int         Most rows to return (default 50)
  --header str[]      A header 'K: V' (repeatable)
  --header-env str[]  A header from a variable, K=VAR: X-Api-Key=API_KEY
  --bearer-env str    Bearer token from this variable
  --az str            Bearer token from the az login for this resource (api://APP-ID)
  --basic str         Basic sign-in as USER:VAR, the password from variable VAR
  --cacert path       Trust these CAs (PEM) instead of the system store
  --cert path[]       mTLS: a PEM with the client certificate and its key, or CERT,KEY
  --insecure          Accept any server certificate
Returns: [{id,method,path,summary,operation_id,tags[],deprecated,security[]}]
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli http operation list https://api.contoso.example --param customer_id --fields id,summary
```

### http operation get

```text
agent-cli http operation get — Show an operation (endpoint): its parameters, body fields, answers, an example
 *<operation> str     SPEC#METHOD PATH (or SPEC#operationId), as operation list prints it
  --spec str          The spec, when the operation is given without it
  --header str[]      A header 'K: V' (repeatable)
  --header-env str[]  A header from a variable, K=VAR: X-Api-Key=API_KEY
  --bearer-env str    Bearer token from this variable
  --az str            Bearer token from the az login for this resource (api://APP-ID)
  --basic str         Basic sign-in as USER:VAR, the password from variable VAR
  --cacert path       Trust these CAs (PEM) instead of the system store
  --cert path[]       mTLS: a PEM with the client certificate and its key, or CERT,KEY
  --insecure          Accept any server certificate
Returns: {id,method,path,url,summary,description,operation_id,tags[],deprecated,security[],parameters[{name,in,required,path,type,format,nullable,enum[],default,limits[],schema,description,read_only,deprecated}],body{required,content_types[],fields[{path,type,format,required,nullable,enum[],default,limits[],schema,description,read_only,deprecated}],example},responses[{status,description,content_types[],fields[{path,type,format,required,nullable,enum[],default,limits[],schema,description,read_only,deprecated}]}]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli http operation get 'https://api.contoso.example/openapi.json#POST /orders'
```

### http schema list

```text
agent-cli http schema list — List a spec's schemas (models, DTOs), or those with a field so named
 *<spec> str          The spec: its URL, its docs page, the API's base URL, or a file
  <words> str[]       Words its name or description holds
  --field str         Only schemas with a field so named: email
  --limit int         Most rows to return (default 50)
  --header str[]      A header 'K: V' (repeatable)
  --header-env str[]  A header from a variable, K=VAR: X-Api-Key=API_KEY
  --bearer-env str    Bearer token from this variable
  --az str            Bearer token from the az login for this resource (api://APP-ID)
  --basic str         Basic sign-in as USER:VAR, the password from variable VAR
  --cacert path       Trust these CAs (PEM) instead of the system store
  --cert path[]       mTLS: a PEM with the client certificate and its key, or CERT,KEY
  --insecure          Accept any server certificate
Returns: [{id,name,type,fields,used_by,description}]
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli http schema list https://api.contoso.example --field email --fields id,fields,used_by
```

### http schema get

```text
agent-cli http schema get — Show a spec's schema (model) as field paths: types, required, enums, users
 *<schema> str        SPEC#Name, as schema list prints it
  --spec str          The spec, when the schema is given by name alone
  --header str[]      A header 'K: V' (repeatable)
  --header-env str[]  A header from a variable, K=VAR: X-Api-Key=API_KEY
  --bearer-env str    Bearer token from this variable
  --az str            Bearer token from the az login for this resource (api://APP-ID)
  --basic str         Basic sign-in as USER:VAR, the password from variable VAR
  --cacert path       Trust these CAs (PEM) instead of the system store
  --cert path[]       mTLS: a PEM with the client certificate and its key, or CERT,KEY
  --insecure          Accept any server certificate
Returns: {id,name,type,description,fields[{path,type,format,required,nullable,enum[],default,limits[],schema,description,read_only,deprecated}],used_by[]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli http schema get 'https://api.contoso.example/openapi.json#Order'
```

### http certificate get

```text
agent-cli http certificate get — Show the TLS certificate a server presents: names, expiry, chain, trusted here
 *<target> str   HOST[:PORT] (443 by default) or a URL
  --cacert path  Trust these CAs (PEM) instead of the system store
Returns: {id,trusted,refusal,version,alpn,connect_ms,handshake_ms,proxy,leaf{subject,issuer,sans[],not_before,not_after,days_left,serial,thumbprint,key,signature,ca},chain[{subject,issuer,sans[],not_before,not_after,days_left,serial,thumbprint,key,signature,ca}]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli http certificate get api.contoso.example
```

### http certificate list

```text
agent-cli http certificate list — Probe several hosts' TLS certificates, soonest expiry first, or those due soon
 *<targets> str[]            Hosts (HOST[:PORT]) or URLs to probe
  --expires-within duration  Only those expiring within this: 30d
  --limit int                Most rows to return (default 50)
  --cacert path              Trust these CAs (PEM) instead of the system store
A duration is 500ms, 30s, 15m, 2h, 7d, 1w.
Returns: [{id,subject,issuer,not_after,days_left,trusted,refusal,thumbprint,problem}]
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli http certificate list api.contoso.example wiki.contoso.example --expires-within 30d --fields id,not_after,days_left
```
