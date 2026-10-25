#!/usr/bin/env bash
# Puts one shared login in Key Vault, as a new secret or a new version of
# one: the value comes on stdin (trailing newlines dropped) and goes to az
# through a pipe, never on a command line, on disk or back on the screen.
#
#   printenv REPORTING_PASSWORD | scripts/vault-put.sh kv-contoso-dev/agentcli-sql-reporting-prod-password prod
#
# VAULT/SECRET is what `agent-cli config` prints for a KEY_kv key (KEY_kv =
# true reads agentcli-SECTION-ENTRY-KEY from [core] vault). The secret is
# tagged app=agent-cli, and env=ENV when given, so
#   az keyvault secret list --vault-name VAULT --query "[?tags.app=='agent-cli'].name" -o tsv
# lists them. Needs az signed in with Key Vault Secrets Officer on the vault
# (or an access policy with Set). Prints the new version's id.
set -euo pipefail

die() {
    printf 'vault-put.sh: %s\n' "$*" >&2
    exit 2
}

[ $# -ge 1 ] && [ $# -le 2 ] || die "usage: scripts/vault-put.sh VAULT/SECRET [ENV] < value"
reference=$1
env=${2:-}
vault=${reference%%/*}
secret=${reference#*/}
[[ $reference == */* && $vault =~ ^[A-Za-z0-9-]{3,24}$ && $secret =~ ^[A-Za-z0-9-]{1,127}$ ]] ||
    die "$reference is not VAULT/SECRET (a vault's name, not its URL, then the secret's)"
[ ! -t 0 ] || die "the value goes on stdin, not the terminal: pipe it in"

value=$(cat)
[ -n "$value" ] || die "stdin was empty; nothing was written"
tags=(app=agent-cli)
[ -z "$env" ] || tags+=("env=$env")

# printf is a builtin, so the value is in no process's arguments; --query id
# keeps az from printing the secret back.
printf '%s' "$value" | az keyvault secret set --vault-name "$vault" --name "$secret" \
    --file /dev/stdin --encoding utf-8 --tags "${tags[@]}" --query id --output tsv
