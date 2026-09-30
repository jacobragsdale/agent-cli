#!/usr/bin/env bash
# Stop this repo's two databases. Pass -v to drop their data volumes too.
set -euo pipefail
cd "$(dirname "$0")/.."
docker compose down "$@"
