# Plan: finish the controlm domain against a real Enterprise Manager

The domain is built and tested against answers shaped by BMC's spec: seven
commands, a doctor, unit tests, labeled search queries, docs. What is left
needs the work network. This is the order to do it in. Each step names what
to run, what to look at and what to change. Delete this plan when every step
is done.

The commands:

| Command | Call |
|---|---|
| `controlm job list` | `GET /run/jobs/status?jobname=&folder=&ctm=&status=&fromTime=…` |
| `controlm job get ID` | `GET /run/job/ID/status`, then `/waitingInfo` when it waits |
| `controlm job logs ID` | `GET /run/job/ID/output[?runNo=N]` (`--execution N`), or `/log` with `--events` |
| `controlm job retry ID` | `POST /run/job/ID/rerun` |
| `controlm job run SERVER/FOLDER/JOB` | `POST /run/order`, then `GET /run/status/RUN_ID` |
| `controlm definition list` | `GET /deploy/jobs?format=json&ctm=&folder=&job=` |
| `controlm definition get SERVER/FOLDER/JOB` | the same, one job, its whole definition |

Every assumption is a `VERIFY(work)` comment beside the code it shapes:
`grep -rn 'VERIFY(work)' crates/controlm`. Rules while doing this: no real
server, host, folder, job or user names in anything committed (use
`contoso`-style names like the tests), and `config.toml` never goes in the
repository.

## 1. Configure, and get past TLS

Add the instance to your own config (`agent-cli config` prints where it is):

```toml
[[controlm.instance]]
name = "prod"
base_url = "https://EM_HOST:8443/automation-api"
username = "YOUR_USER"
password_cmd = "pass show controlm/prod"   # or password_env
read_only = true                           # until the writes are verified
```

Then `cargo build -p agent-cli && target/debug/agent-cli doctor controlm`.

A certificate error (`UnknownIssuer`, `invalid peer certificate`) is
expected. Core trusts only the public roots (ureq with webpki-roots), and an
Enterprise Manager's certificate is self-signed or from the company's CA.
That's a core change for every domain, so make it once:

1. In the root `Cargo.toml`, give ureq the feature:
   `ureq = { version = "3.4.1", features = ["platform-verifier"] }`.
2. In `crates/core/src/http.rs`, where `Https::send` builds the agent, add
   `.tls_config(ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build())`
   to the `config_builder()` chain.
3. Trust the certificate in WSL: export it (in a browser, or
   `openssl s_client -connect EM_HOST:8443 -showcerts </dev/null`), then
   `sudo cp em.crt /usr/local/share/ca-certificates/ && sudo update-ca-certificates`.
4. The commit message says why (`rustls-platform-verifier`: the OS trust
   store, so a company CA works).

Other errors, by what doctor prints:
- **401:** the password.
- **404:** `base_url` lacks `/automation-api`.
- **No answer at all:** the port or a firewall.

## 2. A shell for raw answers

Every check below compares what agent-cli prints with what the API sent.
Get the raw answer with curl (`-k` is fine for looking):

```sh
EM=https://EM_HOST:8443/automation-api
read -rs PW; TOKEN=$(printf '{"username":"%s","password":"%s"}' "$USER" "$PW" \
  | curl -sk -H 'Content-Type: application/json' -d @- "$EM/session/login" \
  | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d["token"]); print(d.get("version"), file=sys.stderr)')
ctm() { curl -sk -H "Authorization: Bearer $TOKEN" "$EM/$1"; }
curl -sk "$EM/yaml" -o /tmp/ctm-spec.yaml    # this version's spec; keep it out of the repo
```

The login prints the Enterprise Manager's version. Note it in the card.

## 3. Reads, one command at a time

For each: run the curl, run the command, fix the code where they disagree,
and change the unit test beside it to the real shape (scrubbed). The
`job()` sample in `src/testing.rs` becomes a real `run/jobs/status` row.

1. **`job list`**
   - `ctm 'run/jobs/status?limit=3'`, then `agent-cli controlm job list --limit 3`.
   - Check `startTime`/`endTime`/`orderDate` against `stamp` and `order_date` in `client.rs`.
   - Check `utc_offset`: a start time agent-cli prints should match the web client's, shifted to UTC.
   - Check `--status failed` and `--status waiting`. The `Status::words` VERIFY asks whether `status` takes a comma list.
   - Check `--since 2h`. The `compact` VERIFY asks for the format of `fromTime`.
   - Check the order rows come in and whether an unfiltered search is slow. If it's slow, require a filter.
2. **`job get`**
   - On a waiting job: `ctm 'run/job/ID/waitingInfo'`.
   - On a failed one: the `[next: …]` note.
   - On a made-up id: which status and message come back. The `refused` VERIFY wants it to be exit 4 with the list's hint.
3. **`job logs`**
   - `ctm 'run/job/ID/output'` and `.../log`, for a finished job, a running one, and an old one whose output is gone.
   - The `text` VERIFY in `client.rs` asks whether text/plain still comes back as a JSON string.
   - `--execution N` on a job that ran twice.
4. **`definition list` and `get`**
   - `ctm 'deploy/jobs?format=json&ctm=*&folder=SOME_FOLDER&job=*'`.
   - Hold the answer against `found` in `definition/mod.rs`: where the server name is, how sub-folders nest, and the key names for command, script, host, run-as.
   - Time an unfiltered `definition list`.
5. **`doctor`:** whether your user may read `config/servers`.

## 4. Writes, on a test job

Set `read_only = false` only on a non-production instance, or pick a
harmless job agreed with its owner.

1. **`job retry ID`** on an ended job.
   - Check the answer's shape (the `answer` field).
   - Retry a job that's still executing: which status and words come back. Map them to exit 5 (`Failure::conflict`) with a hint in `refused` or in `retry.rs`.
2. **`job run SERVER/FOLDER/JOB`**
   - Check that it orders exactly one run.
   - Check that `ignoreCriteria: true` is wanted. The alternative is a `--criteria` flag that respects the calendar.
   - Check that `run/status/RUN_ID` lists the run immediately. If it's empty at first, retry within `ctx.deadline()`.
   - Then try `JOB` = `*` for a folder, and a sub-folder `SERVER/A/B/JOB`.
3. **Session limits:** run 20 commands in a row with the password config. If the Enterprise Manager complains about sessions, log out at the end of each command (the `bearer` VERIFY), or switch to an API token.

## 5. Finish

1. Delete each `VERIFY(work)` comment as its check passes, keeping any
   line that still says why the code is as it is.
2. Move what turned out true (the version, the formats, the quirks) into
   `crates/controlm/AGENTS.md`, under 3 KB.
3. Run `scripts/check.sh --all`, then
   `UPDATE_DOCS=1 cargo test -p agent-cli reference` if any command's help
   changed.
4. Optional, before agent trials: record scrubbed answers into
   `fixtures/world/http/controlm.json` and add `world_controlm.rs`
   (`docs/how-to/add-a-domain.md` §8). A cross-domain story fits naturally:
   the Control-M job that loads orders fails on the same order 88123 as
   Airflow's `etl_nightly`.
5. Delete this plan.

## Left out on purpose

These are easy to add with `scripts/new-command.sh` when someone asks for
them:
- **hold, free, kill, set to OK, confirm** (`POST /run/job/ID/{hold,free,kill,setToOk,confirm}`): set to OK and kill are `Destructive`.
- **run now**, which skips the job's waits (`runNow`).
- **resources and variables.**
- **the jobs a job waits for:** `neighborhood=1&direction=radial` on `run/jobs/status`.
