# How to set up your services

This guide takes a machine with no agent-cli on it to one where every domain
you use passes `agent-cli doctor`. It is written for an agent to follow top to
bottom, with no checkout of this repository: the binary carries the example
config. Do the sections for the services you have and skip the rest; a domain
you leave out stays "not set up" and costs nothing.

Throughout, `contoso`, `web`, `reporting` and the like are placeholders: use
your own names, and keep them out of anything you publish.

## 1. Install

You need Rust 1.88 or later and a C compiler (the Oracle driver builds a small
C library). On Ubuntu:

```sh
sudo apt-get install -y build-essential pkg-config
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
cargo install --git https://github.com/jacobragsdale/agent-cli agent-cli
```

Running the same `cargo install` again upgrades it.

### One build for a team

To give a team a binary that needs no setup, build your finished config into
it. Keep the file outside the checkout:

```sh
AGENT_CLI_BUILTIN_CONFIG=$HOME/team/config.toml cargo build --release -p agent-cli --features builtin-config
```

`target/release/agent-cli` then reads that config whenever there is no file
on disk; `agent-cli config` names it `the built-in config`. Anyone's own
file replaces it whole, and `AGENT_CLI_<SECTION>_<KEY>` still overrides one
key, such as `AGENT_CLI_ADO_PAT` for each person's own token. The binary holds
every literal credential in the file in plain text (`strings` finds them), so
hand it only to the people those credentials are for. Rebuild to change it.

To build only some domains, name them: `--no-default-features --features
sql,airflow,builtin-config` gives a binary with only `sql` and `airflow`.
The features are the domain names (`ado`, `confluence`, `kv`, `acr`, `aks`,
`aisearch`, `arm`, `net`, `lake`, `entra`, `k8s`, `sql`, `redis`, `airflow`, `controlm`, `dd`, `keeper`). The
config decides which databases, instances and environments the build
reaches, so give each group its own file: one holding only the dev
`[[airflow.instance]]` carries nothing for prod. Leave out the sections of
domains you leave out, or their credentials still sit in the binary.
Leaving a domain out removes its commands, not anyone's access: Azure and
Kubernetes run as the person's own `az login` and kubeconfig, which
`az` and `kubectl` reach without agent-cli.

## 2. See where the config goes

```sh
agent-cli config
agent-cli doctor
```

`agent-cli config` prints the config file's path, whether it exists, each
section as the domains read it (credentials masked) and every `AGENT_CLI_*`
variable in effect. The file is `~/.config/agent-cli/config.toml` unless
`AGENT_CLI_CONFIG` names another. `agent-cli doctor` prints one JSON row per
check: a domain that is not set up says so, with the command that prints its
section.

For each service below, print its section, paste it into the config file,
uncomment and fill in the keys, then run the doctor for that domain:

```sh
agent-cli config example sql
agent-cli doctor sql
```

`agent-cli config example` with no domain prints every section. A doctor run
naming a domain exits 1 if any of its checks failed, and each failed row's
`hint` is the next step.

For the first week, give each service a credential that can only read (a
Reader role, a read-only database login, an Airflow Viewer), so nothing an
agent runs can change a service while you learn what it does. A destructive
command already needs `--yes`, and any change can be previewed with
`--dry-run`. Any key in the file can also be
set from the environment as `AGENT_CLI_<SECTION>_<KEY>`, for example
`AGENT_CLI_ADO_PROJECT=web`.

### Where secrets come from

Put each password or token in the config file beside the rest of its
section, and `chmod 600 ~/.config/agent-cli/config.toml`. One file then
holds every service: adding one is a block and a doctor run, and rotating a
password is one line. Each credential `KEY` (`pat`, `password`, `token`) can
instead be `KEY_env` (the variable that holds it) or `KEY_cmd` (a shell
command that prints it, run only when a request needs it, such as
`pass show contoso/db`), or `KEY_kv` (a team's shared login in Key Vault,
below). Use `KEY_env` when many connections share one password, so a
rotation is one change.

An Airflow or Control-M instance whose credential you do not have yet can
go in the file anyway: only that instance fails, naming the key to add.

### A team's shared file

A team keeps its connections, instances and sites in one file everyone
gets, and each person's `config.toml` includes it:

```toml
[core]
include = "~/.config/agent-cli/team.toml"     # or a list of files
```

