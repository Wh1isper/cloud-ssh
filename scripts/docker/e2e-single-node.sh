#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$ROOT"

export OWLMUX_COMPOSE_PROJECT=owlmux-e2e
export OWLMUX_POSTGRES_PORT=55434
COMPOSE=(docker compose --file dev/compose.yml --profile target)
TMP=$(mktemp -d)
SERVER_PID=
RELAY_PID=
ROUTE_TEST_PID=
FENCE_TEST_PID=

cleanup() {
  local status=$?
  for pid in "$ROUTE_TEST_PID" "$FENCE_TEST_PID" "$RELAY_PID"; do
    if [[ -n "$pid" ]]; then
      kill "$pid" 2>/dev/null || true
      wait "$pid" 2>/dev/null || true
    fi
  done
  if [[ -n "$SERVER_PID" ]]; then
    kill -INT "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
  fi
  if (( status != 0 )); then
    printf '\nServer log:\n' >&2
    cat "$TMP/server.log" >&2 2>/dev/null || true
    printf '\nRelay log:\n' >&2
    cat "$TMP/relay.log" >&2 2>/dev/null || true
    printf '\nAttachment refresh log:\n' >&2
    cat "$TMP/refresh.log" >&2 2>/dev/null || true
    cat "$TMP/route-replacement.log" >&2 2>/dev/null || true
  fi
  "${COMPOSE[@]}" down --volumes --remove-orphans >/dev/null 2>&1 || true
  rm -rf "$TMP"
  exit "$status"
}
trap cleanup EXIT INT TERM

"${COMPOSE[@]}" down --volumes --remove-orphans >/dev/null 2>&1 || true
"${COMPOSE[@]}" up --detach --build --wait postgres target

target_tmux_version=$("${COMPOSE[@]}" exec -T target /usr/bin/tmux -V | tr -d '\r\n')
if [[ -n "${OWLMUX_EXPECTED_TMUX_VERSION:-}" ]]; then
  [[ "$target_tmux_version" == "$OWLMUX_EXPECTED_TMUX_VERSION" ]]
fi
printf 'Target fixture: %s (%s)\n' "$target_tmux_version" "${OWLMUX_TARGET_BASE_IMAGE:-debian:13-slim}"

API_KEY=owlmux_sk_v1_YWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWE
ENCRYPTION_KEY=YmJiYmJiYmJiYmJiYmJiYmJiYmJiYmJiYmJiYmJiYmI
OWLMUX_ADDR=0.0.0.0:18080 \
OWLMUX_PUBLIC_ORIGIN=http://127.0.0.1:18080 \
OWLMUX_DATABASE_URL=postgres://owlmux:owlmux@127.0.0.1:55434/owlmux \
OWLMUX_API_KEY="$API_KEY" \
OWLMUX_SSH_KEY_ENCRYPTION_KEY="$ENCRYPTION_KEY" \
OWLMUX_SSH_RUNTIME_ROOT="$TMP/ssh" \
OWLMUX_CONFIG_EPOCH=1 \
OWLMUX_NODE_NAME=e2e \
OWLMUX_WEB_DIR=apps/web/dist \
target/debug/owlmux-server >"$TMP/server.log" 2>&1 &
SERVER_PID=$!

for _ in $(seq 1 100); do
  if curl --fail --silent --show-error --max-time 1 http://127.0.0.1:18080/ready >/dev/null 2>&1; then
    break
  fi
  sleep 0.2
done
curl --fail --silent --show-error --max-time 2 http://127.0.0.1:18080/ready >/dev/null
curl --silent --show-error --max-time 2 -D "$TMP/protected.headers" -o /dev/null \
  -H "Authorization: Bearer $API_KEY" http://127.0.0.1:18080/api/v1/deployment
tr -d '\r' <"$TMP/protected.headers" | grep -qi '^cache-control: no-store$'
tr -d '\r' <"$TMP/protected.headers" | grep -qi '^x-frame-options: DENY$'
tr -d '\r' <"$TMP/protected.headers" | grep -qi '^x-content-type-options: nosniff$'
tr -d '\r' <"$TMP/protected.headers" | grep -qi '^referrer-policy: no-referrer$'
tr -d '\r' <"$TMP/protected.headers" | grep -qi '^permissions-policy: camera=(), display-capture=(), geolocation=(), microphone=(), payment=(), usb=()$'
tr -d '\r' <"$TMP/protected.headers" | grep -qi "^content-security-policy: default-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'; object-src 'none'; connect-src 'self'; style-src 'self' 'unsafe-inline'$"
curl --silent --show-error --max-time 2 -D "$TMP/unauthenticated.headers" -o /dev/null \
  http://127.0.0.1:18080/api/v1/deployment
