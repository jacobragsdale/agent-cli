# How to roll out agent-cli to a team

This guide takes the repository to a team whose members install agent-cli
with one command and get every service the team uses, shared database logins
included, with no setup of their own beyond signing in. It assumes Ubuntu
under WSL on every machine, an Azure DevOps organization with a self-hosted
Linux agent pool that runs Docker, an [Artifact Keeper](https://artifactkeeper.com)
server, and a Key Vault for the logins the team shares.

Throughout, `contoso`, `keeper.contoso.example` and the like are
placeholders: the fork is where the real names go.

## 1. Fork the repository and fill in team.toml

Import the repository into the team's Azure DevOps project as a new
repository (Repos, Import repository), so the fork has no link back to the
public one. Everything below happens in the fork. A fork that already exists
takes the public repository's later changes as `PORTING.md` says, which also
walks the whole move for an agent to follow.

`team/team.toml` is the config every teammate gets. Replace each placeholder
with the team's real names: the `[core] releases` URL, the ADO organization
and projects, the Confluence address, the subscriptions, vaults and
registries, one `[[k8s.scope]]` per cluster, one `[[sql.connection]]` per
database, and the Redis, Airflow and Control-M instances. Every login in it
is shared; only Azure signs in as each person.

- **Shared logins** go in Key Vault, one secret per credential, and never in
  anyone's file. `[core] vault` names the vault, and `password_kv = true` (or
  `pat_kv`, `token_kv` …) on an entry reads the secret named for it,
  `agentcli-SECTION-ENTRY-KEY`; `agent-cli config` prints each name.
  `scripts/vault-put.sh VAULT/SECRET ENV` puts a value there from stdin.
  Every teammate needs Key Vault Secrets User on that vault.
- **Prod entries** keep `read_only = true`, which refuses every write before
  it is sent. Give them logins that can only read as well: on SQL Server the
  login is the only guard, since it has no read-only session.
- **Oracle** connections use the Instant Client that `install.sh` installs;
  keep `[sql] oracle_client_dir` as it is.

Check it with the binary built from the fork:

```sh
cargo build --release -p agent-cli
AGENT_CLI_CONFIG=team/team.toml target/release/agent-cli config
AGENT_CLI_CONFIG=team/team.toml target/release/agent-cli doctor
```

Every section should read back as you wrote it; doctor's failures there are
the credentials, which come with your own install. Install first yourself
(section 5), run doctor and each domain's reads as yourself, and fix what
breaks before anyone else installs.

## 2. Make a home for releases in Artifact Keeper

In Artifact Keeper, as an administrator:

1. Create a generic repository with the key `agent-cli`, local, with
   visibility `public`, so teammates download without signing in.
2. Create an API token for the pipeline (your profile, API tokens) with the
   `read:artifacts`, `write:artifacts` and `delete:artifacts` scopes:
   republishing `latest/` deletes the old files first, since Keeper never
   overwrites one.

The repository's download URL is what `[core] releases` in `team.toml` holds:
`https://keeper.contoso.example/api/v1/repositories/agent-cli/download`.

Keeper limits downloads to 30 a minute per client address. If the team
reaches it through one NAT or proxy and installs fail with HTTP 429, ask its
administrator to raise `RATE_LIMIT_PRESIGN_PER_MIN`.

## 3. Set up the pipeline

`azure-pipelines.yml` runs CI's five checks on every push to main, and on a
`v*` tag also builds the release and uploads it, every step inside the image
`ci/Dockerfile` builds (Ubuntu 22.04, so the binary runs on 22.04 and every
newer Ubuntu).

1. In `azure-pipelines.yml`, set `pool: name:` to the self-hosted pool. Its
   agents need Docker, and the user the agent runs as must be able to run it.
2. In Pipelines, Library, create a variable group named as in the file
   (`contoso-agent-cli-release`) holding `KEEPER_URL`
   (`https://keeper.contoso.example`) and `KEEPER_TOKEN` (the token above,
   marked secret), and give the pipeline permission to use it.
3. Create the pipeline from the existing `azure-pipelines.yml` and run it on
   main.

The first run downloads the crates the workspace uses from crates.io. If the
agents cannot reach it, point cargo at a crates.io proxy repository in
Artifact Keeper, or commit the sources with `cargo vendor`. The cargo
registry and `target/` stay in the agent's pipeline workspace between runs.

## 4. Cut a release

Set `version` under `[workspace.package]` in `Cargo.toml`, commit it to main,
then tag that commit with the same version and push the tag:

```sh
git tag v0.2.0
git push origin v0.2.0
```

The release stage refuses a tag that is not `v` and the workspace version.
It uploads this layout to the repository, the versioned files first and
`latest/VERSION` last:

