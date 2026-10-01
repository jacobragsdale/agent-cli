# How to add a domain

This guide adds a new service to agent-cli as a domain: a crate that exports
one `Domain`, its config section and credential, a client that is the only way
its requests leave the process, its commands, and its part of the contoso
world. The airflow and dd domains were built this way; their plans are in
[docs/plans/](../plans/), and their code is the reference to copy from.

## Decide whether it earns a domain

A domain costs every agent something: a line in the overview, words in search
that can collide with other domains, and a section to configure. Agents
already know kubectl, az and curl well, so a domain earns its place by what it
adds on top of them: bounded output, safety, and joins between sources. It
earns a domain when all four hold:

1. It serves six or more real, recurring tasks.
2. A trial with the tool agents would otherwise use (kubectl, az, curl, a
   vendor CLI), against the real service or a sandbox, shows a loss
   agent-cli fixes: wrong answers, outputs over 30 KB, twice the calls, or an
   unguarded write.
3. Its sign-in fits one config section, and `agent-cli doctor` can check it.
4. It has five or more commands.

With fewer commands, it is a resource of an existing domain. The reasoning is
in [docs/plans/cross-domain.md](../plans/cross-domain.md), section 6.

Prerequisites:

- The service's API reference, and a way to capture real answers (then
  scrub them: the repository is public).
- Familiarity with [How to add a command](add-a-command.md), which this guide
  uses for each command.

## 1. Write the plan

Write `docs/plans/<name>.md` before code, as `airflow.md` and `datadog.md` do:
the commands (path, effect, the API call behind each), the ids rows print and
which verbs take them, the config section and credential, how it joins other
domains (a pod id, an image reference), and the open questions. Reviewing a
plan is cheaper than reviewing a crate.

## 2. Create the crate

Create `crates/<name>/` with a `Cargo.toml` copied from `crates/dd/Cargo.toml`
(workspace version, edition, lints, `publish = false`) and lay out `src/` as
every crate does (`check_layout` holds the commands to it):

```
src/lib.rs               the index: mod lines and the Domain constant
src/client.rs            the door every request goes through (step 4)
src/doctor.rs            status and doctor (step 5)
src/testing.rs           #[cfg(test)] helpers the crate's tests share
src/<resource>/mod.rs    what that resource's verbs share, and their mod lines
src/<resource>/<verb>.rs one command: args, rows, handler, command!, tests
```

Add a card, `crates/<name>/AGENTS.md` (at most 3 KB: what it is, config, ids,
where things are, fixtures, quirks, what an agent can skip), and a
`CLAUDE.md` beside it holding `@AGENTS.md`. A test holds every file and name
the card mentions to the code.
The workspace takes every `crates/*` as a member. Then register it in three
places:

- the root `Cargo.toml` `[workspace.dependencies]`, as
  `agent-cli-<name> = { path = "crates/<name>" }`;
- `crates/cli/Cargo.toml` `[dependencies]`;
- `DOMAINS` in `crates/cli/src/main.rs` (its position is the overview's
  order), and the copy of that list in `crates/cli/tests/common/world.rs`.

Take dependencies from the workspace list. A new third-party dependency needs
a one-line reason in the commit message.

## 3. Define the config section

Each domain parses only its own section, lazily, so a broken section breaks
only that domain. Deserialize it into a struct that refuses unknown keys, and
read it with `ctx.section("<name>")` (or `config.section` in `status`, which
has no `Ctx`):

```rust
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Section {
    pub(crate) site: Option<String>,
    pub(crate) env: Option<String>,
    token: Option<String>,
    token_env: Option<String>,
    token_cmd: Option<String>,
    // …
}
```

A credential is three keys, `KEY` (in the file; discouraged, and dd refuses
it), `KEY_env` and `KEY_cmd`, handed to `Credential::from_keys("token", value,
env, cmd)`. Resolve it with `credential.resolve(ctx)` only when the first
request goes out, so `--dry-run` and read-only refusals never run a
`token_cmd`. `credential.source()` names where it comes from for doctor,
never its value.

A service with several servers takes a list, `[[<name>.instance]]`, and an
`--instance` flag that defaults to the only one through
`agent_cli_core::pick` (as airflow does). Add the section, commented out with
`contoso` placeholders, to `config.example.toml`.

## 4. Write the client

Every request goes through one door in the domain's client, which calls
`ctx.read(request)` or `ctx.write(effect, request)`. The door is where the
credential is attached, and only after checking the URL with
`agent_cli_core::host_under(url, host)`, for every URL including pagination
links. dd's door, `Call::perform` in `crates/dd/src/client.rs`, is a complete
example:

```rust
fn perform(self, ctx: &Ctx) -> Result<Response> {
    let api = format!("api.{}", self.dd.site.name);
    if !host_under(&self.url, &api) {
        bail!(
            "refusing to send the Datadog credential to {}; it goes only to {api}",
            self.url
        );
    }
    // … attach the token or the key pair, then perform the request
}
```

