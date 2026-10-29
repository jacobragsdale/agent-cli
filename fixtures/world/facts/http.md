# http facts

The api the cluster runs (`prod/web/api`, `staging/web/api`) is ASP.NET Core
with Swashbuckle, so its spec is at `/swagger/v1/swagger.json` and its docs
page at `/swagger/index.html`, which names the spec in its `index.js`.

- `https://api.contoso.example` (prod, the ingress `api` in `facts/k8s.md`)
  serves 1.4.2 (`GET /healthz`); `https://api-staging.contoso.example`
  serves 1.4.1. Neither answers `/` or `/openapi.json` (404).
- The spec, `Contoso.Orders.Api`, OpenAPI 3.0.1, signs in with `Bearer` (an
  Entra token), `/healthz` excepted: list, create and get orders, and from
  1.4.2 `GET /orders/{id}/fulfilment`. 1.4.2 also made `lines[].sku`
  required in a new order: `http spec diff` of staging against prod names
  that as its one breaking change.
- Order 42 is `open` on staging and `shipped` on prod; nothing else differs.
  Order 9999 is a 404 (problem+json) on both.
- Staging's `POST /orders` answers 201 (order 9001). Prod's answers 503
  `upstream worker unavailable`, with a traceparent whose trace is
  `1846223019175527734`: the trace `facts/dd.md` holds, whose innermost frame
  is `OrderClient.cs` line 19. `dd span list --trace 1846223019175527734`
  for 21:30 to 22:30 on 09-28 is recorded.
- TLS (`tls/`): `api.contoso.example` serves cert-manager's `api-tls`, from
  `Contoso Issuing CA 1`, valid until 2026-11-30T03:00:00Z (61 days left);
  `www.contoso.example` serves Key Vault's `www-contoso-example`, from
  `Fabrikam TLS CA 2025`, expiring 2026-10-08 (8 days), the same thumbprint
  `kv certificate get` prints. `login.microsoftonline.com` is served under
  `Contoso Proxy CA`: the office network intercepts TLS, and doctor says so.
