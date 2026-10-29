# What the frameworks serve

Recorded with curl on 2026-10-28 against `scripts/openapi-up.sh`, every app at
its framework's default paths. The specs as served are in `specs/`. "Paths
that 404" are the five common guesses
`/openapi.json /swagger.json /swagger/v1/swagger.json /v3/api-docs /openapi/v1.json`;
each app answers only its own one with 200 and the other four with 404, so
none of them is a false positive anywhere (the per-app line says which).
The http domain's well-known paths (`crates/http/src/openapi/find.rs`) come
from this file, and `crates/cli/tests/live_http.rs` holds them to it.

| app | port | spec | declares | Content-Type | HEAD | docs UI | `GET /` |
|---|---|---|---|---|---|---|---|
| fastapi 0.162.0 | 58080 | `/openapi.json` | `openapi: 3.1.0` | `application/json` | 200 | `/docs`, `/redoc` | 404 JSON |
| aspnet-swashbuckle (.NET 8, Swashbuckle 6.9.0) | 58082 | `/swagger/v1/swagger.json`, `.yaml` | `openapi: 3.0.1` | `application/json;charset=utf-8` | **404** | `/swagger` → 301 `/swagger/index.html` | 404, empty |
| aspnet-openapi (.NET 9, Microsoft.AspNetCore.OpenApi 9.0.22, Scalar 2.18.5) | 58083 | `/openapi/v1.json` | `openapi: 3.0.4` | `application/json;charset=utf-8` | **405** | `/scalar` → 302 `/scalar/`, `/scalar/v1` | 404, empty |
| springdoc (Boot 3.5.16, springdoc 2.8.17) | 58084 | `/v3/api-docs`, `/v3/api-docs/{group}`, `.yaml` | `openapi: 3.1.0` | `application/json` | 200 | `/swagger-ui.html` → 302 `/swagger-ui/index.html` | 404 JSON |
| nestjs (Nest 11.2.8, @nestjs/swagger 11.4.7) | 58085 | `/api-json`, `/api-yaml` | `openapi: 3.0.0` | `application/json; charset=utf-8` | 200 | `/api` and `/api/` | 404 JSON |
| go-swag (gin 1.12.0, swag 1.16.6, gin-swagger 1.6.1) | 58086 | `/swagger/doc.json` | `swagger: "2.0"` | `application/json; charset=utf-8` | **404** | `/swagger/index.html` only | 404 text |
| drf-spectacular (Django 5.2.18, DRF 3.18.3, 0.30.0) | 58087 | `/api/schema/` (**YAML by default**) | `openapi: 3.0.3` | `application/vnd.oai.openapi; charset=utf-8` | 200 | `/api/schema/swagger-ui/`, `/api/schema/redoc/` | **200 JSON** (DRF API root) |
| quarkus 3.40.1 (smallrye-openapi) | 58088 | `/q/openapi` (**YAML by default**) | `openapi: 3.1.0` | `application/yaml;charset=UTF-8` | 200 | `/q/swagger-ui` → 302 `/q/swagger-ui/` | 404 HTML |

## fastapi (58080; fastapi-v2 on 58081 is the same)

1. Spec `GET /openapi.json`: 200, `Content-Type: application/json`,
   `content-length: 8588`, `"openapi": "3.1.0"`, `info: {title: Contoso Orders
   API, version: 1.4.2}` (2.0.0 on 58081). No YAML: `/openapi.yaml` is 404.