| Path | What it is |
|---|---|
| `0.2.0/agent-cli-x86_64-linux.tar.gz` | the binary, alone in the archive |
| `0.2.0/agent-cli-x86_64-linux.tar.gz.sha256` | its checksum, as `sha256sum` prints it |
| `0.2.0/team.toml` | `team/team.toml` as it was at the tag |
| `0.2.0/install.sh` | `scripts/install.sh`, defaulting to `[core] releases` |
| `latest/…` | the same four files |
| `latest/VERSION` | `0.2.0`, which `install.sh` and `doctor` read |

A change to `team.toml` alone is also a release: bump the version and tag.

To release without the pipeline, build in the CI image (as yourself, so
`target/` stays yours) and upload from anywhere:

```sh
docker build -t agent-cli-ci ci
docker run --rm --user "$(id -u):$(id -g)" -v "$PWD:/src" -w /src -e HOME=/tmp -e CARGO_HOME=/src/target/cargo agent-cli-ci scripts/release-build.sh v0.2.0
KEEPER_URL=https://keeper.contoso.example KEEPER_TOKEN=... scripts/release-upload.sh target/dist
```

## 5. What a teammate runs

Send each teammate these steps:

```sh
curl -fsSL https://keeper.contoso.example/api/v1/repositories/agent-cli/download/latest/install.sh | bash
az login
agent-cli config pull
agent-cli doctor
```

`install.sh` puts the binary in `~/.local/bin`, the team's config in
`~/.config/agent-cli/team.toml`, and on the first run a `config.toml` of their
own that includes it. It installs Oracle Instant Client under
`~/.local/opt/oracle`, using sudo only for the apt packages it lacks (unzip,
patchelf, libaio), then reads the shared logins (`agent-cli config pull`,
which needs the `az login`) and runs `agent-cli doctor`. If `~/.local/bin` was
not on their PATH yet, it says to open a new terminal.

Nobody adds credentials: every login comes from Key Vault. A person who
needs to be themselves somewhere (their own ADO token, so `@me` is them)
sets it in `~/.config/agent-cli/config.toml`, where an entry with the same
name changes only the keys it sets, and a credential replaces the team's:

```toml
[ado]
pat_env = "MY_ADO_PAT"
```

Last, the line `install.sh` printed goes into each person's agent
instructions (a global `AGENTS.md`, Cursor rules or Copilot instructions). It
names no services, since the binary says what is configured:

> agent-cli is installed: it reaches the team's work services; run `agent-cli`
> to see what is configured, then `agent-cli search WORDS`.

## 6. Updates

Running `install.sh` again updates the binary and replaces `team.toml`; it
leaves `config.toml` alone. `agent-cli doctor` reads `latest/VERSION` and
says when a newer release is out (`0.3.0 is out; rerun install.sh`). To
install a given release instead of the latest, name it:

```sh
curl -fsSL https://keeper.contoso.example/api/v1/repositories/agent-cli/download/latest/install.sh | AGENT_CLI_VERSION=0.2.0 bash
```

The Instant Client is downloaded again only when `install.sh` pins a new
one; `AGENT_CLI_ORACLE_ZIP_URL` points it at a mirror of the same zip if
Oracle's site is blocked.

## 7. Rotate a shared login

Set the new password on the database, then store it as a new version of its
Key Vault secret (`agent-cli config` prints its name):

```sh
pass show db/reporting | agent-cli kv secret update kv-contoso-dev/agentcli-sql-reporting-prod-password -
```

Nobody else has to do anything: when the kept password
fails to sign in, agent-cli reads the secret again, keeps the new value and
retries once. To refresh every shared login at once:

```sh
agent-cli config pull
```

It prints one row per secret, whether it changed, never the value.

## 8. Collect feedback

agent-cli logs each command it runs on the machine it runs on (the command,
the flag names, the exit code, the time and requests taken; never values) and
sends nothing anywhere. Keep one work item on the team's board for agent-cli
feedback, and ask each teammate to post their week to it every Friday:

```sh
agent-cli usage --since 7d | agent-cli ado workitem comment ID -
```

The summary shows what agents call most, which calls fail and with what
flags, and which are slow: where the next fix goes.

## Rehearse it locally

The whole loop runs on one machine against a throwaway Artifact Keeper.
Build in the CI image as in section 4 (without the tag), then:

```sh
eval "$(scripts/keeper-up.sh)"
scripts/release-upload.sh target/dist
docker run --rm -it --network host -e AGENT_CLI_RELEASES ubuntu:24.04 bash
```

In the container, `apt-get update && apt-get install -y curl`, then
`curl -fsSL "$AGENT_CLI_RELEASES/latest/install.sh" | bash`. Doctor's version
check reads `team.toml`'s placeholder address and says it got no answer;
nothing else depends on it. Stop the server with `scripts/keeper-up.sh down`.
