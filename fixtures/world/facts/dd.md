# dd in the contoso world

What the recordings in `http/dd.json` say, consistent with every other
domain's facts. Keep a new recording consistent with these, and add a line
when you add a fact.

- Site `datadoghq.eu`, env `prod`. Services `api` and `worker`, tagged `kube_cluster_name:prod`, `kube_namespace:web`, so a row's `pod` is the k8s id (`prod/web/…`).
- Monitor **4711** "[prod] api error rate above 5%" triggered at 21:36 on 09-28, two minutes after api `v1.4.2` rolled out (21:34:40), and recovered at 21:58 (OK now; groups per api pod). Monitor **4712** "[prod] worker crash-looping" has been in Alert since 21:41 for pod `worker-5c4d3e9f1-q8zt1`. No downtimes, no incidents.
- Error logs 21:30–22:30 on 09-28: the worker pod's `password authentication failed for user "worker"` from 21:36:05 on, and api's `POST /orders 503 … upstream worker unavailable` 21:35–21:37 (trace ids match `dd span list --service api --status error` in that window). `trace.http.request.errors{service:api,env:prod}` peaks at 64 at 21:40. The event stream holds the rollout, both triggers and the recovery.
