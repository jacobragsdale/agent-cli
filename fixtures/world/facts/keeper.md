# keeper in the contoso world

What the recordings in `http/keeper.json` say, consistent with every other
domain's facts. Keep a new recording consistent with these, and add a line
when you add a fact.

- Artifact Keeper 1.11.7 at `https://keeper.contoso.example`; the token signs in as `agent`. Repositories: `agent-cli` (generic, the releases `install.sh` reads), `builds` (generic, pipeline outputs), `internal` (generic, private), `pypi` (hosted PyPI, Contoso's own packages), `pypi-proxy` (a proxy of `https://pypi.org`, keeping answers 86400 s) and `pypi-all` (virtual: `pypi`, then `pypi-proxy`; pip's index-url).
- `builds/api@1.4.2` is api-ci run 8812's upload (ado): `api-1.4.2-linux-x64.tar.gz` and its `.sha256`, by `svc-ado-pipelines` at 2026-09-28T21:30:05Z, four minutes before the api ReplicaSet for `api:v1.4.2` (k8s, 21:34:12Z). `builds/api@1.4.1` (run 8790, 2026-09-15T16:12:40Z) is one tarball.
- `pypi/contoso-orders-client`: 2.3.0 (the client that retries on 429, PR 431's change) published by `svc-ado-pipelines` at 2026-09-28T21:31:10Z, and 2.2.0 at 2026-09-15T16:14:02Z; one wheel each. `pypi/contoso-etl-utils` 0.9.0 (2026-09-10), the DAGs' helpers.
- `pypi-proxy` has cached `pandas` 2.2.2, `pyarrow` 16.1.0 and `requests` 2.32.3 (a wheel each, 2026-08-20), not `pyarrow` 17.0.0, upstream's latest: `package get pypi-proxy/pyarrow` marks 17.0.0 `cached: false`. Asked for `contoso-etl-utils`, the proxy answers 404 (pypi.org has no such project, so `pip install` with pypi-proxy as the index fails); its hint, `package list contoso-etl-utils`, finds it in `pypi`, which `pypi-all` serves.
