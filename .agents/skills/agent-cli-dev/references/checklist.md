# Before every commit

- [ ] `scripts/check.sh --all` passes: CI's five checks (fmt, clippy with and
      without `--features agent-cli/fixtures`, tests with and without it).
- [ ] The reference is fresh: `UPDATE_DOCS=1 cargo test -p agent-cli reference`
      leaves `git status` clean.
- [ ] No placeholder is left: no `TODO` in a new command file, and no
      `text = "TODO` query in a `search.toml`.
- [ ] Every new or changed command has a fixture test; a `Write`,
      `Destructive` or `Varies` one also has a dry-run test.
- [ ] Names are `contoso`-style placeholders everywhere, commit message
      included: the repository is public.
- [ ] Only the files the task needs changed (`git status`); a shared file
      (`VERBS`, `SHARED_WORDS`, `DOMAINS`, `config.example.toml`) changed
      only on purpose.
- [ ] A new dependency comes from the workspace list, with a one-line reason
      in the commit message.
- [ ] The commit subject is a descriptive sentence saying what changed
      (see `git log`). Push only when asked.