The included file is read first and yours over it. A key you set wins; a
table merges key by key; an entry of a list whose entries have a `name`
(`[[sql.connection]]`, `[[k8s.scope]]`, `[[airflow.instance]]`, …) merges
with the team's entry of that name, so you set only the keys that differ,
and a new name adds an entry. Any other list you set replaces the team's.
Only your own file may include. An included file that is missing stops
every domain with exit 3, naming the file and the line that includes it:
rerun the team's `install.sh`, which puts it there. `agent-cli config`
lists each included file (found or not) and every section as merged.

`[core] releases` (usually in the team's file) names where releases are
published; `agent-cli doctor`'s first row then says when a newer one is out.

### Shared logins from Key Vault

A login the whole team shares (a read-only database user, a service
account) can live in Key Vault instead of in anyone's file. Name the vault
and the secret, `VAULT/SECRET`, with `KEY_kv`:

```toml
[[sql.connection]]
name = "ledger"
# …
password_kv = "kv-contoso-team/ledger-reader"
```

The first command that needs it reads the secret with your `az login`
(which needs the Key Vault Secrets User role on the vault) and keeps it in
`secrets.toml` beside your config file (mode 0600; agent-cli writes it,
nobody edits it). When the kept value stops signing in, because someone
rotated it, the domain reads Key Vault again and, if the value changed,
retries once. To read every `*_kv` key again at once:

```sh
agent-cli config pull
```

It prints one row per key (`section`, `key`, `kv`, `ok`, `changed`), never a
value; a key that fails keeps its old value. `agent-cli config` shows the
`*_kv` references and never what `secrets.toml` holds.

### Read only

`read_only = true` in a section (`[sql]`, `[azure]` for all eight Azure
domains, `[datadog]`, `[ado]`, …) refuses every change agent-cli would
make there: exit 2 before anything is sent, naming the scope, with
`--dry-run` still printing what it would have done. On one entry
(`[[sql.connection]]`, `[[k8s.scope]]`, `[[airflow.instance]]`,
`[[controlm.instance]]`, `[[redis.instance]]`) it overrides its section's.
Reading a secret (`kv secret get --reveal`) is a read and still works.

A team's file marks its prod entries read-only. No flag and no
`AGENT_CLI_*` variable lifts it (a variable that tries is exit 3), since
those are what an agent can set. A person who needs to change prod adds the
entry to their own `config.toml` with only the key that differs:

```toml
[[sql.connection]]
name = "ledger"
read_only = false
```

On a read-only Oracle connection every read also runs inside `SET
TRANSACTION READ ONLY`, and a read-only Postgres connection starts its
session with `default_transaction_read_only=on`, so the server itself
refuses a write agent-cli did not see coming. SQL Server has no session
equivalent, so a prod SQL Server still needs a login that can only read:
`read_only` stops agent-cli, not the login.

## 3. Azure DevOps (ado)

```sh
agent-cli config example ado
```

```toml
[ado]
org = "contoso"
project = ["web", "mobile"]        # the first is the default
team = ["web team", "mobile/mobile team"]   # PROJECT/TEAM for a project other than the first
pat = "..."
```

With several projects, `workitem list` and `activity list` search all of them
in one query, and `--project NAME` narrows to one. Commands bound to one
project (`workitem create`, `sprint`, `backlog`, `team`, `query`,
`workitem-type`, `person`) take `--project` and default to the first. A
command on one work item uses that item's own project, whatever the default.
`--iteration @current` reads each listed team's current sprint, so list a team
per project. `code_project` (repositories, pull requests, pipelines) stays one
project.

The personal access token comes from `pat` (or `pat_env`, `pat_cmd`), else
`AZURE_DEVOPS_EXT_PAT`, else your `az login`. Give it Work Items, Code,
Build, Test Management, and Project and Team scopes (read only for the first
week).

```sh
agent-cli doctor ado
```

The credential row names where the token came from. A 401 means the token is
wrong or expired; a project check that fails means a name under `project` is
misspelled or belongs to another organization.

## 4. Azure: Key Vault, Container Registry, AKS, AI Search, Data Lake and Entra ID (kv, acr, aks, aisearch, lake, entra)

These sign in with the Azure CLI's login. Install `az`, then:

```sh
az login
az account list -o table
```

```toml
[azure]
# subscriptions = ["00000000-0000-0000-0000-000000000000"]
```

The section may be empty; it must be there for doctor to check Azure. Lists
of `subscriptions`, `vaults`, `registries`, `search_services` and
`storage_accounts` narrow what the commands see (`agent-cli config example
azure` shows them).

```sh
agent-cli doctor kv
agent-cli doctor acr
agent-cli doctor aks
agent-cli doctor aisearch
agent-cli doctor lake
agent-cli doctor entra
```

A failed `az login` check means the login expired: run `az login` again. A
vault check that fails while the inventory passes means your account can see
the vault but has no data-plane role on it.

AI Search is called the way each service's own settings allow, never as
configured. A service that takes roles (`disableLocalAuth`, or API keys and
roles both) gets your `az` token, and that needs these roles on the service:

| Commands | Role |
|---|---|
| `document list`, `document get` | Search Index Data Reader |
| `service get`, `index list`, `index get`, `indexer list`, `indexer get`, `indexer wait` | Reader, or Search Service Contributor |
| `index create`, `index update`, `index delete`, `indexer run` | Search Service Contributor |
| `document create`, `document update`, `document delete` | Search Index Data Contributor |

A service that takes API keys only (the default for a new one) gets its
admin key, which the CLI fetches from ARM for each run and never stores;
fetching it needs Contributor or Search Service Contributor. Owner and
Contributor give no data access by token. A role takes 5 to 10 minutes to
apply. `agent-cli aisearch service list` shows which way each service is
called (`auth`).

A Data Lake (a storage account with a hierarchical namespace) reads data
with your `az` token, which needs a data role on the account or a container
(Storage Blob Data Reader to read, Contributor to write) or ACLs that let
you in: Owner or Contributor on the account gives no data access. A doctor
account check that fails with a 403 names which is missing;
`agent-cli lake access get PATH --principal @me` shows where an ACL stops
you. An account reachable only through a private endpoint needs this
machine to resolve its `privatelink` name (`agent-cli net endpoint list`).

Entra ID reads the tenant your `az login` is in through Microsoft Graph,
with the Azure CLI's own Graph token: nothing to configure, and nothing it
sends writes. A member of the tenant can read users, groups, app
registrations, service principals and directory roles by default; a guest
cannot. The logs need more:

| Commands | Needs |
|---|---|
| `entra audit list` | Reports Reader, Security Reader or Global Reader |
| `entra signin list` | one of those roles (or Security Operator, Security Administrator), and Entra ID P1 or P2 on the tenant; Free keeps 7 days, P1 and P2 30 |

`agent-cli doctor entra` tries one read of each kind and names what a
refusal stops. Sign-ins of service principals and managed identities come
from Graph's beta version, the only one that lists them.

## 5. Kubernetes (k8s), through AKS

For each AKS cluster, fetch its credentials; the command prints the
`[[k8s.scope]]` block to paste:

```sh
agent-cli aks cluster connect NAME --resource-group GROUP
```

```toml
[[k8s.scope]]
name = "dev"
context = "aks-contoso-dev"
namespaces = ["web", "jobs"]
```

It runs `az aks get-credentials` (then `kubelogin convert-kubeconfig`), which
changes your kubeconfig, so it is a write. k8s needs `kubectl`
on `PATH`, and a cluster that uses Entra ID sign-in also needs `kubelogin`.

```sh
agent-cli doctor k8s
```

A failed `kubectl` row means it is not installed or not on `PATH`; a failed
scope row names the cluster and namespace your login cannot read.

## 6. SQL Server, Oracle, Postgres and Synapse (sql)

```sh
agent-cli config example sql
```

```toml
[sql]
oracle_client_dir = "~/.local/opt/oracle/instantclient_23_26"   # Oracle only

[[sql.connection]]
name = "reporting"
kind = "mssql"
host = "sql.contoso.example"
database = "reporting"
user = "app_reader"
password = "..."

[[sql.connection]]
name = "ledger"
kind = "oracle"
host = "ora.contoso.example"
port = 1521
service = "LEDGER"
user = "app_reader"
password = "..."
```

One `[[sql.connection]]` per database; `--conn` takes its `name`. SQL Server
takes a SQL login (`trust_cert = true` for a self-signed certificate). Oracle
takes the service name, not a SID, and needs Oracle Instant Client (Basic or
Basic Light): unzip it into `oracle_client_dir`. On Ubuntu 24.04 it also needs
libaio under its old name (see WSL below).

```sh
agent-cli doctor sql
```

A failed Oracle client row says why the client did not load: the directory,
or libaio. A connection that does not answer within doctor's 5 seconds is a
host, port, VPN or firewall problem; a refused login is the user or password.

Synapse's serverless SQL pool queries the Data Lake's files where they are.
Connect to it as SQL Server, signing in with your `az login` instead of a
password:

```toml
[[sql.connection]]
name = "synapse"
kind = "mssql"
host = "contoso-ws-ondemand.sql.azuresynapse.net"   # the workspace's serverless endpoint
database = "master"                                 # or a database made for views
auth = "az"
```

The token is minted for `https://database.windows.net/`, and your Entra ID
account must be a user of that Synapse workspace. Synapse then reads the
files as you, so you need Storage Blob Data Reader on the account or ACLs
that let you in: "File cannot be opened because it does not exist or it is
used by another process" usually means you lack them, which
`agent-cli lake access get PATH --principal @me` shows. `agent-cli lake table
get TABLE` prints the `bulk` path `OPENROWSET` takes:

```sh
agent-cli sql query run --conn synapse "SELECT r.filepath(1) AS date, count(*) AS orders FROM OPENROWSET(BULK 'https://contosolake.dfs.core.windows.net/raw/sales/orders/date=*/*.parquet', FORMAT = 'PARQUET') AS r GROUP BY r.filepath(1)"
```

Serverless bills by the data a query scans (at least 10 MB a query), and
stops a query at 30 minutes.

Postgres takes a database, a user and a password, and TLS by default
(`encrypt = true`), checked against the system's certificate store like
every other connection agent-cli makes:

```toml
[[sql.connection]]
name = "orders"
kind = "postgres"
host = "pg.contoso.example"          # port 5432 unless you say otherwise
database = "orders"
user = "agent_reader"
password_kv = "kv-contoso-team/pg-orders-reader"
```

A Postgres in AKS reached straight from WSL (a port-forward or an internal
load balancer) may speak no TLS: the connection then fails with `server
does not support TLS`, and `encrypt = false` connects in the clear. One
with a self-signed certificate wants `trust_cert = true`, or its CA in the
system store (`/usr/local/share/ca-certificates/`, then `sudo
update-ca-certificates`).

Azure Database for PostgreSQL Flexible Server signs you in with your
`az login`: the token is the password, and `user` is the Entra user or
group the server has a role for (your own `alice@contoso.example`, or a
group such as `contoso-pg-readers` you are a member of):

```toml
[[sql.connection]]
name = "events"
kind = "postgres"
host = "contoso-events.postgres.database.azure.com"
database = "events"
user = "contoso-pg-readers"
auth = "az"
```

The token is minted for `https://ossrdbms-aad.database.windows.net`. The
server needs Microsoft Entra authentication turned on and a role for that
principal (`pgaadauth_create_principal`); a refused sign-in says
`password authentication failed`. Flexible Server requires TLS, so leave
`encrypt` on.

## 7. Airflow (airflow)

```sh
agent-cli config example airflow
```

```toml
[[airflow.instance]]
name = "dev"
base_url = "https://airflow-dev.contoso.example"
username = "agent"
password = "..."

[[airflow.instance]]
name = "prod"
base_url = "https://airflow.contoso.example"
username = "agent"
password = "..."
```

One block per environment; `--instance` takes its `name`. Airflow 2.9
(`/api/v1`) and Airflow 3 (`/api/v2`) are both supported: each instance's
version is asked of the server once an hour (`api = "v1"` or `"v2"` skips the
question).

```sh
agent-cli doctor airflow
```

The sign-in row says how it got in. On Airflow 2 the password goes as HTTP
Basic, which needs `airflow.api.auth.backend.basic_auth` in the server's
`[api] auth_backends`; when Basic is refused, agent-cli signs in through the
web login form instead. The form works, but it signs in again on every
command, and the webserver allows about five sign-ins in 40 seconds, so a busy
agent gets throttled: ask the admins to add `basic_auth`. A wrong password
exits 3. A login page with no password field means single sign-on, where your
own login has no password: ask for a service account.

## 8. Datadog (dd)

```sh
agent-cli config example dd
```

```toml
[datadog]
site = "datadoghq.eu"
token = "..."
```

The credential is a personal access token, or the API and application key
pair (`api_key`, `app_key`). Give it read scopes only.

```sh
agent-cli doctor dd
```

The credential row names where the credential came from, never its value; a
failed connection row carries Datadog's own message. A refusal usually means
the wrong `site` or a missing scope.

## 9. Confluence (confluence)

A self-hosted Confluence: Data Center or Server, 7.9 or later (the first
with personal access tokens). Confluence Cloud (`SITE.atlassian.net`) is
not supported.

```sh
agent-cli config example confluence
```

```toml
[confluence]
url = "https://wiki.contoso.example"
token = "..."
```

`url` is the wiki's address as the browser shows it, before `/display` or
`/spaces`, with its context path when it has one
(`https://contoso.example/confluence`). Make a personal access token in
Confluence: your avatar, Profile, Personal Access Tokens, Create token (or
open `URL/plugins/personalaccesstokens/usertokens.action`). It is sent as
`Bearer` to addresses under `url` and nowhere else. Give it an expiry you
will notice, or none if your admin allows it; an admin can also revoke it
or turn tokens off for the site. The token can do what your account can,
so agents publish and comment as you.

A wiki whose certificate comes from your company's own certificate
authority needs that CA in the system's store (on Ubuntu or WSL,
`/usr/local/share/ca-certificates/`, then `sudo update-ca-certificates`).

