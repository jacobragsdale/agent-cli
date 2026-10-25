# Porting agent-cli to work

Instructions for the agent doing the move, with the person who owns it
beside you. You are in the team's fork of agent-cli in Azure DevOps, on WSL
Ubuntu, signed in to the work tenant with `az login`, and the person's
`~/.config/agent-cli/config.toml` holds working credentials for every
service. GitHub holds the public repository, which has the team rollout the
fork lacks: a team config, shared logins in Key Vault, the installer and the
release pipeline. The fork has work of its own that GitHub never sees.

When you are done, the fork has both, every shared login is a secret in the
team's dev Key Vault, and the person runs the released agent-cli installed
the way each teammate will install it.

Each step ends with what to show the person. Go on only when they agree.
When something here doesn't match what you find, stop and say so rather
than working around it.

## Rules for this work

- **Never print a credential.** Don't open, `cat` or grep `config.toml`,
  `secrets.toml`, the files a `*_cmd` key reads, or a shell profile that
  holds a password. `agent-cli config` shows the config with every
  credential masked. Values move only through a pipe into
  `scripts/vault-put.sh`.
- **Ask before anything leaves this machine:** a push to the fork, a write to
  Key Vault, a release tag. Say what will happen first.
- **GitHub is read-only.** The fork holds real names; never push to the
  `github` remote (step 1 makes that fail).
- **Code changes** follow `AGENTS.md` and the `agent-cli-dev` skill, and
  `scripts/check.sh --all` passes before every commit.

