#!/bin/sh
# Prints the environment that runs agent-cli against a recorded world, for
# agent trials with no real Azure DevOps, Azure or cluster:
#
#   cargo build --features fixtures -p agent-cli
#   eval "$(scripts/trial-env.sh)"          # or: scripts/trial-env.sh fixtures/world
#   agent-cli k8s deployment list
#
# AGENT_CLI_FIXTURES replays <world>/http/*.json instead of the network (only
# in a build with the fixtures feature) and serves <world>/kubectl.json from
# the fake kubectl; AGENT_CLI_FIXTURES_MATCH=loose answers a request the world
# did not record with the closest recording of its method and path (an agent
# picks its own --since and --limit), or a 404; PATH puts scripts/fake (az,
# kubectl, kubelogin) and the fixtures build first; AGENT_CLI_NOW freezes the
# clock the world was recorded at, so relative times such as --since 1d
# resolve to recorded URLs.
set -eu
repo=$(cd "$(dirname "$0")/.." && pwd)
world=$(cd "${1:-$repo/fixtures/world}" && pwd)
cat <<ENV
export AGENT_CLI_FIXTURES='$world'
export AGENT_CLI_FIXTURES_MATCH=loose
export AGENT_CLI_CONFIG='$world/config.toml'
export AGENT_CLI_NOW='2026-09-29T12:00:00Z'
export PATH='$repo/target/debug:$repo/scripts/fake':"\$PATH"
export AZURE_CONFIG_DIR='$world/.azure-unused'
unset AZURE_DEVOPS_EXT_PAT AGENT_CLI_READ_ONLY
ENV