2. Docs: `/docs` (Swagger UI, 200 `text/html; charset=utf-8`) and `/redoc`
   (ReDoc). Both name the spec inline and load the UI from jsdelivr:
       <script src="https://cdn.jsdelivr.net/npm/swagger-ui-dist@5/swagger-ui-bundle.js"></script>
           const ui = SwaggerUIBundle({
               url: '/openapi.json',
   and
           <redoc spec-url="/openapi.json"></redoc>
3. `GET /`: 404 `application/json` `{"detail":"Not Found"}`.
4. `/openapi.json` 200; `/swagger.json`, `/swagger/v1/swagger.json`,
   `/v3/api-docs`, `/openapi/v1.json` 404.
5. HEAD `/openapi.json`: 200 `application/json`. (HEAD on the app's own GET
   routes, `/healthz` say, is 405; FastAPI adds HEAD to its docs routes only.)
6. `servers`: absent.

## aspnet-swashbuckle (58082)

1. Spec `GET /swagger/v1/swagger.json`: 200,
   `Content-Type: application/json;charset=utf-8`, chunked (no
   Content-Length), `"openapi": "3.0.1"`, `info: {title: Orders, version:
   1.0}` (the title is the assembly name). YAML at `/swagger/v1/swagger.yaml`:
   200 `text/yaml;charset=utf-8`. An `Accept: application/yaml` on the .json
   URL still gets JSON.
2. Docs: `/swagger` and `/swagger/` → 301 `/swagger/index.html` (absolute
   Location `http://127.0.0.1:58082/swagger/index.html`). index.html has no
   URL; it loads `index.js`:
           <script src="index.js" charset="utf-8"></script>
   and `/swagger/index.js` holds the config, its URL relative to the page:
           var configObject = JSON.parse('{"urls":[{"url":"v1/swagger.json","name":"Orders v1"}],"deepLinking":false,...
           configObject.urls.forEach(function (item) {
               if (item.url.startsWith("http") || item.url.startsWith("/")) return;
               item.url = window.location.href.replace("index.html", item.url).split('#')[0];
   so `v1/swagger.json` resolves against `/swagger/index.html` to
   `/swagger/v1/swagger.json`.
3. `GET /`: 404, no body, no Content-Type.
4. `/swagger/v1/swagger.json` 200; the other four 404.
5. HEAD on the spec: **404**.
6. `servers`: absent.

Quirk: `[Required]` on positional record parameters does not reach the spec;
no schema has a `required` list.

## aspnet-openapi (58083)

1. Spec `GET /openapi/v1.json`: 200, `Content-Type: application/json;charset=utf-8`,
   chunked, `"openapi": "3.0.4"`, `info: {title: "Orders | v1", version: 1.0.0}`.
   No YAML route by default.
2. Docs: `/scalar` → 302 `/scalar/` (absolute Location). `/scalar/{name}`
   answers for any name (`/scalar/v1`, `/scalar/v2`: 200 `text/html`) and
   names `openapi/{name}.json`, which for v2 is 404. The page:
               import { initialize } from './scalar.aspnetcore.js'
               initialize(
               '%2Fscalar%2F',
               false,
               {"favicon":"favicon.svg","_integration":"dotnet","sources":[{"title":"v1","url":"openapi/v1.json"}]},
   The URL is relative, but not to the page: `scalar.aspnetcore.js` strips
   the first argument (the route, `/scalar/` or `/scalar/v1`) from
   `location.pathname` and resolves against what is left:
         const basePath = getBasePath(path)
           url: new URL(source.url, `${window.location.origin}${basePath}/`).toString(),
   so it is `/openapi/v1.json`; `/scalar/openapi/v1.json` is 404.
3. `GET /`: 404, no body.
4. `/openapi/v1.json` 200; the other four 404.
5. HEAD on the spec: **405**.
6. `servers`: `[{"url": "http://127.0.0.1:58083/"}]`, built from the request:
   with `Host: api.contoso.test` it is `http://api.contoso.test/` (trailing
   slash; the scheme stays http).

Quirk: every record property is in `required`, `note` (a `string?`) too.

## springdoc (58084)

1. Spec `GET /v3/api-docs`: 200, `Content-Type: application/json`,
   `Content-Length: 1874`, `"openapi": "3.1.0"`, `info: {title: "OpenAPI
   definition", version: v0}` (springdoc's placeholders). With the two
   groups configured it still serves everything (`/orders`, `/orders/{id}`,
   `/admin/reset`); each group is at `/v3/api-docs/orders` and
   `/v3/api-docs/admin`. YAML: `/v3/api-docs.yaml` and
   `/v3/api-docs.yaml/{group}`, 200 `application/vnd.oai.openapi`;
   `/v3/api-docs/orders.yaml` is 404.
2. Docs: `/swagger-ui.html` → 302 `/swagger-ui/index.html` (absolute
   Location). index.html loads `./swagger-initializer.js`, which still
   carries swagger-ui's **petstore URL** beside the real `configUrl`:
         window.ui = SwaggerUIBundle({
           url: "https://petstore.swagger.io/v2/swagger.json",
       ...
         "configUrl" : "/v3/api-docs/swagger-config",
         "validatorUrl" : ""
   Swagger UI lets `configUrl` win. `/v3/api-docs/swagger-config` (200
   `application/json`) lists the groups, not `/v3/api-docs`:
       {"configUrl":"/v3/api-docs/swagger-config","oauth2RedirectUrl":"http://127.0.0.1:58084/swagger-ui/oauth2-redirect.html","urls":[{"url":"/v3/api-docs/admin","name":"admin"},{"url":"/v3/api-docs/orders","name":"orders"}],"validatorUrl":""}
3. `GET /`: 404 `application/json`
   `{"timestamp":"…","status":404,"error":"Not Found","path":"/"}`.
4. `/v3/api-docs` 200; the other four 404.
5. HEAD on the spec: 200 `application/json`.
6. `servers`: `[{"url": "http://127.0.0.1:58084", "description": "Generated
   server url"}]`, from the request: `Host: api.contoso.test` with
   `X-Forwarded-Proto: https` gives `http://api.contoso.test` (forwarded
   headers are not honoured by default).

## nestjs (58085)

1. Spec `GET /api-json`: 200, `Content-Type: application/json; charset=utf-8`,
   `Content-Length: 1705`, an `ETag`, `"openapi": "3.0.0"`, `info: {title:
   Contoso Orders API, description: "", version: "1.0", contact: {}}`. YAML
   `/api-yaml`: 200 `text/yaml; charset=utf-8`.
2. Docs: `/api` and `/api/`, 200 `text/html; charset=utf-8`. The page names
   no spec URL; the spec is **inline** in `swagger-ui-init.js` as
   `swaggerDoc`, and the only `url` is the page's origin:
         let url = window.location.search.match(/url=([^&]+)/);
           url = window.location.origin;
         let options = {
         "swaggerDoc": {
           "openapi": "3.0.0",
   The script path depends on the trailing slash: `/api` has
   `<script src="./api/swagger-ui-init.js">`, `/api/` has
   `<script src="./swagger-ui-init.js">`; both resolve to
   `/api/swagger-ui-init.js` (200 `application/javascript`).
3. `GET /`: 404 `application/json`
   `{"message":"Cannot GET /","error":"Not Found","statusCode":404}`.
4. All five 404 (its spec is `/api-json`).
5. HEAD on the spec: 200.
6. `servers`: `[]` (present, empty).

## go-swag (58086)

1. Spec `GET /swagger/doc.json`: 200, `Content-Type: application/json;
   charset=utf-8`, chunked, `"swagger": "2.0"`, `info: {description: "",
   title: Contoso Orders API, contact: {}, version: "1.0"}`. No YAML route
   (`/swagger/doc.yaml`, `/swagger/swagger.yaml` 404). Definitions are named
   after the Go package: `main.Order`, `main.OrderIn`.
2. Docs: `/swagger/index.html` only. `/swagger` → 301 `/swagger/`, and
   `/swagger/` is **404**. index.html loads `./swagger-initializer.js`, which
   names the spec relative to the page:
       <script src="./swagger-initializer.js"> </script>
   and
           url: "doc.json",
3. `GET /`: 404 `text/plain` `404 page not found`.
4. All five 404.
5. HEAD on the spec: **404** (gin routes GET only).
6. No `servers` (Swagger 2.0): `"host": ""`, `"basePath": ""`, `"schemes": []`,
   all present and empty.

## drf-spectacular (58087)

1. Spec `GET /api/schema/`: 200, **YAML** by default, `Content-Type:
   application/vnd.oai.openapi; charset=utf-8`, `openapi: 3.0.3`, `info:
   {title: Contoso Orders API, version: 1.0.0}`. JSON by `?format=json` or
   `Accept: application/json` (Content-Type `application/vnd.oai.openapi+json`
   and `application/json` respectively); `Accept: */*` gets YAML. The spec
   lists its own `/api/schema/` as an operation, and DRF's trailing slashes:
   `/orders/`, `/orders/{id}/`.
2. Docs: `/api/schema/swagger-ui/` and `/api/schema/redoc/`, 200
   `text/html; charset=utf-8`, both from jsdelivr `@latest`:
       const ui = SwaggerUIBundle({
         url: "/api/schema/",
   and
           <redoc spec-url="/api/schema/"></redoc>
3. `GET /`: **200** `application/json` `{"orders":"http://127.0.0.1:58087/orders/"}`
   (DefaultRouter's API root; `text/html` with `Accept: text/html`). `/orders`
   and `/orders/1` → 301 to the slashed path.
4. All five 404.
5. HEAD on the spec: 200 (with `?format=json`, `application/vnd.oai.openapi+json`).
6. `servers`: absent.

## quarkus (58088)

1. Spec `GET /q/openapi`: 200, **YAML** by default, `Content-Type:
   application/yaml;charset=UTF-8`, `openapi: 3.1.0`, `info: {title: orders
   API, version: 1.0.0}` (from the artifactId and version). JSON by
   `?format=json`, `/q/openapi.json` or `Accept: application/json`;
   `Accept: application/vnd.oai.openapi+json` gets YAML. `/q/openapi.yaml`
   and `?format=yaml` 200. Nothing at `/openapi`.
2. Docs: `/q/swagger-ui` → 302 `/q/swagger-ui/` (absolute Location); the
   page names the YAML URL inline:
                       var ui = SwaggerUIBundle({
                                   url: '/q/openapi',
3. `GET /`: 404 `text/html; charset=utf-8`
   `<html><body><h1>Resource not found</h1></body></html>`.
4. All five 404.
5. HEAD on the spec: 200.
6. `servers`: absent.

## The rest of the harness

- TLS (curl, OpenSSL 3.5, `--cacert $OPENAPI_CA`): 58443 200; 58444 400
  without a client cert, 200 with one; 58445 `certificate has expired`; 58446
  `self-signed certificate`; 58447 `no alternative certificate subject name
  matches target hostname 'localhost'`; 58449 200 (nginx sends leaf +
  intermediate). **58448 is 200 under OpenSSL**: the leaf is itself a
  trusted root, which OpenSSL accepts; webpki (rustls) refuses a CA:TRUE
  end-entity, so expect `CaUsedAsEndEntity` there, and a pass under curl.
- Through `$OPENAPI_PROXY` (with `-U agent:proxy-pass`; 407 without):
  `http://echo:8000/echo` sees `via: 1.1 <container id> (squid/6.14)` and
  `x-forwarded-for`; CONNECT to `api.contoso.test:58443` and `:58444` (with
  the client cert) work; CONNECT to any other port is refused. The host
  cannot resolve `fastapi`. `$OPENAPI_PROXY_OPEN` the same without auth.
- `$OPENAPI_PROXY_NEGOTIATE`: 407 with `Proxy-Authenticate: Negotiate` and
  `Proxy-Authenticate: NTLM` for GET and CONNECT alike, then closes.
- `/fail/503`'s `traceparent` is
  `00-0000000000000000199e6ab3c4d5e6f7-00f067aa0ba902b7-01`: trace id
  `0000000000000000199e6ab3c4d5e6f7`, its low 64 bits `199e6ab3c4d5e6f7` =
  `1846030217602524919`, span id `00f067aa0ba902b7`.
- curl `-L` drops `Authorization` on `/redirect/external` (another port is
  another origin), which `/echo` on 58089 shows.
