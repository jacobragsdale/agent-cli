# k8s in the contoso world

What the recordings in `kubectl.json` (the fake kubectl) say, consistent with every other
domain's facts. Keep a new recording consistent with these, and add a line
when you add a fact.

- Scope `prod` (context `aks-contoso-prod`, namespace `web`). Deployment `api` runs `contosoacr.azurecr.io/api:v1.4.2`, 3/3 ready. Deployment `worker` (`worker:v1.4.2`) is 0/1: pod `worker-5c4d3e9f1-q8zt1` is in CrashLoopBackOff with 23 restarts, BackOff events, and a previous log saying the database password was refused. Configmap `api-config`; secrets `api-env`, `worker-db`; SecretProviderClasses `api-kv` and `worker-kv`.