Below, `VAULT` is the name of the team's dev Key Vault, which holds every
agent-cli secret for now (prod's too), and `KEEPER` is the Artifact Keeper
server's host.

## 1. Merge GitHub's changes into the fork

Add GitHub as a remote that can only fetch:

```sh
git remote add github https://github.com/jacobragsdale/agent-cli.git
git remote set-url --push github DISABLED
git fetch github main
```

If github.com is blocked from work, the person makes a bundle at home
(`git bundle create agent-cli.bundle main`) and brings the file; add that
path as the `github` remote instead.

### Find where the fork started

If the fork kept GitHub's history, this prints a commit, which is BASE; go
on to "Review":

```sh
git merge-base HEAD github/main
```

If it prints nothing, the fork was imported as new history. Find the newest
GitHub commit whose files one of the fork's commits has exactly:

```sh
fork_trees=$(git log --format=%T HEAD)
for c in $(git rev-list github/main); do
  grep -qx "$(git rev-parse "$c^{tree}")" <<<"$fork_trees" && { echo "$c"; break; }
done
```

That commit is BASE. FORK is the oldest fork commit with the same tree:
`git log --format='%H %T' HEAD | awk -v t="$(git rev-parse "BASE^{tree}")" '$2 == t {print $1}' | tail -1`.

If no commit matches, the import changed files. Then FORK is the fork's first
commit, and BASE is the GitHub commit with the fewest changed lines from it:

```sh
root=$(git rev-list --max-parents=0 HEAD | tail -1)
for c in $(git rev-list github/main); do
  printf '%s %s\n' "$(git diff --numstat "$c" "$root" | awk '{n += $1 + $2} END {print n + 0}')" "$c"
done | sort -n | head -3
```

Show the person BASE (`git log -1 --format='%h %ad %s' BASE`) and the
runners-up, and confirm. Then, only for an imported fork, tell git for this
merge that FORK came after BASE:

```sh
git replace --graft FORK BASE
```

This rewrites no commit. It lets the merge compare both sides with BASE, so
it takes only GitHub's changes since then instead of treating every file
as a conflict.

### Review

What arrives, and what the fork changed, which it keeps:

```sh
git log --oneline BASE..github/main
git diff --stat BASE github/main
git diff --stat BASE HEAD
```

Files in both `--stat` lists are where conflicts will be. Show the person
both lists.

### Merge

```sh
git merge --no-ff --no-commit github/main
```

Resolve each conflict:

- **`Cargo.lock`**: `git checkout --theirs Cargo.lock`; the build below adds
  back what the fork's own crates need.
- **`docs/reference/`**: take either side; it is generated, and
  `UPDATE_DOCS=1 cargo test -p agent-cli reference` rewrites it from the
  merged code once it builds.
- **A list both sides added to** (`DOMAINS` in `crates/cli/src/main.rs`, the
  workspace `members` in `Cargo.toml`, a `search.toml`, a table in
  `README.md` or `AGENTS.md`, `TODO.md`): keep both sides' entries.
- **Code both sides changed**: keep both behaviours. `git log
  BASE..github/main -- PATH` shows why GitHub changed it.

Then run the checks:

```sh
cargo build --workspace
scripts/check.sh --all
```

A failure in the fork's own commands is usually a rule GitHub has tightened
since BASE (`check_registry`, the search gate, the 1 KB overview). Fix the
fork's code the way the failure says, then finish the merge:

```sh
git commit -m "Merge agent-cli from GitHub up to $(git rev-parse --short github/main)"
```

If you grafted, remove the graft: `git replace -d FORK`. The merge commit has
`github/main` as a parent, so the next sync is a plain
`git merge github/main`. To stop partway, run `git merge --abort` and the
same `git replace -d FORK`.

Show the person `git log --oneline -3` and the check results, and push to
the fork's main when they say so.

## 2. Take stock of the credentials

Build the merged binary and read the config as it is today:

```sh
cargo build --release -p agent-cli
target/release/agent-cli config
target/release/agent-cli doctor
```

`config` masks each credential and names its source: a literal (shown as
`***`), `KEY_env`, `KEY_cmd` or `KEY_kv`. Today's doctor is the baseline:
whatever passes now must pass at the end.

Make a table with one row per credential key. The columns:

- the section, the entry and the key
- where the value is now
- the secret it becomes
- its environment (`dev`, `qa` or `prod`)

The secret name is `agentcli-` plus the section, the entry's name and the
key, in lower case, with anything but letters and digits as `-`. For
example, `[[sql.connection]] name = "reporting-prod"`'s `password` becomes
`agentcli-sql-reporting-prod-password`, and `[ado]`'s `pat` becomes
`agentcli-ado-pat`. It is the name `KEY_kv = true` reads, so derive it
exactly.

Every login except Azure becomes a shared secret:

- the SQL Server, Oracle and Postgres passwords
- Redis access keys
- Airflow logins
- the ADO and Confluence tokens
- the Datadog keys

Three kinds of credential don't:

- Anything with `auth = "az"` stays on each person's own `az login`.
- Control-M reads with no credential, so drop its username and password.
- Keeper's public repositories read anonymously.

A value that already lives in another vault (`KEY_kv = "OTHER/NAME"`) is
copied too, so every agent-cli secret sits in one vault under one prefix.
Also list the settings in `config.toml` that are only the person's, not
the team's (a local test database, a personal default). They stay in
`config.toml`.

Tell the person this: with one shared ADO token, everyone is that account
in ADO, so `@me`, comments and work item changes all belong to it, and so
does anything in the fork that depends on who is calling. Anyone who needs
to be themselves keeps their own `pat` in `config.toml`, which replaces the
team's.

Show the person the table, and go on when they confirm it.

## 3. Fill in team/team.toml

`team/team.toml` comes with contoso placeholders. Replace them with the
real values from the table and from `agent-cli config`:

- `[core] releases` is `https://KEEPER/api/v1/repositories/agent-cli/download`,
  and `[core] vault` is VAULT.
- Write each section and entry under the name the person uses today. Entry
  names become secret names, so keep them stable: renaming an entry
  later means renaming its secret.
- Write each shared credential as `KEY_kv = true` (`password_kv`, `pat_kv`,
  `token_kv`, `api_key_kv`, `app_key_kv`), with the non-secret half of the
  login (`user`, `username`) beside it.
- Set `read_only = true` on every prod entry. A prod SQL Server login must
  also be a login that can only read, since SQL Server has no read-only
  session; name any that can write.
- Delete the placeholder entries the team doesn't have.

Check it:

```sh
AGENT_CLI_CONFIG=team/team.toml target/release/agent-cli config
```

Every section should read with no `problem`, and every `*_kv` should print
as `VAULT/agentcli-…`. Those are the names the vault must hold, and they
must match the table's. Commit `team/team.toml`: the fork is private, and
real names belong in it.

## 4. Put the secrets in Key Vault

Confirm the account and the vault:

```sh
az account show --query user.name -o tsv
az keyvault show --name VAULT --query "{id: id, rbac: properties.enableRbacAuthorization}"
az keyvault secret list --vault-name VAULT --query "[?starts_with(name, 'agentcli-')].name" -o tsv
```

The last command lists any agent-cli secrets already there. Show the person
the secrets you will write, by name and environment (never values), and
ask. Then put each one, piping its value from where it is now into
`scripts/vault-put.sh VAULT/NAME ENV`. The script prints the new version's
id and never the value.

A literal in `config.toml`: the key is its path with entry names, here
`sql.connection.reporting-prod.password` (Python 3.11 or later):

```sh
python3 -c 'import sys, tomllib; value = tomllib.load(open(sys.argv[1], "rb"))
for part in sys.argv[2].split("."): value = next(e for e in value if e.get("name") == part) if isinstance(value, list) else value[part]
print(value)' ~/.config/agent-cli/config.toml sql.connection.reporting-prod.password | scripts/vault-put.sh VAULT/agentcli-sql-reporting-prod-password prod
```

A `KEY_env = "VAR"`:

```sh
printenv VAR | scripts/vault-put.sh VAULT/agentcli-ado-pat dev
```

A `KEY_cmd`, run as `config.toml` has it:

```sh
sh -c 'pass show work/ledger' | scripts/vault-put.sh VAULT/agentcli-sql-ledger-prod-password prod
```

A `KEY_kv = "OTHER/SECRET"` in another vault:

```sh
az keyvault secret show --vault-name OTHER --name SECRET --query value -o tsv | scripts/vault-put.sh VAULT/agentcli-redis-orders-prod-password prod
```

If the first write is refused as forbidden, the account lacks Key Vault
Secrets Officer on VAULT (or a Set access policy, when `rbac` is false).
The vault's owner grants it. When every row is written, compare:

```sh
az keyvault secret list --vault-name VAULT --query "[?tags.app=='agent-cli'].name" -o tsv | sort
```

It should list exactly the table's names. Never print a value to check it:
step 5 checks each one by signing in with it.

## 5. Switch the person's config to the team's

Keep the old file until everything works. It holds passwords, so step 7
deletes it.

```sh
cp -p ~/.config/agent-cli/config.toml ~/.config/agent-cli/config.toml.before-team
cp team/team.toml ~/.config/agent-cli/team.toml
```

Write `~/.config/agent-cli/config.toml` anew (mode 0600). It holds only the
include and the settings step 2 kept for the person, taken from `agent-cli
config`'s output, never from the old file:

```toml
[core]
include = ["~/.config/agent-cli/team.toml"]
```

No shared credential stays in it: one written there would replace the
team's Key Vault one for that entry. Then:

```sh
target/release/agent-cli config pull
target/release/agent-cli doctor
```

`config pull` prints one row per secret, and each should say `ok: true`. A
failure's detail says why:

- **Not found:** the name differs from the vault's; compare steps 3 and 4.
- **Forbidden:** the account lacks Key Vault Secrets User.

Doctor should pass every row that passed in step 2. A refused sign-in there
means a value was copied wrong: put that one again (step 4) and rerun.

Then use each domain as an agent would. `target/release/agent-cli` lists
them, and `target/release/agent-cli DOMAIN` its commands; run a list or
get in each, on dev and prod entries. `grep -rn "VERIFY(work)" crates`
lists the assumptions made without a real service. Check those whose
service you just used, and fix or record what differs.

Show the person `config pull`, `doctor` and what each domain answered.

## 6. Release and install

Set up Keeper and the pipeline as sections 2 to 4 of
`docs/how-to/roll-out-to-a-team.md` say:

- a public generic repository `agent-cli` in Keeper
- the pipeline's Keeper token in its variable group
- the agent pool's name in `azure-pipelines.yml`

Commit, ask, and push main. When the pipeline's checks pass on it, tag the
workspace version (`[workspace.package] version` in `Cargo.toml`) as
`vVERSION` and push the tag, again asking first. The pipeline uploads the
release.

Then install it as a teammate would:

```sh
curl -fsSL https://KEEPER/api/v1/repositories/agent-cli/download/latest/install.sh | bash
```

`install.sh` replaces `team.toml` with the released copy and leaves
`config.toml` as step 5 wrote it. `which -a agent-cli` should list
`~/.local/bin/agent-cli` first; an older `cargo install`ed copy earlier on
the PATH shadows it, so remove that one. `agent-cli doctor`'s first row
names the release. Show the person doctor and the system prompt line that
`install.sh` printed.

## 7. Finish

- Once doctor passes, delete the old file:
  `shred -u ~/.config/agent-cli/config.toml.before-team`. Ask the person
  whether the variables and files the old `KEY_env` and `KEY_cmd` keys read
  (a password exported in `~/.bashrc`) should go too.
- Teammates each need Key Vault Secrets User on VAULT, one role assignment
  to the team's Entra group, which the vault's owner may have to make. Then
  they follow section 5 of `docs/how-to/roll-out-to-a-team.md`. That role on
  the whole vault also reads the dev apps' secrets. Assigning it on each
  `agentcli-` secret instead (`--scope VAULT_ID/secrets/NAME`) limits it to
  these.
- When prod's secrets move to the prod vault, name each one whole on its
  entry, `password_kv = "PROD_VAULT/agentcli-sql-reporting-prod-password"`,
  which overrides `[core] vault`. Then delete the dev copies, which
  `az keyvault secret list --vault-name VAULT --query "[?tags.app=='agent-cli'].name"`
  finds.

## Afterwards

- **A new shared login:** add its entry to `team/team.toml` with
  `KEY_kv = true`. `AGENT_CLI_CONFIG=team/team.toml agent-cli config` prints
  the secret's name. Pipe the value into
  `scripts/vault-put.sh VAULT/NAME ENV`, then release.
- **A rotation:** set the new password on the service, then pipe it into
  `agent-cli kv secret update VAULT/NAME -`. Everyone's agent-cli reads it
  again after its next refused sign-in.
- **More from GitHub:** `git fetch github main`, then
  `git merge github/main`; there's no graft after the first time.
