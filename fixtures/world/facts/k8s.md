# k8s in the contoso world

What the recordings in `kubectl.json` (the fake kubectl) say, consistent with every other
domain's facts. Keep a new recording consistent with these, and add a line
when you add a fact.

- Scope `prod` (context `aks-contoso-prod`, namespace `web`). Deployment `api` runs `contosoacr.azurecr.io/api:v1.4.2`, 3/3 ready. Deployment `worker` (`worker:v1.4.2`) is 0/1: pod `worker-5c4d3e9f1-q8zt1` is in CrashLoopBackOff with 23 restarts, BackOff events, and a previous log saying the database password was refused. Configmap `api-config`; secrets `api-env`, `worker-db`; SecretProviderClasses `api-kv` and `worker-kv`.
- Deployments carry `tags.datadoghq.com/service` (`api`, `worker`), the dd service names. `api`'s rollout finished at 2026-09-28T21:34:40Z (Progressing NewReplicaSetAvailable), so `k8s deployment wait prod/web/api` exits 0 and names `dd service get api --since 2026-09-28T21:34:40Z`. Kubernetes gave up on `worker`'s at 21:44:12 (Progressing=False, ProgressDeadlineExceeded, ten minutes after its pod was made), so `deployment wait prod/web/worker` exits 1.
- Only the worker pod mounts `kv-contoso-prod/worker-db-password` (through `worker-kv`); the api pods mount `db-password` and `api-key` (through `api-kv`).
