# agent-cli

One command-line tool that gives coding agents Azure DevOps, Azure (Key Vault,
Container Registry, AKS), Kubernetes and SQL Server/Oracle. It is built for
agents rather than people: every command is `agent-cli <domain> <resource>
<verb>`, output is compact JSON that `--fields` can narrow, errors say what to
run next, and changes honour `--dry-run`, `--yes` and a read-only mode.

## Install

```sh
cargo install --git https://github.com/jacobragsdale/agent-cli agent-cli
```

## Start

Run `agent-cli` for the overview, then start with `agent-cli search` and the
words for what you want to do.