```sh
agent-cli doctor confluence
```

The connection row must say "signed in as" your name. A wrong or expired
token is refused with 401, and the hint names the token page; a site that
lets anyone read may instead answer as the anonymous user, which doctor
also reports as a failure.

## 10. Control-M (controlm)

An on-prem Control-M Enterprise Manager, over its Automation API (usually
port 8443).

```sh
agent-cli config example controlm
```

```toml
[[controlm.instance]]
name = "prod"
base_url = "https://ctm-em.contoso.example:8443/automation-api"
username = "agent"
password = "..."
utc_offset = "-05:00"
```

The username and password are the ones Control-M's web client takes; each
command logs in once. An API token (Enterprise Manager 9.0.21 and later,
made in the web client) goes as `token` instead of the pair. Set
`utc_offset` to the Control-M/Servers' clock: the API prints times on it, and
agent-cli turns them into UTC.

```sh
agent-cli doctor controlm
```

The credential row names the Control-M/Servers the credential sees. A
certificate this machine does not trust exits 3: agent-cli uses the
system's trust store, so install the CA that signed the Enterprise Manager's
certificate there (on WSL, as [below](#wsl)), not anywhere in agent-cli.

## 11. Redis (redis)

Azure Cache for Redis, Azure Managed Redis or a self-hosted Redis (Valkey
too), one `[[redis.instance]]` each, usually one per project and
environment. agent-cli only reads: it sends Redis nothing but read commands.

```sh
agent-cli config example redis
```

```toml
[[redis.instance]]
name = "orders-qa"
host = "contoso-orders-qa.redis.cache.windows.net"
auth = "az"

[[redis.instance]]
name = "orders-prod"
host = "contoso-orders.redis.cache.windows.net"
password_cmd = "az redis list-keys -g rg-contoso -n contoso-orders --query primaryKey -o tsv"

[[redis.instance]]
name = "sessions-qa"
host = "redis-qa.contoso.internal"
user = "agent"
password_env = "REDIS_QA_PASSWORD"
```

Azure's hosts default to TLS on their own ports (6380 for Azure Cache,
10000 for Azure Managed Redis); anything else is plain 6379 unless `tls`
and `port` say otherwise. `auth = "az"` signs in with your `az login`'s
Entra ID token, as its object id; your account needs a data access policy
on the cache. An access key works as the password instead. A self-hosted
server with a self-signed certificate needs `trust_cert = true`.

```sh
agent-cli doctor redis
```

Each instance answers `PING` or says why not. `redis instance get` reads
`INFO`, and `redis client list` and `redis slowlog list` their own admin
reads, so give the user exactly those: the ACL `+@read -@dangerous +info
+slowlog|get +client|list +client|id +client|setname +ping +select`, or an
Azure access policy like it. A user without them exits 3 naming the ACL.

## 12. Artifact Keeper (keeper)

Artifact Keeper, the self-hosted artifact registry: its PyPI, generic and
other repositories, read only. Public repositories read without signing in,
so most people need only the address:

```sh
agent-cli config example keeper
```

```toml
[keeper]
url = "https://keeper.contoso.example"
token_env = "KEEPER_TOKEN"     # optional: for private repositories
```

`url` is the address the browser shows, without `/api/v1`. For private
repositories, make an API token on your profile's Access Tokens page
(`URL/access-tokens`); it goes as `Bearer` to URLs under `url` and nowhere
else. Without one, a private repository answers as if it were not there,
and the error says so.

```sh
agent-cli doctor keeper
```

Doctor names the server's version and whether it reads anonymously or as
the token's user. Keeper limits downloads per address (30 a minute by
default); `keeper file get --output` waits out one throttle.

