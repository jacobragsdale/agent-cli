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
`aisearch`, `arm`, `net`, `lake`, `k8s`, `sql`, `airflow`, `controlm`, `dd`);
leaving out `lake` also leaves out DuckDB, its query engine. The
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
`pass show contoso/db`). Use `KEY_env` when many connections share one
password, so a rotation is one change.

An Airflow or Control-M instance whose credential you do not have yet can
go in the file anyway: only that instance fails, naming the key to add.

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

## 4. Azure: Key Vault, Container Registry, AKS, AI Search and Data Lake (kv, acr, aks, aisearch, lake)

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

## 6. SQL Server and Oracle (sql)

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

Confluence Cloud only: a site at `SITE.atlassian.net`. Data Center and
Server are not supported.

```sh
agent-cli config example confluence
```

```toml
[confluence]
url = "https://contoso.atlassian.net/wiki"
email = "jane@contoso.com"
token = "..."
```

Make an API token at
https://id.atlassian.com/manage-profile/security/api-tokens ("Create API
token") while signed in as the account in `email`; it is sent as Basic
`email:token` to the site and nowhere else. Every token expires within a
year, so make a note of when.

A token made with "Create API token with scopes", or a service account's,
works only through Atlassian's gateway: add the site's `cloud_id`, which
`https://SITE.atlassian.net/_edge/tenant_info` prints with no sign-in. Calls
then go to `api.atlassian.com`, as Bearer when there is no `email`. Give
such a token the read scopes for pages, spaces, comments, attachments and
users, and the write scopes only where agents may publish.

```sh
agent-cli doctor confluence
```

The connection row must say "signed in as" your name. A site that lets
anyone read answers a wrong or expired token as the anonymous user instead
of refusing it, and doctor reports that as a failure.

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

## Check everything

```sh
agent-cli doctor
agent-cli config
```

Every domain you set up should show only `"ok":true` rows. Then tell your
agents the tool exists; one line in a global `CLAUDE.md` or `AGENTS.md` is
enough: "agent-cli gives you Azure DevOps, Azure, Kubernetes, SQL, Airflow
and Datadog; start with `agent-cli`."
