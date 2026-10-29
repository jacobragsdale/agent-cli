#!/usr/bin/env bash
# Start the http domain's live harness (scripts/openapi/compose.yaml): the
# same small orders API on nine OpenAPI frameworks, TLS in front of fastapi,
# and three proxies, all on 127.0.0.1. Every secret here is throwaway.
#
#   eval "$(scripts/openapi-up.sh)"                   build, start (again), wait, export
#   eval "$(scripts/openapi-up.sh fastapi go-swag)"   only these services (the Java
#                                                     builds are slow); tls brings fastapi
#   scripts/openapi-up.sh down                        stop and remove everything
#
# A rerun keeps running containers and rebuilds only what changed. It waits
# until every running service answers, then prints, on stdout and nothing
# else, the exports for the running services and the shared values:
#
#   OPENAPI_FASTAPI_URL             58080  FastAPI 1.4.2, the rich app (below)
#   OPENAPI_FASTAPI_V2_URL          58081  the same app as 2.0.0 (changes below)
#   OPENAPI_ASPNET_SWASHBUCKLE_URL  58082  ASP.NET Core 8, Swashbuckle 6.9.0
#   OPENAPI_ASPNET_OPENAPI_URL      58083  ASP.NET Core 9, MapOpenApi + Scalar
#   OPENAPI_SPRINGDOC_URL           58084  Spring Boot 3.5, springdoc 2.8, groups orders and admin
#   OPENAPI_NESTJS_URL              58085  NestJS 11, @nestjs/swagger at 'api'
#   OPENAPI_GO_SWAG_URL             58086  gin + swaggo (Swagger 2.0)
#   OPENAPI_DRF_SPECTACULAR_URL     58087  Django REST Framework + drf-spectacular
#   OPENAPI_QUARKUS_URL             58088  Quarkus 3 + smallrye-openapi, Swagger UI in prod
#   OPENAPI_ECHO_URL                58089  a second origin running the fastapi app; /echo
#   OPENAPI_TLS_URL                 58443  https, the test CA's cert for localhost,
#                                          127.0.0.1 and api.contoso.test
#   OPENAPI_MTLS_URL                58444  the same, and a client cert from the test CA required
#   OPENAPI_TLS_EXPIRED_URL         58445  from the test CA, expired 2021-01-01
#   OPENAPI_TLS_SELF_SIGNED_URL     58446  self-signed, not from the test CA
#   OPENAPI_TLS_WRONG_HOST_URL      58447  from the test CA, SAN only other.contoso.test
#   OPENAPI_TLS_CA_AS_LEAF_URL      58448  the test CA's own cert (CA:TRUE, SAN localhost)
#   OPENAPI_TLS_INTERMEDIATE_URL    58449  a leaf from an intermediate CA, sent with it
#   OPENAPI_PROXY                   53128  squid, Basic auth OPENAPI_PROXY_USER:OPENAPI_PROXY_PASSWORD
#   OPENAPI_PROXY_OPEN              53129  squid, no auth
#   OPENAPI_PROXY_NEGOTIATE         53130  answers everything, CONNECT too, with 407
#                                          Proxy-Authenticate: Negotiate and NTLM
#   OPENAPI_CA                      the test root CA (PEM); the TLS ports behind it
#   OPENAPI_CLIENT_CERT, OPENAPI_CLIENT_KEY   the mTLS client cert and unencrypted key (PEM)
#   OPENAPI_TOKEN                   contoso-test-token, for /secure/orders
#   OPENAPI_BASIC_USER, OPENAPI_BASIC_PASSWORD  agent:basic-pass, for /basic
#   OPENAPI_API_KEY                 contoso-api-key, as X-Api-Key, for /apikey
#   OPENAPI_TRACE_ID                the trace id in /fail/503's traceparent
#
# The proxies resolve the compose service names, which the host cannot:
# http://fastapi:8000/healthz and http://echo:8000/echo work only through
# them, and CONNECT reaches https://api.contoso.test:58443-58449 (the tls
# service by its alias; the certs name it). Both squids add Via.
#
# The fastapi app (58080) besides GET/POST /orders and GET /orders/{id}:
#   GET /orders?status=open        600 orders; /orders/1's note is 5,120 bytes
#   GET /orders/9999               404 {"detail": "order 9999 not found"}; a bad POST is 422
#   POST /orders/search            a POST that only reads
#   PUT, PATCH /orders/{id} 200;   DELETE /orders/{id} 204
#   GET /fail/400 401 409 500      the status with a JSON error (401 with WWW-Authenticate)
#   GET /fail/503                  503 with traceparent
#                                  00-0000000000000000199e6ab3c4d5e6f7-00f067aa0ba902b7-01
#                                  (low 64 bits 1846030217602524919) and
#                                  x-request-id: req-contoso-503
#   GET /fail/429?key=K            429 with Retry-After: 1 the first time per key, then 200
#   GET /slow?seconds=N            answers after N seconds
#   GET /redirect                  302 to /orders/1 (?to=URL, ?status=307 and the like)
#   GET /redirect/external         302 to $OPENAPI_ECHO_URL/echo (?status= too)
#   GET /redirect/loop             302 to itself
#   GET /secure/orders             Bearer contoso-test-token, else 401 WWW-Authenticate: Bearer
#   GET /basic, GET /apikey        Basic agent:basic-pass; X-Api-Key: contoso-api-key
#   any /echo                      {method, path, headers, query, body} as received
#   GET /binary, GET /text         2,048 bytes of octet-stream; 40 KB of text/plain
#   POST /token                    {"access_token": "secret-value", "token_type": "Bearer", ...}
#   GET /healthz                   {"status": "ok", "version": "1.4.2"}
#   POST /admin/deploy?version=X&after=S   /healthz reports X from S seconds on
#   POST /admin/reset              forget orders made, keys throttled and deploys
# The harness routes (fail, slow, redirect, echo, binary, text, admin) are
# left out of the spec.
#
# fastapi-v2 (58081) is the same app with ORDERS_API_VERSION=2.0.0; against
# 1.4.2 its spec:
#   breaking  POST /orders (and PUT) body gains required currency, enum EUR, USD
#   breaking  GET /orders' status query param loses on_hold
#   breaking  DELETE /orders/{id} is removed
#   breaking  the Order response drops note
#   added     GET /orders/{order_id}/fulfilment (schema Fulfilment)
#   added     GET /orders gains the optional query param customer_id
#   info.version 1.4.2 -> 2.0.0
# scripts/openapi/specs/ holds the spec each framework served at its
# default path (fastapi's 1.4.2 and 2.0.0 both), and scripts/openapi/FINDINGS.md
# what each serves where: the http domain's well-known paths come from it.
# crates/cli/tests/live_http.rs (AGENT_CLI_TEST_HTTP=1) runs every http
# command against all of this.
#
# OPENAPI_STATE (default target/openapi-up) holds the generated certificates
# and the proxy's password file; they are made once and again when the
# server certificate is within a day of expiring.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
state=${OPENAPI_STATE:-$(dirname "$here")/target/openapi-up}
export OPENAPI_STATE=$state
compose=(docker compose -f "$here/openapi/compose.yaml")