## WSL

On WSL 2 under Windows 11, five things go wrong that do not on Linux.

**The corporate VPN breaks DNS.** Names resolve on Windows but not in WSL.
Mirror Windows' networking: put this in `%UserProfile%\.wslconfig` on the
Windows side, then run `wsl --shutdown` from PowerShell and reopen the
terminal.

```ini
[wsl2]
networkingMode=mirrored
```

**Oracle Instant Client on Ubuntu 24.04** looks for `libaio.so.1`, which
24.04 ships as `libaio.so.1t64`:

```sh
sudo apt-get install -y libaio1t64
sudo ln -s /usr/lib/x86_64-linux-gnu/libaio.so.1t64 /usr/lib/x86_64-linux-gnu/libaio.so.1
```

**An internal server's certificate is not trusted** (exit 3, "does not
trust the server's certificate"). Windows trusts the company CA, but WSL's
Ubuntu keeps its own store, which agent-cli reads. Export the CA on the
Windows side (PowerShell; change the subject to your company's CA):

```powershell
Get-ChildItem Cert:\LocalMachine\Root | Where-Object Subject -like '*Contoso*' |
  ForEach-Object { Export-Certificate -Cert $_ -FilePath "$env:USERPROFILE\company-ca.cer" }
```

then add it in WSL (`update-ca-certificates` takes PEM files named `.crt`):

```sh
openssl x509 -inform der -in /mnt/c/Users/YOU/company-ca.cer -out company-ca.crt
sudo cp company-ca.crt /usr/local/share/ca-certificates/ && sudo update-ca-certificates
```

A server with a self-signed certificate is trusted the same way, with its
own certificate in place of the CA's.

**`az login` opens no browser.** Use the device code instead, and open the
link it prints on Windows:

```sh
az login --use-device-code
```

**A `password_cmd` that calls `powershell.exe`** (to read the Windows
credential manager) costs about half a second per connection, every time.
Keep the password in the config file instead, or in a `chmod 600` env file
that your shell sources and a `password_env` names.

## The usage log

Every run appends one line to `~/.local/state/agent-cli/usage.jsonl`
(`$XDG_STATE_HOME`): the time, the version, the command path, the flag names
given (never their values, never a positional), the exit code, how long it
took, and the HTTP requests, bytes received and child processes it cost.
Nothing is sent anywhere; past 5 MB the file keeps its newest half, and
`AGENT_CLI_USAGE=off` stops it.

```sh
agent-cli usage --since 7d
```

prints the summary to paste where the team tracks agent-cli: calls per
command with their failures by exit code, median and slowest times,
requests and bytes; the most frequent exit 2s with their flag names; the
slowest runs and those with the most requests.

## Check everything

```sh
agent-cli doctor
agent-cli config
```

Every domain you set up should show only `"ok":true` rows. Search and the
overview leave out the domains that are not set up here, and those whose
reads doctor last saw refused with a 403 (it keeps them in
`~/.local/state/agent-cli/denied.json`, and a later doctor that passes
clears them); a search then ends with a note naming them, and
`agent-cli search --all WORDS` searches every domain. Their commands still
run. Then tell your
agents the tool exists; one line in a global `CLAUDE.md` or `AGENTS.md` is
enough: "agent-cli gives you Azure DevOps, Azure, Kubernetes, SQL, Airflow
and Datadog; start with `agent-cli`."
