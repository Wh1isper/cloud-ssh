#!/usr/bin/env bash
set -euo pipefail

image="${1:?usage: smoke-server-image.sh <image>}"
suffix="${RANDOM}-$$"
name="owlmux-smoke-${suffix}"
postgres_name="owlmux-smoke-postgres-${suffix}"
network="owlmux-smoke-${suffix}"

image_user="$(docker image inspect --format '{{.Config.User}}' "$image")"
image_revision="$(docker image inspect --format '{{index .Config.Labels "org.opencontainers.image.revision"}}' "$image")"
image_source="$(docker image inspect --format '{{index .Config.Labels "org.opencontainers.image.source"}}' "$image")"
image_version="$(docker image inspect --format '{{index .Config.Labels "org.opencontainers.image.version"}}' "$image")"
[[ "$image_user" == "owlmux" ]]
[[ -n "$image_revision" && "$image_revision" != "unknown" && "$image_revision" != "<no value>" ]]
[[ -n "$image_source" && "$image_source" != "<no value>" ]]
[[ -n "$image_version" && "$image_version" != "<no value>" ]]
[[ "$(docker image inspect --format '{{index .Config.Labels "org.opencontainers.image.licenses"}}' "$image")" == "BSD-3-Clause" ]]

image_environment="$(docker image inspect --format '{{range .Config.Env}}{{println .}}{{end}}' "$image")"
if grep -Eq '^(OWLMUX_API_KEY|OWLMUX_CLUSTER_KEY|OWLMUX_SSH_KEY_ENCRYPTION_KEY)=' <<<"$image_environment"; then
  echo "runtime image contains a secret-bearing environment variable" >&2
  exit 1
fi

docker run --rm --entrypoint /bin/sh "$image" -eu -c '
  test "$(id -u)" = 10001
  test -x /usr/local/bin/owlmux-server
  test -r /usr/share/owlmux/web/index.html
  test -r /usr/share/licenses/owlmux/LICENSE
  for required in ssh curl tini; do
    command -v "$required" >/dev/null
  done
  ssh -V >/dev/null 2>&1
  curl --version >/dev/null
  tini --version >/dev/null 2>&1
  for forbidden in apt apt-get apt-cache apt-mark deb-systemd-helper deb-systemd-invoke debconf dpkg dpkg-deb dpkg-query dpkg-reconfigure update-alternatives perl cargo rustc node npm npx pnpm cc gcc make git; do
    if command -v "$forbidden" >/dev/null 2>&1; then
      echo "unexpected runtime command: $forbidden" >&2
      exit 1
    fi
  done
  for forbidden_path in /etc/apt /etc/debconf.conf /etc/dpkg /etc/perl /usr/lib/apt /usr/lib/dpkg /usr/libexec/dpkg /usr/share/debconf /usr/share/dpkg /var/cache/debconf /var/lib/apt /var/lib/debconf /var/lib/dpkg /workspace /usr/local/cargo; do
    test ! -e "$forbidden_path"
  done
  unexpected_management_tool="$(
    find \
      /etc/cron.daily \
      /etc/logrotate.d \
      /etc/systemd/system \
      /usr/bin \
      /usr/lib/systemd/system \
      /usr/libexec \
      /usr/sbin \
      /usr/share/polkit-1/actions \
      -maxdepth 3 \
      \( -name "apt*" -o -name "deb-systemd-*" -o -name "debconf*" -o -name "*dpkg*" -o -name "update-alternatives" \) \
      -print -quit
  )"
  if [ -n "$unexpected_management_tool" ]; then
    echo "unexpected package-management path: $unexpected_management_tool" >&2
    exit 1
  fi
'

cleanup() {
  status=$?
  trap - EXIT
  if ((status != 0)); then
    if docker inspect "$name" >/dev/null 2>&1; then
      echo "--- OwlMux Server logs ---" >&2
      docker logs "$name" >&2 || true
    fi
    if docker inspect "$postgres_name" >/dev/null 2>&1; then
      echo "--- PostgreSQL logs ---" >&2
      docker logs "$postgres_name" >&2 || true
    fi
  fi
  docker rm --force "$name" "$postgres_name" >/dev/null 2>&1 || true
  docker network rm "$network" >/dev/null 2>&1 || true
  exit "$status"
}
trap cleanup EXIT

docker network create "$network" >/dev/null
docker run --detach --name "$postgres_name" --network "$network" \
  --env POSTGRES_DB=owlmux \
  --env POSTGRES_PASSWORD=owlmux \
  --env POSTGRES_USER=owlmux \
  postgres:17.10-alpine >/dev/null
for _ in {1..30}; do
  if docker exec "$postgres_name" pg_isready --host 127.0.0.1 --port 5432 --username owlmux --dbname owlmux >/dev/null 2>&1; then
    break
  fi
  sleep 1
done
docker exec "$postgres_name" pg_isready --host 127.0.0.1 --port 5432 --username owlmux --dbname owlmux >/dev/null

docker run --detach --name "$name" --network "$network" --publish 127.0.0.1::8080 \
  --env OWLMUX_PUBLIC_ORIGIN=http://127.0.0.1:8080 \
  --env "OWLMUX_DATABASE_URL=postgres://owlmux:owlmux@${postgres_name}:5432/owlmux" \
  --env OWLMUX_API_KEY=owlmux_sk_v1_YWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWE \
  --env OWLMUX_SSH_KEY_ENCRYPTION_KEY=YmJiYmJiYmJiYmJiYmJiYmJiYmJiYmJiYmJiYmJiYmI \
  --env OWLMUX_SSH_RUNTIME_ROOT=/tmp/owlmux-ssh \
  --env OWLMUX_CONFIG_EPOCH=1 \
  --env OWLMUX_NODE_NAME=image-smoke \
  "$image" >/dev/null
port="$(docker port "$name" 8080/tcp | sed -n '1{s/.*://;p;}')"
[[ -n "$port" ]]

ready=false
for _ in {1..30}; do
  ready_response="$(curl --fail --silent "http://127.0.0.1:${port}/ready" 2>/dev/null || true)"
  if grep -q '"status":"ready"' <<<"$ready_response"; then
    ready=true
    break
  fi
  sleep 1
done
[[ "$ready" == true ]]

health_response="$(curl --fail --silent "http://127.0.0.1:${port}/health")"
grep -q '"service":"owlmux-server"' <<<"$health_response"
root_response="$(curl --fail --silent "http://127.0.0.1:${port}/")"
grep -q 'OwlMux' <<<"$root_response"
deployment_response="$(
  curl --fail --silent \
    --header 'Authorization: Bearer owlmux_sk_v1_YWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWE' \
    "http://127.0.0.1:${port}/api/v1/deployment"
)"
grep -Fq "+${image_revision}\"" <<<"$deployment_response"
status="$(curl --silent --output /dev/null --write-out '%{http_code}' "http://127.0.0.1:${port}/api/v1/machines")"
[[ "$status" == "401" ]]
