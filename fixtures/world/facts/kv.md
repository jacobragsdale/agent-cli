# kv in the contoso world

What the recordings in `http/azure.json` say, consistent with every other
domain's facts. Keep a new recording consistent with these, and add a line
when you add a fact.

- Vault `kv-contoso-prod`: `db-password` expires 2026-10-14 (within 30 days), `worker-db-password` expired 2026-09-20 (why the worker cannot log in), `api-key`, `orders-db-conn`.
- `kv-contoso-prod` (`kv vault get`) is in RBAC mode (who may read is `arm role list --scope kv-contoso-prod`), with soft delete (90 days) and purge protection. Public network access is Enabled, which `arm policy list` flags, but its firewall's default action is Deny with no IP or vnet rules (AzureServices bypass): the way in is `pe-kv-contoso-prod`, Approved, whose id `net endpoint get` takes and which the AKS pods resolve (facts/net.md).
- Vault `kv-contoso-billing` (rg-contoso-prod) is in the inventory but not in `[azure] vaults`, so kv's commands leave it out; its private endpoint `pe-kv-contoso-billing` has no DNS zone group, so the pods cannot resolve it (trace 1, facts/net.md).
- Key `orders-cmk` (RSA 2048, encrypt, decrypt, wrapKey, unwrapKey; created 2026-07-15, expires 2027-07-15) rotates 90 days after creation and notifies 30 days before expiry, each version living a year. `kv key list` also shows the two certificates' keys, which the vault manages.
- Certificate `www-contoso-example` (`CN=www.contoso.example`, SANs `www.contoso.example` and `contoso.example`, the name `agw-contoso-prod` serves) expires 2026-10-08, within 14 days. Its issuer is Unknown (imported from an outside CA), so it does not renew itself: its policy only emails contacts 30 days before expiry, and no renewal is pending. `kv certificate get` notes `kv version list kv-contoso-prod/www-contoso-example`, which shows one version, so no renewal has landed. Certificate `internal-contoso-example` (`CN=internal.contoso.example`, self-signed) renews itself at 80% of its life and expires 2027-06-01. Both certificates' backing secrets are in `kv secret list` (`managed`, tag `owner=web`), so `--expires-within 30d` lists `www-contoso-example` too.
- One deleted secret, `orders-api-key-old` (deleted 2026-09-20T10:15:00Z, purged 2026-12-19T10:15:00Z unless recovered): `kv secret list --deleted`.