Core handles the rest for every domain: a non-2xx answer becomes a `Failure`
with the status, the exit code and the service's own words; throttles honour
`Retry-After` and `X-RateLimit-Reset` within the deadline; redirects are never
followed; a header named `Authorization`, `*-key`, `*_key` or `*token*` is
masked in plans and errors. If the service words its errors in a shape core
does not read yet, add it to `failure_message` in `crates/core/src/http.rs`.

## 5. Define the `Domain`

The crate exports one constant (the azure crate exports three: `KV`, `ACR`,
`AKS`):

```rust
pub const DOMAIN: Domain = Domain {
    name: "dd",
    summary: "Datadog",
    commands: &[
        logs::LOG_LIST,
        // … one line per command, in listing order
    ],
    synonyms: &[
        ("datadog", &["dd"]),
        ("alert", &["monitor"]),
        ("trace", &["span"]),
        // … words agents use for this domain's resources
    ],
    status,
    doctor,
};
```

- **`name`**: one lowercase word, not a built-in (`search`, `doctor`,
  `help`), not another domain's.
- **`summary`**: a few words for the overview's title line.
- **`synonyms`**: the words agents use for this domain's resources and
  fields. A key may be a phrase (`"error rate"`). They count only for this
  domain's commands, so a word can mean different things in two domains.
- **`status`**: `fn(&Config) -> String`, the overview's config line for the
  domain, such as `dd eu` or `airflow 2 instances`, from config alone: no
  network, no processes. It is cut at 22 characters.
- **`doctor`**: `fn(&Ctx) -> Vec<Check>`, the live checks for `agent-cli
  doctor`: the config, where the credential comes from, and one cheap live
  call. Return an empty `Vec` when the domain has no section (and no
  credential in the environment), and give each `Check::failed` a hint.

`Domain`, `Check`, `Config`, `Ctx`, `check_registry` and `check_layout` come
from `agent_cli_core`. Give the
crate the first test ado and dd have, so every later step has a checkpoint:

```rust
#[test]
fn the_registry_keeps_every_rule() {
    assert_eq!(check_registry(&[DOMAIN]), Vec::<String>::new());
    assert_eq!(check_layout(&[DOMAIN]), Vec::<String>::new());
}
```

Run `scripts/check.sh <name>` after each command you add.

## 6. Settle word collisions

`check_registry` refuses a synonym that is another domain's resource name,
because search would weigh the two readings the same. When both readings are
real, add the word to `SHARED_WORDS` in `crates/core/src/registry.rs` and add
labeled queries for each reading, as `task` (an ado Task work item, an
airflow task instance), `container` and `log` have.

## 7. Add the commands

Follow [How to add a command](add-a-command.md) for each one. Give every row
an `id` that the domain's `get` takes, and accept the service's web URL there
too. Where a row names another domain's thing, print that thing's id:
airflow's task `pod` is `prod/web/<pod>`, the id `k8s pod logs` takes, and
dd's rows do the same from the pod tags Datadog records. Add at least two
labeled queries per command to the new crate's `search.toml`
(`crates/<name>/search.toml`); the search gate reads every crate's file.

## 8. Add the domain to the contoso world

Trials need the new service to describe the same company as the rest:

1. Add `fixtures/world/http/<name>.json` with the exchanges for the service's
   host (see the world's README for the format).
2. Add the section to `fixtures/world/config.toml`.
3. For a credential, a stand-in `*_cmd` in that section, such as
   `token_cmd = "echo stand-in"`; `scripts/trial-env.sh` and the tests need
   no change.
4. Add `fixtures/world/facts/<name>.md`, the facts that tie the new service
   to the rest, such as the Airflow DAG whose task pods run in `prod/web`, or
   the Datadog monitor that alerted when the api rolled out, and link it from
   the world's README.
5. Add `crates/cli/tests/world_<name>.rs` for the commands a trial is likely
   to run, and a test to `crates/cli/tests/world_cross.rs` that walks one
   cross-domain chain end to end.

## 9. Keep the overview under 1 KB

The overview lists every domain on one line with its command count and adds
its status to the `Config:` line. `check_registry` holds it under 1024 bytes
with every domain's status at its longest, and
`the_overview_fits_with_every_domain_configured` in `crates/cli/src/main.rs`
checks a realistic config: add the new section to the TOML string there, with
names as long as real ones get, and add the status your `status` function
prints to the list the test asserts on. If the overview goes over 1 KB,
shorten the status line; the `Config:` line already cuts itself at 210
bytes.

## 10. Update the docs and run the checks

Add the domain to the tables in `README.md` (configure and domains), then
regenerate the command reference and run everything:

```sh
UPDATE_DOCS=1 cargo test -p agent-cli reference
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features agent-cli/fixtures -- -D warnings
cargo test --workspace
cargo test --workspace --features agent-cli/fixtures
```

To confirm the domain is wired in, rebuild the fixtures binary and look for it
in the overview and in doctor's checks against the world:

```sh
cargo build --features fixtures -p agent-cli
eval "$(scripts/trial-env.sh)"
agent-cli
agent-cli doctor --fields domain,check,ok
```

Then run at least six agent trial tasks against it, as
[How to run agent trials](run-agent-trials.md) describes, and turn every miss
into a fix, a test or a labeled query.
