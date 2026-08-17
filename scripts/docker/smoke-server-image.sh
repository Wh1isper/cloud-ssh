#!/usr/bin/env bash
set -euo pipefail

image="${1:?usage: smoke-server-image.sh <image>}"
suffix="${RANDOM}-$$"
name="owlmux-smoke-${suffix}"
postgres_name="owlmux-smoke-postgres-${suffix}"
network="owlmux-smoke-${suffix}"

cleanup() {
  docker rm --force "$name" "$postgres_name" >/dev/null 2>&1 || true
  docker network rm "$network" >/dev/null 2>&1 || true
}
trap cleanup EXIT

docker network create "$network" >/dev/null
docker run --detach --name "$postgres_name" --network "$network" \
  --env POSTGRES_DB=owlmux \
  --env POSTGRES_PASSWORD=owlmux \
  --env POSTGRES_USER=owlmux \
  postgres:17.10-alpine >/dev/null
for _ in {1..30}; do
  if docker exec "$postgres_name" pg_isready --username owlmux --dbname owlmux >/dev/null 2>&1; then
    break
  fi
  sleep 1
done
docker exec "$postgres_name" pg_isready --username owlmux --dbname owlmux >/dev/null

docker run --detach --name "$name" --network "$network" --publish 127.0.0.1::8080 \
  --env OWLMUX_PUBLIC_ORIGIN=http://127.0.0.1:8080 \
  --env "OWLMUX_DATABASE_URL=postgres://owlmux:owlmux@${postgres_name}:5432/owlmux" \
  --env OWLMUX_API_KEY=owlmux_sk_v1_YWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWE \
  --env OWLMUX_SSH_KEY_ENCRYPTION_KEY=YmJiYmJiYmJiYmJiYmJiYmJiYmJiYmJiYmJiYmJiYmI \
  --env OWLMUX_SSH_RUNTIME_ROOT=/tmp/owlmux-ssh \
  --env OWLMUX_CONFIG_EPOCH=1 \
  --env OWLMUX_NODE_NAME=image-smoke \
  "$image" >/dev/null
port="$(docker port "$name" 8080/tcp | sed -n 's/.*://p' | head -n 1)"
[[ -n "$port" ]]

for _ in {1..30}; do
  if curl --fail --silent "http://127.0.0.1:${port}/health" | grep -q '"status":"ok"'; then
    break
  fi
  sleep 1
done

curl --fail --silent "http://127.0.0.1:${port}/health" | grep -q '"service":"owlmux-server"'
curl --fail --silent "http://127.0.0.1:${port}/ready" | grep -q '"status":"ready"'
curl --fail --silent "http://127.0.0.1:${port}/" | grep -q 'OwlMux'
status="$(curl --silent --output /dev/null --write-out '%{http_code}' "http://127.0.0.1:${port}/api/v1/machines")"
[[ "$status" == "401" ]]