if [ "${1:-}" = down ]; then
    exec "${compose[@]}" down --remove-orphans
fi

started=$SECONDS
mkdir -p "$state"

# certs: the test CA and every certificate the TLS ports serve, one key for
# all the leaves.
certs() {
    local dir=$1 ext=$1/ext.cnf
    cd "$dir"
    openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out ca-key.pem
    openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out intermediate-key.pem
    openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out server-key.pem
    openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out client-key.pem
    # The root names localhost too, so on 58448 being a CA is its only fault.
    openssl req -x509 -new -key ca-key.pem -sha256 -days 3650 -out ca.pem \
        -subj "/O=Contoso/CN=Contoso Test Root CA" \
        -addext "basicConstraints=critical,CA:TRUE" \
        -addext "keyUsage=critical,keyCertSign,cRLSign,digitalSignature" \
        -addext "subjectAltName=DNS:localhost,IP:127.0.0.1"
    # sign NAME KEY ISSUER EXTENSIONS [x509 options]: NAME.pem from a CSR for KEY.
    sign() {
        local name=$1 key=$2 issuer=$3 extensions=$4
        shift 4
        printf '%s\n' "$extensions" | tr ';' '\n' >"$ext"
        openssl req -new -key "$key" -subj "/O=Contoso/CN=$name" |
            openssl x509 -req -CA "$issuer.pem" -CAkey "$issuer-key.pem" -sha256 \
                -extfile "$ext" -out "$name.pem" "$@"
    }
    local leaf="basicConstraints=CA:FALSE;keyUsage=critical,digitalSignature;extendedKeyUsage=serverAuth"
    local names="subjectAltName=DNS:localhost,IP:127.0.0.1,DNS:api.contoso.test"
    sign server server-key.pem ca "$leaf;$names" -days 825
    sign expired server-key.pem ca "$leaf;$names" -not_before 20200101000000Z -not_after 20210101000000Z
    sign wrong-host server-key.pem ca "$leaf;subjectAltName=DNS:other.contoso.test" -days 825
    sign intermediate intermediate-key.pem ca \
        "basicConstraints=critical,CA:TRUE,pathlen:0;keyUsage=critical,keyCertSign,cRLSign" -days 3650
    sign intermediate-leaf server-key.pem intermediate "$leaf;$names" -days 825
    cat intermediate-leaf.pem intermediate.pem >chain.pem
    sign client client-key.pem ca \
        "basicConstraints=CA:FALSE;keyUsage=critical,digitalSignature;extendedKeyUsage=clientAuth" -days 825
    openssl req -x509 -new -key server-key.pem -sha256 -days 825 -out self-signed.pem \
        -subj "/O=Contoso/CN=localhost" -addext "$names" -addext "basicConstraints=CA:FALSE"
    rm -f "$ext" ./*.srl
}
if ! openssl x509 -checkend 86400 -noout -in "$state/server.pem" >/dev/null 2>&1; then
    (certs "$state") >&2
    echo "openapi-up: wrote the test CA and certificates to $state" >&2
fi
[ -f "$state/squid-passwd" ] ||
    printf 'agent:%s\n' "$(openssl passwd -apr1 proxy-pass)" >"$state/squid-passwd"
# squid reads it as its own user, nginx the keys as root.
chmod 644 "$state/squid-passwd"

"${compose[@]}" up -d --build --quiet-build "$@" >&2
# A container that exits at once (a bad config) is gone before the wait.
sleep 1
exited=$("${compose[@]}" ps --services --status exited)
if [ -n "$exited" ]; then
    echo "openapi-up: exited:" $exited"; see: docker compose -p agent-cli-openapi logs SERVICE" >&2
    exit 1
fi

url_of() {
    case $1 in
        fastapi) echo http://127.0.0.1:58080 ;;
        fastapi-v2) echo http://127.0.0.1:58081 ;;
        aspnet-swashbuckle) echo http://127.0.0.1:58082 ;;
        aspnet-openapi) echo http://127.0.0.1:58083 ;;
        springdoc) echo http://127.0.0.1:58084 ;;
        nestjs) echo http://127.0.0.1:58085 ;;
        go-swag) echo http://127.0.0.1:58086 ;;
        drf-spectacular) echo http://127.0.0.1:58087 ;;
        quarkus) echo http://127.0.0.1:58088 ;;
        echo) echo http://127.0.0.1:58089 ;;
        tls) echo https://localhost:58443 ;;
        squid) echo http://127.0.0.1:53128 ;;
        squid-open) echo http://127.0.0.1:53129 ;;
        negotiate) echo http://127.0.0.1:53130 ;;
    esac
}
# answers SERVICE: the service answers as it should once it is up.
answers() {
    local url code
    url=$(url_of "$1")
    case $1 in
        fastapi | fastapi-v2 | echo) code=$(curl -s -o /dev/null -w '%{http_code}' -m 2 "$url/healthz") ;;
        drf-spectacular) code=$(curl -s -o /dev/null -w '%{http_code}' -m 2 "$url/orders/") ;;
        tls) code=$(curl -s -o /dev/null -w '%{http_code}' -m 2 --cacert "$state/ca.pem" "$url/healthz") ;;
        # A proxy is up once it answers at all; squid's answer is 407 or 503.
        squid | squid-open | negotiate)
            code=$(curl -s -o /dev/null -w '%{http_code}' -m 2 -x "$url" http://echo:8000/echo)
            [ "$code" != 000 ]
            return
            ;;
        *) code=$(curl -s -o /dev/null -w '%{http_code}' -m 2 "$url/orders") ;;
    esac
    [ "$code" = 200 ]
}

mapfile -t running < <("${compose[@]}" ps --services --status running)
deadline=$((SECONDS + 180))
for service in "${running[@]}"; do
    until answers "$service"; do
        if [ -z "$("${compose[@]}" ps -q --status running "$service")" ]; then
            echo "openapi-up: $service exited; see: docker compose -p agent-cli-openapi logs $service" >&2
            exit 1
        fi
        if [ $SECONDS -ge $deadline ]; then
            echo "openapi-up: $service did not answer on $(url_of "$service") within 180 s; see: docker compose -p agent-cli-openapi logs $service" >&2
            exit 1
        fi
        sleep 0.5
    done
done
echo "openapi-up: ${running[*]} answering after $((SECONDS - started)) s" >&2

for service in "${running[@]}"; do
    case $service in
        tls)
            echo "export OPENAPI_TLS_URL=https://localhost:58443"
            echo "export OPENAPI_MTLS_URL=https://localhost:58444"
            echo "export OPENAPI_TLS_EXPIRED_URL=https://localhost:58445"
            echo "export OPENAPI_TLS_SELF_SIGNED_URL=https://localhost:58446"
            echo "export OPENAPI_TLS_WRONG_HOST_URL=https://localhost:58447"
            echo "export OPENAPI_TLS_CA_AS_LEAF_URL=https://localhost:58448"
            echo "export OPENAPI_TLS_INTERMEDIATE_URL=https://localhost:58449"
            ;;
        squid) echo "export OPENAPI_PROXY=$(url_of squid)" ;;
        squid-open) echo "export OPENAPI_PROXY_OPEN=$(url_of squid-open)" ;;
        negotiate) echo "export OPENAPI_PROXY_NEGOTIATE=$(url_of negotiate)" ;;
        *)
            name=${service//-/_}
            echo "export OPENAPI_${name^^}_URL=$(url_of "$service")"
            ;;
    esac
done
echo "export OPENAPI_CA=$state/ca.pem"
echo "export OPENAPI_CLIENT_CERT=$state/client.pem"
echo "export OPENAPI_CLIENT_KEY=$state/client-key.pem"
echo "export OPENAPI_PROXY_USER=agent"
echo "export OPENAPI_PROXY_PASSWORD=proxy-pass"
echo "export OPENAPI_TOKEN=contoso-test-token"
echo "export OPENAPI_BASIC_USER=agent"
echo "export OPENAPI_BASIC_PASSWORD=basic-pass"
echo "export OPENAPI_API_KEY=contoso-api-key"
echo "export OPENAPI_TRACE_ID=0000000000000000199e6ab3c4d5e6f7"
