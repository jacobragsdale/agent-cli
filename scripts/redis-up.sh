#!/usr/bin/env bash
# Start the two throwaway Redis instances crates/redis/tests/redis.rs runs
# against, and seed them (scripts/seed/redis.py). Localhost only; every
# password here is throwaway.
#
#   qa    127.0.0.1:16379  plain TCP, allkeys-lru, every command slow-logged;
#                          user agent/agent-pass holds only the reads agent-cli
#                          sends, user nobody/nobody-pass not even INFO
#   prod  127.0.0.1:16380  TLS with a self-signed certificate (target/redis-tls),
#                          allkeys-lfu, default user, password prod-pass
#
#   scripts/redis-up.sh        start (again) and seed both
#   scripts/redis-up.sh down   remove both
#
# Then: AGENT_CLI_TEST_REDIS=1 cargo test -p agent-cli-redis
set -euo pipefail
cd "$(dirname "$0")/.."

IMAGE=redis:7.4
READS=(+@read -@dangerous +info +slowlog\|get +client\|list +client\|id +client\|setname +ping +select)

docker rm -f agent-cli-redis-qa agent-cli-redis-prod >/dev/null 2>&1 || true
[ "${1:-}" = down ] && exit 0

certs=target/redis-tls
if [ ! -f "$certs/server.crt" ]; then
    mkdir -p "$certs"
    openssl req -x509 -newkey rsa:2048 -nodes -days 3650 -subj /CN=localhost \
        -addext "subjectAltName=IP:127.0.0.1,DNS:localhost" \
        -keyout "$certs/server.key" -out "$certs/server.crt" 2>/dev/null
    chmod 644 "$certs/server.key"
fi

docker run -d --name agent-cli-redis-qa -p 127.0.0.1:16379:6379 "$IMAGE" \
    redis-server --save '' --appendonly no --maxmemory 64mb --maxmemory-policy allkeys-lru \
    --slowlog-log-slower-than 0 --slowlog-max-len 1024 --requirepass seed-pass \
    --user agent on '>agent-pass' '~*' '&*' "${READS[@]}" \
    --user nobody on '>nobody-pass' '~*' +ping +client\|setname >/dev/null
docker run -d --name agent-cli-redis-prod -p 127.0.0.1:16380:6379 \
    -v "$PWD/$certs:/tls:ro" "$IMAGE" \
    redis-server --save '' --appendonly no --maxmemory 64mb --maxmemory-policy allkeys-lfu \
    --port 0 --tls-port 6379 --tls-cert-file /tls/server.crt --tls-key-file /tls/server.key \
    --tls-ca-cert-file /tls/server.crt --tls-auth-clients no --requirepass prod-pass >/dev/null

python3 -I scripts/seed/redis.py
echo "redis-up: qa 127.0.0.1:16379, prod 127.0.0.1:16380 (TLS); AGENT_CLI_TEST_REDIS=1 cargo test -p agent-cli-redis"
