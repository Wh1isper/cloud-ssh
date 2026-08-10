#!/usr/bin/env bash
set -euo pipefail

image="${1:?usage: smoke-server-image.sh <image>}"
name="owlmux-smoke-${RANDOM}-$$"

cleanup() {
  docker rm --force "$name" >/dev/null 2>&1 || true
}
trap cleanup EXIT

docker run --detach --name "$name" --publish 127.0.0.1::8080 "$image" >/dev/null
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
[[ "$status" == "404" ]]