tr -d '\r' <"$TMP/unauthenticated.headers" | grep -q '^HTTP/1.1 401'
tr -d '\r' <"$TMP/unauthenticated.headers" | grep -qi '^cache-control: no-store$'
python3 - <<'PY'
import socket

with socket.create_connection(("127.0.0.1", 18080), timeout=2) as connection:
    connection.settimeout(7)
    connection.sendall(b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\n")
    assert connection.recv(1) == b"", "incomplete HTTP headers were not closed by the read deadline"
PY
curl --fail --silent --show-error --max-time 2 http://127.0.0.1:18080/health >/dev/null
missing_content_type_status=$(curl --silent --show-error --max-time 5 -o /dev/null -w '%{http_code}' \
  -X POST -H "Authorization: Bearer $API_KEY" --data '{"name":"invalid"}' \
  http://127.0.0.1:18080/api/v1/ssh-credentials)
[[ "$missing_content_type_status" == 415 ]]
oversized_body=$(python3 -c 'print("x" * 17000)')
oversized_status=$(curl --silent --show-error --max-time 5 -o /dev/null -w '%{http_code}' \
  -X POST -H "Authorization: Bearer $API_KEY" -H 'Content-Type: application/json' \
  --data "$oversized_body" http://127.0.0.1:18080/api/v1/ssh-credentials)
[[ "$oversized_status" == 413 ]]

credentials=$(curl --fail --silent --show-error --max-time 5 \
  -H "Authorization: Bearer $API_KEY" \
  http://127.0.0.1:18080/api/v1/ssh-credentials)
public_key=$(python3 -c 'import json,sys; print(json.load(sys.stdin)[0]["public_key"])' <<<"$credentials")
printf '%s\n' "$public_key" | "${COMPOSE[@]}" exec -T target sh -c \
  'cat > /home/owlmux/.ssh/authorized_keys && chown owlmux:owlmux /home/owlmux/.ssh/authorized_keys && chmod 0600 /home/owlmux/.ssh/authorized_keys'
host_identity=$("${COMPOSE[@]}" exec -T target cat /etc/ssh/ssh_host_ed25519_key.pub | tr -d '\r\n')

race_credential=$(curl --fail --silent --show-error --max-time 5 \
  -X POST -H "Authorization: Bearer $API_KEY" -H 'Content-Type: application/json' \
  --data '{"name":"Retirement race"}' http://127.0.0.1:18080/api/v1/ssh-credentials)
race_credential_id=$(python3 -c 'import json,sys; print(json.load(sys.stdin)["ssh_credential_id"])' <<<"$race_credential")
race_machine_body=$(python3 -c 'import json,sys; print(json.dumps({"alias":"credential-race","target_account":"owlmux","tmux_path":"/usr/bin/tmux","tmux_socket_identity":"owlmux","host_identity":sys.argv[1],"ssh_credential_id":sys.argv[2]}))' "$host_identity" "$race_credential_id")
curl --silent --show-error --max-time 10 -o "$TMP/race-create.body" -w '%{http_code}' \
  -X POST -H "Authorization: Bearer $API_KEY" -H 'Content-Type: application/json' \
  --data "$race_machine_body" http://127.0.0.1:18080/api/v1/machines >"$TMP/race-create.status" &
race_create_pid=$!
curl --silent --show-error --max-time 10 -o "$TMP/race-retire.body" -w '%{http_code}' \
  -X POST -H "Authorization: Bearer $API_KEY" \
  "http://127.0.0.1:18080/api/v1/ssh-credentials/$race_credential_id/retire" >"$TMP/race-retire.status" &
race_retire_pid=$!
wait "$race_create_pid"
wait "$race_retire_pid"
race_create_status=$(cat "$TMP/race-create.status")
race_retire_status=$(cat "$TMP/race-retire.status")
if [[ "$race_create_status" == 201 ]]; then
  [[ "$race_retire_status" == 409 ]]
else
  [[ "$race_retire_status" == 204 ]]
  [[ "$race_create_status" == 404 || "$race_create_status" == 409 ]]
fi
race_credentials=$(curl --fail --silent --show-error --max-time 5 \
  -H "Authorization: Bearer $API_KEY" http://127.0.0.1:18080/api/v1/ssh-credentials)
race_machines=$(curl --fail --silent --show-error --max-time 5 \
  -H "Authorization: Bearer $API_KEY" http://127.0.0.1:18080/api/v1/machines)
python3 -c 'import json,sys; credentials=json.loads(sys.stdin.read()); credential_id,status=sys.argv[1:]; credential=next(item for item in credentials if item["ssh_credential_id"] == credential_id); assert (status == "201" and credential["status"] == "active" and credential["bound_machine_count"] == 1) or (status != "201" and credential["status"] == "retired" and credential["bound_machine_count"] == 0)' "$race_credential_id" "$race_create_status" <<<"$race_credentials"
python3 -c 'import json,sys; machines=json.loads(sys.stdin.read()); credential_id,status=sys.argv[1:]; bound=[machine for machine in machines if machine["ssh_credential_id"] == credential_id]; assert (status == "201" and len(bound) == 1) or (status != "201" and len(bound) == 0)' "$race_credential_id" "$race_create_status" <<<"$race_machines"

machine_body=$(python3 -c 'import json,sys; print(json.dumps({"alias":"e2e-target","target_account":"owlmux","tmux_path":"/usr/bin/tmux","tmux_socket_identity":"owlmux","host_identity":sys.argv[1]}))' "$host_identity")
created=$(curl --fail --silent --show-error --max-time 5 \
  -X POST -H "Authorization: Bearer $API_KEY" -H 'Content-Type: application/json' \
  --data "$machine_body" http://127.0.0.1:18080/api/v1/machines)
machine_id=$(python3 -c 'import json,sys; print(json.load(sys.stdin)["machine"]["machine_id"])' <<<"$created")
enrollment_token=$(python3 -c 'import json,sys; print(json.load(sys.stdin)["enrollment_token"])' <<<"$created")
OWLMUX_E2E_SERVER=ws://127.0.0.1:18080 \
OWLMUX_E2E_ENROLLMENT_TOKEN="$enrollment_token" \
pnpm --filter @owlmux/web exec node scripts/enrollment-disconnect-smoke.mjs
for _ in $(seq 1 50); do
  machine=$(curl --fail --silent --show-error --max-time 2 \
    -H "Authorization: Bearer $API_KEY" "http://127.0.0.1:18080/api/v1/machines/$machine_id")
  if python3 -c 'import json,sys; raise SystemExit(0 if json.load(sys.stdin)["lifecycle"] == "pending" else 1)' <<<"$machine"; then
    break
  fi
  sleep 0.1
done
python3 -c 'import json,sys; assert json.load(sys.stdin)["lifecycle"] == "pending"' <<<"$machine"
replacement_token_response=$(curl --fail --silent --show-error --max-time 5 -X POST \
  -H "Authorization: Bearer $API_KEY" \
  "http://127.0.0.1:18080/api/v1/machines/$machine_id/enrollment-token")
enrollment_token=$(python3 -c 'import json,sys; print(json.load(sys.stdin)["enrollment_token"])' <<<"$replacement_token_response")

target_container=$("${COMPOSE[@]}" ps --quiet target)
docker cp target/debug/owlmux-relay "$target_container:/usr/local/bin/owlmux-relay"
"${COMPOSE[@]}" exec -T target chmod 0700 /var/lib/owlmux
printf '%s\n' "$enrollment_token" | "${COMPOSE[@]}" exec -T target \
  /usr/local/bin/owlmux-relay enroll \
  --server ws://host.docker.internal:18080 \
  --state /var/lib/owlmux/state.json \
  --account owlmux \
  --confirm-ready

"${COMPOSE[@]}" exec -T target sh -c \
  'echo $$ > /tmp/owlmux-relay.pid; exec /usr/local/bin/owlmux-relay run --server ws://host.docker.internal:18080 --state /var/lib/owlmux/state.json' \
  >"$TMP/relay.log" 2>&1 &
RELAY_PID=$!

for _ in $(seq 1 100); do
  machines=$(curl --fail --silent --show-error --max-time 2 \
    -H "Authorization: Bearer $API_KEY" http://127.0.0.1:18080/api/v1/machines)
  if python3 -c 'import json,sys; machines=json.load(sys.stdin); m=next(item for item in machines if item["machine_id"] == sys.argv[1]); raise SystemExit(0 if m["lifecycle"] == "active" and m["reachability"] == "reachable" else 1)' "$machine_id" <<<"$machines"; then
    break
  fi
  sleep 0.2
done

OWLMUX_E2E_SERVER=ws://127.0.0.1:18080 \
OWLMUX_E2E_MACHINE_ID="$machine_id" \
OWLMUX_E2E_API_KEY="$API_KEY" \
pnpm --filter @owlmux/web exec node scripts/attachment-smoke.mjs

"${COMPOSE[@]}" exec -T --user owlmux target /usr/bin/tmux -L owlmux new-session -d -s owlmux-cutover \
  'i=0; while :; do printf "CUTOVER-%08d\r" "$i"; i=$((i + 1)); sleep 0.005; done'
"${COMPOSE[@]}" exec -T --user owlmux target /usr/bin/tmux -L owlmux resize-window -t owlmux-cutover:0 -x 490 -y 2000
for _ in 1 2 3; do
  "${COMPOSE[@]}" exec -T --user owlmux target /usr/bin/tmux -L owlmux split-window -d -v -t owlmux-cutover:0 \
    'while :; do sleep 3600; done'
done
"${COMPOSE[@]}" exec -T --user owlmux target /usr/bin/tmux -L owlmux select-layout -t owlmux-cutover:0 tiled
OWLMUX_E2E_SERVER=ws://127.0.0.1:18080 \
OWLMUX_E2E_MACHINE_ID="$machine_id" \
OWLMUX_E2E_API_KEY="$API_KEY" \
pnpm --filter @owlmux/web exec node scripts/attachment-cutover-smoke.mjs
"${COMPOSE[@]}" exec -T --user owlmux target /usr/bin/tmux -L owlmux kill-session -t owlmux-cutover

"${COMPOSE[@]}" exec -T --user owlmux target /usr/bin/tmux -L owlmux new-session -d -s owlmux-live-output \
  'block=xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx; stty -echo; while IFS= read -r line; do if [ "$line" = 0 ]; then printf "\033]2;owlmux-live-ready\007\377SYNC\n"; continue; fi; i=0; while [ "$i" -lt 64 ]; do printf "%s" "$block"; i=$((i + 1)); done; printf "\377LIVE-%s\n" "$line"; done'
for _ in $(seq 1 100); do
  live_command=$("${COMPOSE[@]}" exec -T --user owlmux target /usr/bin/tmux -L owlmux display-message -p -t owlmux-live-output:0.0 '#{pane_current_command}')
  [[ $live_command != stty ]] && break
  sleep 0.1
done
[[ $live_command != stty ]]
OWLMUX_E2E_SERVER=ws://127.0.0.1:18080 \
OWLMUX_E2E_MACHINE_ID="$machine_id" \
OWLMUX_E2E_API_KEY="$API_KEY" \
pnpm --filter @owlmux/web exec node scripts/attachment-live-output.mjs
"${COMPOSE[@]}" exec -T --user owlmux target /usr/bin/tmux -L owlmux kill-session -t owlmux-live-output

OWLMUX_E2E_SERVER=ws://127.0.0.1:18080 \
OWLMUX_E2E_MACHINE_ID="$machine_id" \
OWLMUX_E2E_API_KEY="$API_KEY" \
pnpm --filter @owlmux/web exec node scripts/attachment-route-replacement.mjs >"$TMP/route-replacement.log" 2>&1 &
ROUTE_TEST_PID=$!
for _ in $(seq 1 100); do
  grep -q '^workspace-ready$' "$TMP/route-replacement.log" 2>/dev/null && break
  sleep 0.1
done
grep -q '^workspace-ready$' "$TMP/route-replacement.log"
old_route_epoch=$("${COMPOSE[@]}" exec -T postgres psql --username owlmux --dbname owlmux --tuples-only --no-align \
  --command "SELECT connection_epoch FROM machine_owners WHERE machine_id = '$machine_id'")
old_relay_pid=$RELAY_PID
"${COMPOSE[@]}" exec -T target sh -c 'kill -TERM "$(cat /tmp/owlmux-relay.pid)"'
wait "$old_relay_pid" 2>/dev/null || true
RELAY_PID=
"${COMPOSE[@]}" exec -T target sh -c \
  'echo $$ > /tmp/owlmux-relay.pid; exec /usr/local/bin/owlmux-relay run --server ws://host.docker.internal:18080 --state /var/lib/owlmux/state.json' \
  >>"$TMP/relay.log" 2>&1 &
RELAY_PID=$!
wait "$ROUTE_TEST_PID"
ROUTE_TEST_PID=
cat "$TMP/route-replacement.log"
for _ in $(seq 1 100); do
  machine=$(curl --fail --silent --show-error --max-time 2 \
    -H "Authorization: Bearer $API_KEY" "http://127.0.0.1:18080/api/v1/machines/$machine_id")
  if python3 -c 'import json,sys; m=json.load(sys.stdin); raise SystemExit(0 if m["lifecycle"] == "active" and m["reachability"] == "reachable" else 1)' <<<"$machine"; then
    break
  fi
  sleep 0.1
done
OWLMUX_E2E_SERVER=ws://127.0.0.1:18080 \
OWLMUX_E2E_MACHINE_ID="$machine_id" \
OWLMUX_E2E_API_KEY="$API_KEY" \
pnpm --filter @owlmux/web exec node scripts/attachment-smoke.mjs
replacement_epoch=$("${COMPOSE[@]}" exec -T postgres psql --username owlmux --dbname owlmux --tuples-only --no-align \
  --command "SELECT connection_epoch FROM machine_owners WHERE machine_id = '$machine_id'")
(( replacement_epoch > old_route_epoch ))

"${COMPOSE[@]}" exec -T target su - owlmux -c '/usr/bin/tmux -L owlmux has-session -t alpha'
first_epoch=$replacement_epoch
curl --fail --silent --show-error --max-time 5 -X POST \
  -H "Authorization: Bearer $API_KEY" \
  "http://127.0.0.1:18080/api/v1/machines/$machine_id/re-enroll" >/dev/null
machine=$(curl --fail --silent --show-error --max-time 5 \
  -H "Authorization: Bearer $API_KEY" \
  "http://127.0.0.1:18080/api/v1/machines/$machine_id")
python3 -c 'import json,sys; m=json.load(sys.stdin); assert m["lifecycle"] == "pending" and m["reachability"] == "unknown"' <<<"$machine"

"${COMPOSE[@]}" exec -T target sh -c 'kill -TERM "$(cat /tmp/owlmux-relay.pid)"' 2>/dev/null || true
wait "$RELAY_PID" 2>/dev/null || true
RELAY_PID=
"${COMPOSE[@]}" exec -T target /usr/local/bin/owlmux-relay reset --state /var/lib/owlmux/state.json
new_token_response=$(curl --fail --silent --show-error --max-time 5 -X POST \
  -H "Authorization: Bearer $API_KEY" \
  "http://127.0.0.1:18080/api/v1/machines/$machine_id/enrollment-token")
new_enrollment_token=$(python3 -c 'import json,sys; print(json.load(sys.stdin)["enrollment_token"])' <<<"$new_token_response")
printf '%s\n' "$new_enrollment_token" | "${COMPOSE[@]}" exec -T target \
  /usr/local/bin/owlmux-relay enroll \
  --server ws://host.docker.internal:18080 \
  --state /var/lib/owlmux/state.json \
  --account owlmux \
  --confirm-ready
"${COMPOSE[@]}" exec -T target sh -c \
  'echo $$ > /tmp/owlmux-relay.pid; exec /usr/local/bin/owlmux-relay run --server ws://host.docker.internal:18080 --state /var/lib/owlmux/state.json' \
  >>"$TMP/relay.log" 2>&1 &
RELAY_PID=$!
for _ in $(seq 1 100); do
  machine=$(curl --fail --silent --show-error --max-time 2 \
    -H "Authorization: Bearer $API_KEY" "http://127.0.0.1:18080/api/v1/machines/$machine_id")
  if python3 -c 'import json,sys; m=json.load(sys.stdin); raise SystemExit(0 if m["lifecycle"] == "active" and m["reachability"] == "reachable" else 1)' <<<"$machine"; then
    break
  fi
  sleep 0.2
done
second_epoch=$("${COMPOSE[@]}" exec -T postgres psql --username owlmux --dbname owlmux --tuples-only --no-align \
  --command "SELECT connection_epoch FROM machine_owners WHERE machine_id = '$machine_id'")
(( second_epoch > first_epoch ))
OWLMUX_E2E_SERVER=ws://127.0.0.1:18080 \
OWLMUX_E2E_MACHINE_ID="$machine_id" \
OWLMUX_E2E_API_KEY="$API_KEY" \
pnpm --filter @owlmux/web exec node scripts/attachment-smoke.mjs
xss_machine_body=$(python3 -c 'import json,sys; print(json.dumps({"alias":"<img src=x onerror=globalThis.owlmuxXss=true>","target_account":"owlmux","tmux_path":"/usr/bin/tmux","tmux_socket_identity":"xss-fixture","host_identity":sys.argv[1]}))' "$host_identity")
curl --fail --silent --show-error --max-time 5 \
  -X POST -H "Authorization: Bearer $API_KEY" -H 'Content-Type: application/json' \
  --data "$xss_machine_body" http://127.0.0.1:18080/api/v1/machines >/dev/null
OWLMUX_E2E_HTTP_SERVER=http://127.0.0.1:18080 \
OWLMUX_E2E_API_KEY="$API_KEY" \
pnpm --filter @owlmux/web exec node scripts/browser-workspace-smoke.mjs
OWLMUX_E2E_SERVER=ws://127.0.0.1:18080 \
OWLMUX_E2E_MACHINE_ID="$machine_id" \
OWLMUX_E2E_API_KEY="$API_KEY" \
pnpm --filter @owlmux/web exec node scripts/attachment-interactive.mjs
"${COMPOSE[@]}" exec -T target test ! -e /tmp/owlmux-should-not-dispatch
"${COMPOSE[@]}" exec -T --user owlmux target /usr/bin/tmux -L owlmux kill-session -t owlmux-interactive
for _ in $(seq 1 100); do
  machine=$(curl --fail --silent --show-error --max-time 2 \
    -H "Authorization: Bearer $API_KEY" "http://127.0.0.1:18080/api/v1/machines/$machine_id")
  if python3 -c 'import json,sys; m=json.load(sys.stdin); raise SystemExit(0 if m["lifecycle"] == "active" and m["reachability"] == "reachable" else 1)' <<<"$machine"; then
    break
  fi
  sleep 0.1
done
python3 -c 'import json,sys; m=json.load(sys.stdin); assert m["lifecycle"] == "active" and m["reachability"] == "reachable"' <<<"$machine"
"${COMPOSE[@]}" exec -T --user owlmux target /usr/bin/tmux -L owlmux has-session -t alpha
refresh_session=owlmux-refresh
"${COMPOSE[@]}" exec -T --user owlmux target /usr/bin/tmux -L owlmux new-session -d -s "$refresh_session" \
  'printf "refresh-primary-ready"; while :; do sleep 3600; done'
"${COMPOSE[@]}" exec -T --user owlmux target /usr/bin/tmux -L owlmux split-window -d -t "$refresh_session:0" \
  'printf "refresh-secondary-ready"; while :; do sleep 3600; done'
"${COMPOSE[@]}" exec -T --user owlmux target /usr/bin/tmux -L owlmux select-layout -t "$refresh_session:0" even-horizontal

OWLMUX_E2E_SERVER=ws://127.0.0.1:18080 \
OWLMUX_E2E_MACHINE_ID="$machine_id" \
OWLMUX_E2E_API_KEY="$API_KEY" \
OWLMUX_E2E_SESSION_NAME="$refresh_session" \
pnpm --filter @owlmux/web exec node scripts/attachment-refresh-smoke.mjs >"$TMP/refresh.log" 2>&1 &
ROUTE_TEST_PID=$!
for _ in $(seq 1 100); do
  grep -q '^workspace-ready$' "$TMP/refresh.log" 2>/dev/null && break
  sleep 0.1
done
grep -q '^workspace-ready$' "$TMP/refresh.log"
replacement_credential=$(curl --fail --silent --show-error --max-time 5 \
  -X POST -H "Authorization: Bearer $API_KEY" -H 'Content-Type: application/json' \
  --data '{"name":"Replacement"}' http://127.0.0.1:18080/api/v1/ssh-credentials)
replacement_credential_id=$(python3 -c 'import json,sys; print(json.load(sys.stdin)["ssh_credential_id"])' <<<"$replacement_credential")
replacement_public_key=$(python3 -c 'import json,sys; print(json.load(sys.stdin)["public_key"])' <<<"$replacement_credential")
printf '%s\n%s\n' "$public_key" "$replacement_public_key" | "${COMPOSE[@]}" exec -T target sh -c \
  'cat > /home/owlmux/.ssh/authorized_keys && chown owlmux:owlmux /home/owlmux/.ssh/authorized_keys && chmod 0600 /home/owlmux/.ssh/authorized_keys'
route_before_rebind=$("${COMPOSE[@]}" exec -T postgres psql --username owlmux --dbname owlmux --tuples-only --no-align \
  --command "SELECT route_revision FROM machines WHERE id = '$machine_id'")
credential_revision_before=$("${COMPOSE[@]}" exec -T postgres psql --username owlmux --dbname owlmux --tuples-only --no-align \
  --command "SELECT credential_revision FROM machines WHERE id = '$machine_id'")
curl --fail --silent --show-error --max-time 5 -X PATCH \
  -H "Authorization: Bearer $API_KEY" -H 'Content-Type: application/json' \
  --data '{"alias":"renamed-target"}' \
  "http://127.0.0.1:18080/api/v1/machines/$machine_id" >/dev/null
rebind_body=$(python3 -c 'import json,sys; print(json.dumps({"ssh_credential_id":sys.argv[1]}))' "$replacement_credential_id")
curl --fail --silent --show-error --max-time 5 -X PATCH \
  -H "Authorization: Bearer $API_KEY" -H 'Content-Type: application/json' \
  --data "$rebind_body" \
  "http://127.0.0.1:18080/api/v1/machines/$machine_id/ssh-credential" >/dev/null
printf '%s\n' "$replacement_public_key" | "${COMPOSE[@]}" exec -T target sh -c \
  'cat > /home/owlmux/.ssh/authorized_keys && chown owlmux:owlmux /home/owlmux/.ssh/authorized_keys && chmod 0600 /home/owlmux/.ssh/authorized_keys'
machine=$(curl --fail --silent --show-error --max-time 5 \
  -H "Authorization: Bearer $API_KEY" "http://127.0.0.1:18080/api/v1/machines/$machine_id")
python3 -c 'import json,sys; m=json.load(sys.stdin); assert set(m) == {"machine_id","ssh_credential_id","alias","lifecycle","reachability"}; assert m["alias"] == "renamed-target" and m["ssh_credential_id"] == sys.argv[1] and m["lifecycle"] == "active"' "$replacement_credential_id" <<<"$machine"
route_after_rebind=$("${COMPOSE[@]}" exec -T postgres psql --username owlmux --dbname owlmux --tuples-only --no-align \
  --command "SELECT route_revision FROM machines WHERE id = '$machine_id'")
credential_revision_after=$("${COMPOSE[@]}" exec -T postgres psql --username owlmux --dbname owlmux --tuples-only --no-align \
  --command "SELECT credential_revision FROM machines WHERE id = '$machine_id'")
[[ "$route_after_rebind" == "$route_before_rebind" ]]
(( credential_revision_after == credential_revision_before + 1 ))
audit_events=$(curl --fail --silent --show-error --max-time 5 \
  -H "Authorization: Bearer $API_KEY" http://127.0.0.1:18080/api/v1/audit-events)
python3 -c 'import json,sys; events=json.load(sys.stdin); actions={event["action"] for event in events}; expected={"rename","rebind","attachment_start","attachment_end","ssh_tmux_probe","ssh_tmux_control","writer_takeover","tmux_session_create"}; assert expected <= actions, sorted(expected-actions); assert all(set(event) <= {"audit_event_id","resource_kind","machine_id","ssh_credential_id","action","outcome_class","occurred_at"} for event in events)' <<<"$audit_events"
metrics=$(curl --fail --silent --show-error --max-time 5 \
  -H "Authorization: Bearer $API_KEY" http://127.0.0.1:18080/api/v1/metrics)
python3 -c 'import json,sys; metrics=json.load(sys.stdin); assert metrics["node_ready"] is True and metrics["api_authenticated_requests_total"] > 0; assert all(isinstance(value, (bool,int)) for value in metrics.values())' <<<"$metrics"
"${COMPOSE[@]}" exec -T --user owlmux target /usr/bin/tmux -L owlmux split-window -d -t "$refresh_session:0" \
  "printf 'tertiary-ready'; while :; do sleep 3600; done"
for _ in $(seq 1 100); do
  grep -q '^projection-refreshed$' "$TMP/refresh.log" 2>/dev/null && break
  sleep 0.1
done
grep -q '^projection-refreshed$' "$TMP/refresh.log"
"${COMPOSE[@]}" exec -T --user owlmux target /usr/bin/tmux -L owlmux kill-server
wait "$ROUTE_TEST_PID"
ROUTE_TEST_PID=
cat "$TMP/refresh.log"
OWLMUX_E2E_SERVER=ws://127.0.0.1:18080 \
OWLMUX_E2E_MACHINE_ID="$machine_id" \
OWLMUX_E2E_API_KEY="$API_KEY" \
pnpm --filter @owlmux/web exec node scripts/attachment-zero-sessions.mjs
"${COMPOSE[@]}" exec -T --user owlmux target /usr/bin/tmux -L owlmux new-session -d -s alpha \
  "printf 'primary-ready\\377'; while :; do if [ -f /tmp/owlmux-live-output ]; then cat /tmp/owlmux-live-output; rm -f /tmp/owlmux-live-output; fi; sleep 1; done"
"${COMPOSE[@]}" exec -T --user owlmux target /usr/bin/tmux -L owlmux split-window -d -t alpha:0 \
  "printf 'secondary-ready'; while :; do sleep 3600; done"
"${COMPOSE[@]}" exec -T --user owlmux target /usr/bin/tmux -L owlmux select-layout -t alpha:0 even-horizontal

OWLMUX_E2E_SERVER=ws://127.0.0.1:18080 \
OWLMUX_E2E_MACHINE_ID="$machine_id" \
OWLMUX_E2E_API_KEY="$API_KEY" \
pnpm --filter @owlmux/web exec node scripts/attachment-fence-smoke.mjs >"$TMP/fence.log" 2>&1 &
FENCE_TEST_PID=$!
for _ in $(seq 1 100); do
  grep -q '^workspace-ready$' "$TMP/fence.log" 2>/dev/null && break
  sleep 0.1
done
grep -q '^workspace-ready$' "$TMP/fence.log"
"${COMPOSE[@]}" stop postgres >/dev/null
ready_status=200
for _ in $(seq 1 100); do
  ready_status=$(curl --silent --output /dev/null --write-out '%{http_code}' --max-time 1 \
    http://127.0.0.1:18080/ready || true)
  [[ "$ready_status" == 503 ]] && break
  sleep 0.2
done
[[ "$ready_status" == 503 ]]
wait "$FENCE_TEST_PID"
FENCE_TEST_PID=
cat "$TMP/fence.log"
wait "$SERVER_PID" 2>/dev/null || true
SERVER_PID=
"${COMPOSE[@]}" exec -T target su - owlmux -c '/usr/bin/tmux -L owlmux has-session -t alpha'
"${COMPOSE[@]}" exec -T target sh -c 'kill -TERM "$(cat /tmp/owlmux-relay.pid)"' 2>/dev/null || true
wait "$RELAY_PID" 2>/dev/null || true
RELAY_PID=
"${COMPOSE[@]}" exec -T target su - owlmux -c '/usr/bin/tmux -L owlmux has-session -t alpha'
printf 'Single-node Docker E2E passed: enrollment recovery, owner claim, credential locking/rebind, safe presentation/audit/metrics, HTTP header/body/readiness bounds, Browser headers/XSS/navigation/logout/unknown-outcome refresh, Chromium/xterm projection, continuous snapshot/live cutover, binary live output, one writer, session creation, literal input, takeover, authoritative resize, projection refresh, route replacement, hard fencing, and target tmux survival.\n'
