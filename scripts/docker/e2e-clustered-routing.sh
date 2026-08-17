#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$ROOT"

export OWLMUX_COMPOSE_PROJECT=owlmux-cluster-e2e
export OWLMUX_POSTGRES_PORT=55435
COMPOSE=(docker compose --file dev/compose.yml --profile target)
TMP=$(mktemp -d)
NODE_A_PID=
NODE_B_PID=
RELAY_PID=
RACE_RELAY_PID=

cleanup() {
  local status=$?
  if [[ -n "$RELAY_PID" ]]; then
    "${COMPOSE[@]}" exec -T target sh -c 'kill -TERM "$(cat /tmp/owlmux-cluster-relay.pid)"' 2>/dev/null || true
    kill "$RELAY_PID" 2>/dev/null || true
    wait "$RELAY_PID" 2>/dev/null || true
  fi
  if [[ -n "$RACE_RELAY_PID" ]]; then
    "${COMPOSE[@]}" exec -T target sh -c 'kill -TERM "$(cat /tmp/owlmux-cluster-race-relay.pid)"' 2>/dev/null || true
    kill "$RACE_RELAY_PID" 2>/dev/null || true
    wait "$RACE_RELAY_PID" 2>/dev/null || true
  fi
  for pid in "$NODE_A_PID" "$NODE_B_PID"; do
    if [[ -n "$pid" ]]; then
      kill -INT "$pid" 2>/dev/null || true
      wait "$pid" 2>/dev/null || true
    fi
  done
  if (( status != 0 )); then
    for log in "$TMP"/*.log; do
      [[ -e "$log" ]] || continue
      printf '\n%s:\n' "$(basename "$log")" >&2
      cat "$log" >&2 || true
    done
  fi
  "${COMPOSE[@]}" down --volumes --remove-orphans >/dev/null 2>&1 || true
  rm -rf "$TMP"
  exit "$status"
}
trap cleanup EXIT INT TERM

"${COMPOSE[@]}" down --volumes --remove-orphans >/dev/null 2>&1 || true
"${COMPOSE[@]}" up --detach --build --wait postgres target

API_KEY=owlmux_sk_v1_YWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWE
CONFIG_EPOCH=1
ENCRYPTION_KEY=YmJiYmJiYmJiYmJiYmJiYmJiYmJiYmJiYmJiYmJiYmI
CLUSTER_KEY=Y2NjY2NjY2NjY2NjY2NjY2NjY2NjY2NjY2NjY2NjY2M
PUBLIC_ORIGIN=http://owlmux.test
DATABASE_URL=postgres://owlmux:owlmux@127.0.0.1:55435/owlmux

openssl req -x509 -newkey rsa:2048 -nodes -days 1 -sha256 \
  -subj '/CN=OwlMux test CA' -keyout "$TMP/ca.key" -out "$TMP/ca.crt" >/dev/null 2>&1
for node in a b c; do
  openssl req -newkey rsa:2048 -nodes -sha256 -subj "/CN=node-$node" \
    -addext 'subjectAltName=IP:127.0.0.1' \
    -keyout "$TMP/node-$node.key" -out "$TMP/node-$node.csr" >/dev/null 2>&1
  printf 'subjectAltName=IP:127.0.0.1\nextendedKeyUsage=serverAuth\n' >"$TMP/node-$node.ext"
  openssl x509 -req -days 1 -sha256 -in "$TMP/node-$node.csr" \
    -CA "$TMP/ca.crt" -CAkey "$TMP/ca.key" -CAcreateserial \
    -extfile "$TMP/node-$node.ext" -out "$TMP/node-$node.crt" >/dev/null 2>&1
done
openssl req -newkey rsa:2048 -nodes -sha256 -subj '/CN=invalid-owner' \
  -addext 'subjectAltName=DNS:invalid-owner.test' \
  -keyout "$TMP/invalid-owner.key" -out "$TMP/invalid-owner.csr" >/dev/null 2>&1
printf 'subjectAltName=DNS:invalid-owner.test\nextendedKeyUsage=serverAuth\n' >"$TMP/invalid-owner.ext"
openssl x509 -req -days 1 -sha256 -in "$TMP/invalid-owner.csr" \
  -CA "$TMP/ca.crt" -CAkey "$TMP/ca.key" -CAcreateserial \
  -extfile "$TMP/invalid-owner.ext" -out "$TMP/invalid-owner.crt" >/dev/null 2>&1

start_node() {
  local node=$1 public_port=$2 internal_port=$3 log=$4
  OWLMUX_ADDR="0.0.0.0:$public_port" \
  OWLMUX_PUBLIC_ORIGIN="$PUBLIC_ORIGIN" \
  OWLMUX_DATABASE_URL="$DATABASE_URL" \
  OWLMUX_API_KEY="$API_KEY" \
  OWLMUX_SSH_KEY_ENCRYPTION_KEY="$ENCRYPTION_KEY" \
  OWLMUX_SSH_RUNTIME_ROOT="$TMP/ssh-$node-$(date +%s%N)" \
  OWLMUX_CONFIG_EPOCH="$CONFIG_EPOCH" \
  OWLMUX_NODE_LEASE_TTL_SECONDS=6 \
  OWLMUX_NODE_LEASE_SAFETY_MARGIN_SECONDS=2 \
  OWLMUX_SHUTDOWN_TIMEOUT_SECONDS=5 \
  OWLMUX_NODE_NAME="node-$node" \
  OWLMUX_PROFILE=clustered \
  OWLMUX_CLUSTER_KEY="$CLUSTER_KEY" \
  OWLMUX_INTERNAL_ADDR="127.0.0.1:$internal_port" \
  OWLMUX_INTERNAL_URL="wss://127.0.0.1:$internal_port/internal/v1/owner" \
  OWLMUX_INTERNAL_TLS_CERT="$TMP/node-$node.crt" \
  OWLMUX_INTERNAL_TLS_KEY="$TMP/node-$node.key" \
  OWLMUX_INTERNAL_TLS_CA="$TMP/ca.crt" \
  OWLMUX_WEB_DIR=apps/web/dist \
  target/debug/owlmux-server >"$log" 2>&1 &
  LAST_PID=$!
}

wait_ready() {
  local port=$1
  for _ in $(seq 1 100); do
    if curl --fail --silent --max-time 1 "http://127.0.0.1:$port/ready" >/dev/null 2>&1; then
      return
    fi
    sleep 0.2
  done
  curl --fail --silent --show-error --max-time 2 "http://127.0.0.1:$port/ready" >/dev/null
}

psql_value() {
  "${COMPOSE[@]}" exec -T postgres psql --username owlmux --dbname owlmux \
    --tuples-only --no-align --command "$1" | tr -d '[:space:]'
}

start_node a 18080 19443 "$TMP/node-a.log"
NODE_A_PID=$LAST_PID
wait_ready 18080

deployment=$(curl --fail --silent --show-error --max-time 5 \
  -H "Authorization: Bearer $API_KEY" http://127.0.0.1:18080/api/v1/deployment)
python3 -c 'import json,sys; assert json.load(sys.stdin)["profile"] == "clustered"' <<<"$deployment"
credentials=$(curl --fail --silent --show-error --max-time 5 \
  -H "Authorization: Bearer $API_KEY" http://127.0.0.1:18080/api/v1/ssh-credentials)
public_key=$(python3 -c 'import json,sys; print(json.load(sys.stdin)[0]["public_key"])' <<<"$credentials")
printf '%s\n' "$public_key" | "${COMPOSE[@]}" exec -T target sh -c \
  'cat > /home/owlmux/.ssh/authorized_keys && chown owlmux:owlmux /home/owlmux/.ssh/authorized_keys && chmod 0600 /home/owlmux/.ssh/authorized_keys'
host_identity=$("${COMPOSE[@]}" exec -T target cat /etc/ssh/ssh_host_ed25519_key.pub | tr -d '\r\n')
machine_body=$(python3 -c 'import json,sys; print(json.dumps({"alias":"cluster-target","target_account":"owlmux","tmux_path":"/usr/bin/tmux","tmux_socket_identity":"owlmux","host_identity":sys.argv[1]}))' "$host_identity")
created=$(curl --fail --silent --show-error --max-time 5 \
  -X POST -H "Authorization: Bearer $API_KEY" -H 'Content-Type: application/json' \
  --data "$machine_body" http://127.0.0.1:18080/api/v1/machines)
machine_id=$(python3 -c 'import json,sys; print(json.load(sys.stdin)["machine"]["machine_id"])' <<<"$created")
enrollment_token=$(python3 -c 'import json,sys; print(json.load(sys.stdin)["enrollment_token"])' <<<"$created")
target_container=$("${COMPOSE[@]}" ps --quiet target)
docker cp target/debug/owlmux-relay "$target_container:/usr/local/bin/owlmux-relay"
"${COMPOSE[@]}" exec -T target chmod 0700 /var/lib/owlmux
printf '%s\n' "$enrollment_token" | "${COMPOSE[@]}" exec -T target \
  /usr/local/bin/owlmux-relay enroll --server ws://host.docker.internal:18080 \
  --state /var/lib/owlmux/state.json --account owlmux --confirm-ready
"${COMPOSE[@]}" exec -T target sh -c \
  'echo $$ > /tmp/owlmux-cluster-relay.pid; exec /usr/local/bin/owlmux-relay run --server ws://host.docker.internal:18080 --state /var/lib/owlmux/state.json' \
  >"$TMP/relay.log" 2>&1 &
RELAY_PID=$!
for _ in $(seq 1 100); do
  machine=$(curl --fail --silent --show-error --max-time 2 \
    -H "Authorization: Bearer $API_KEY" "http://127.0.0.1:18080/api/v1/machines/$machine_id")
  if python3 -c 'import json,sys; m=json.load(sys.stdin); raise SystemExit(0 if m["lifecycle"] == "active" and m["reachability"] == "reachable" else 1)' <<<"$machine"; then
    break
  fi
  sleep 0.2
done
owner_a=$(psql_value "SELECT owner_incarnation_id FROM machine_owners WHERE machine_id = '$machine_id';")
epoch_a=$(psql_value "SELECT connection_epoch FROM machine_owners WHERE machine_id = '$machine_id';")
[[ -n "$owner_a" && "$epoch_a" -gt 0 ]]

start_node b 18081 19444 "$TMP/node-b.log"
NODE_B_PID=$LAST_PID
wait_ready 18081
owner_after_join=$(psql_value "SELECT owner_incarnation_id FROM machine_owners WHERE machine_id = '$machine_id';")
[[ "$owner_after_join" == "$owner_a" ]]
OWLMUX_E2E_SERVER=ws://127.0.0.1:18081 \
OWLMUX_E2E_ORIGIN="$PUBLIC_ORIGIN" \
OWLMUX_E2E_MACHINE_ID="$machine_id" \
OWLMUX_E2E_API_KEY="$API_KEY" \
pnpm --filter @owlmux/web exec node scripts/attachment-smoke.mjs

original_url=$(psql_value "SELECT internal_wss_url FROM server_nodes WHERE incarnation_id = '$owner_a';")
"${COMPOSE[@]}" exec -T postgres psql --username owlmux --dbname owlmux --command \
  "UPDATE server_nodes SET internal_wss_url = 'wss://127.0.0.1:19999/internal/v1/owner' WHERE incarnation_id = '$owner_a';" >/dev/null
OWLMUX_E2E_SERVER=ws://127.0.0.1:18081 \
OWLMUX_E2E_ORIGIN="$PUBLIC_ORIGIN" \
OWLMUX_E2E_MACHINE_ID="$machine_id" \
OWLMUX_E2E_API_KEY="$API_KEY" \
pnpm --filter @owlmux/web exec node scripts/attachment-owner-unreachable.mjs
"${COMPOSE[@]}" exec -T postgres psql --username owlmux --dbname owlmux --command \
  "UPDATE server_nodes SET internal_wss_url = 'wss://localhost:19443/internal/v1/owner' WHERE incarnation_id = '$owner_a';" >/dev/null
OWLMUX_E2E_SERVER=ws://127.0.0.1:18081 \
OWLMUX_E2E_ORIGIN="$PUBLIC_ORIGIN" \
OWLMUX_E2E_MACHINE_ID="$machine_id" \
OWLMUX_E2E_API_KEY="$API_KEY" \
pnpm --filter @owlmux/web exec node scripts/attachment-owner-unreachable.mjs
"${COMPOSE[@]}" exec -T postgres psql --username owlmux --dbname owlmux --command \
  "UPDATE server_nodes SET internal_wss_url = '$original_url' WHERE incarnation_id = '$owner_a';" >/dev/null

kill -KILL "$NODE_A_PID"
wait "$NODE_A_PID" 2>/dev/null || true
NODE_A_PID=
"${COMPOSE[@]}" exec -T target sh -c 'kill -TERM "$(cat /tmp/owlmux-cluster-relay.pid)"' 2>/dev/null || true
wait "$RELAY_PID" 2>/dev/null || true
RELAY_PID=
"${COMPOSE[@]}" exec -T target su - owlmux -c '/usr/bin/tmux -L owlmux has-session -t alpha'
"${COMPOSE[@]}" exec -T target sh -c \
  'echo $$ > /tmp/owlmux-cluster-relay.pid; exec /usr/local/bin/owlmux-relay run --server ws://host.docker.internal:18081 --state /var/lib/owlmux/state.json' \
  >>"$TMP/relay.log" 2>&1 &
RELAY_PID=$!
for _ in $(seq 1 150); do
  owner_b=$(psql_value "SELECT owner_incarnation_id FROM machine_owners WHERE machine_id = '$machine_id';")
  epoch_b=$(psql_value "SELECT connection_epoch FROM machine_owners WHERE machine_id = '$machine_id';")
  if [[ -n "$owner_b" && "$owner_b" != "$owner_a" && "$epoch_b" -gt "$epoch_a" ]]; then
    break
  fi
  sleep 0.2
done
[[ -n "$owner_b" && "$owner_b" != "$owner_a" && "$epoch_b" -gt "$epoch_a" ]]
OWLMUX_E2E_SERVER=ws://127.0.0.1:18081 \
OWLMUX_E2E_ORIGIN="$PUBLIC_ORIGIN" \
OWLMUX_E2E_MACHINE_ID="$machine_id" \
OWLMUX_E2E_API_KEY="$API_KEY" \
pnpm --filter @owlmux/web exec node scripts/attachment-smoke.mjs

start_node a 18080 19443 "$TMP/node-a-restarted.log"
NODE_A_PID=$LAST_PID
wait_ready 18080
owner_after_restart=$(psql_value "SELECT owner_incarnation_id FROM machine_owners WHERE machine_id = '$machine_id';")
[[ "$owner_after_restart" == "$owner_b" ]]
OWLMUX_E2E_SERVER=ws://127.0.0.1:18080 \
OWLMUX_E2E_ORIGIN="$PUBLIC_ORIGIN" \
OWLMUX_E2E_MACHINE_ID="$machine_id" \
OWLMUX_E2E_API_KEY="$API_KEY" \
pnpm --filter @owlmux/web exec node scripts/attachment-smoke.mjs

node_count_before=$(psql_value 'SELECT count(*) FROM server_nodes;')
set +e
OWLMUX_ADDR=127.0.0.1:18082 \
OWLMUX_PUBLIC_ORIGIN="$PUBLIC_ORIGIN" \
OWLMUX_DATABASE_URL="$DATABASE_URL" \
OWLMUX_API_KEY="$API_KEY" \
OWLMUX_SSH_KEY_ENCRYPTION_KEY="$ENCRYPTION_KEY" \
OWLMUX_SSH_RUNTIME_ROOT="$TMP/ssh-c" \
OWLMUX_CONFIG_EPOCH=1 \
OWLMUX_PROFILE=clustered \
OWLMUX_CLUSTER_KEY=ZGRkZGRkZGRkZGRkZGRkZGRkZGRkZGRkZGRkZGRkZGQ \
OWLMUX_INTERNAL_ADDR=127.0.0.1:19445 \
OWLMUX_INTERNAL_URL=wss://127.0.0.1:19445/internal/v1/owner \
OWLMUX_INTERNAL_TLS_CERT="$TMP/node-c.crt" \
OWLMUX_INTERNAL_TLS_KEY="$TMP/node-c.key" \
OWLMUX_INTERNAL_TLS_CA="$TMP/ca.crt" \
OWLMUX_WEB_DIR=apps/web/dist \
timeout 10 target/debug/owlmux-server >"$TMP/mismatched-node.log" 2>&1
mismatch_status=$?
set -e
[[ "$mismatch_status" -ne 0 ]]
node_count_after=$(psql_value 'SELECT count(*) FROM server_nodes;')
[[ "$node_count_after" == "$node_count_before" ]]

set +e
OWLMUX_ADDR=127.0.0.1:18083 \
OWLMUX_PUBLIC_ORIGIN="$PUBLIC_ORIGIN" \
OWLMUX_DATABASE_URL="$DATABASE_URL" \
OWLMUX_API_KEY="$API_KEY" \
OWLMUX_SSH_KEY_ENCRYPTION_KEY="$ENCRYPTION_KEY" \
OWLMUX_SSH_RUNTIME_ROOT="$TMP/ssh-invalid" \
OWLMUX_CONFIG_EPOCH=1 \
OWLMUX_PROFILE=clustered \
OWLMUX_CLUSTER_KEY="$CLUSTER_KEY" \
OWLMUX_INTERNAL_ADDR=127.0.0.1:19446 \
OWLMUX_INTERNAL_URL=wss://127.0.0.1:19446/internal/v1/owner \
OWLMUX_INTERNAL_TLS_CERT="$TMP/invalid-owner.crt" \
OWLMUX_INTERNAL_TLS_KEY="$TMP/invalid-owner.key" \
OWLMUX_INTERNAL_TLS_CA="$TMP/ca.crt" \
OWLMUX_WEB_DIR=apps/web/dist \
timeout 10 target/debug/owlmux-server >"$TMP/invalid-identity-node.log" 2>&1
identity_status=$?
set -e
[[ "$identity_status" -ne 0 ]]
[[ "$(psql_value 'SELECT count(*) FROM server_nodes;')" == "$node_count_before" ]]

"${COMPOSE[@]}" exec -T postgres psql --username owlmux --dbname owlmux --command \
  "BEGIN; SELECT id FROM machines WHERE id = '$machine_id' FOR UPDATE; SELECT pg_sleep(2); COMMIT;" \
  >"$TMP/transition-lock.log" 2>&1 &
LOCK_PID=$!
sleep 0.2
curl --fail --silent --show-error --max-time 0.5 -X POST \
  -H "Authorization: Bearer $API_KEY" \
  "http://127.0.0.1:18080/api/v1/machines/$machine_id/relay/revoke" \
  >"$TMP/transition-result.log" 2>"$TMP/transition-client.log" &
TRANSITION_PID=$!
for _ in $(seq 1 50); do
  waiting=$(psql_value "SELECT count(*) FROM pg_stat_activity WHERE wait_event_type = 'Lock' AND query LIKE 'SELECT route_revision FROM machines WHERE id%';")
  [[ "$waiting" -gt 0 ]] && break
  sleep 0.1
done
[[ "$waiting" -gt 0 ]]
[[ "$(psql_value "SELECT owner_incarnation_id FROM machine_owners WHERE machine_id = '$machine_id';")" == "$owner_b" ]]
"${COMPOSE[@]}" exec -T target sh -c \
  'echo $$ > /tmp/owlmux-cluster-race-relay.pid; exec /usr/local/bin/owlmux-relay run --server ws://host.docker.internal:18080 --state /var/lib/owlmux/state.json' \
  >"$TMP/race-relay.log" 2>&1 &
RACE_RELAY_PID=$!
for _ in $(seq 1 50); do
  reconnect_waiting=$(psql_value "SELECT count(*) FROM pg_stat_activity WHERE wait_event_type = 'Lock' AND query LIKE 'SELECT EXISTS(SELECT 1 FROM deployment WHERE singleton%';")
  [[ "$reconnect_waiting" -gt 0 ]] && break
  sleep 0.02
done
[[ "$reconnect_waiting" -gt 0 ]]
[[ "$(psql_value "SELECT owner_incarnation_id FROM machine_owners WHERE machine_id = '$machine_id';")" == "$owner_b" ]]
wait "$LOCK_PID"
set +e
wait "$TRANSITION_PID"
transition_status=$?
set -e
[[ "$transition_status" == 28 ]]
for _ in $(seq 1 50); do
  lifecycle=$(psql_value "SELECT lifecycle FROM machines WHERE id = '$machine_id';")
  [[ "$lifecycle" == "disabled" ]] && break
  sleep 0.1
done
[[ "$lifecycle" == "disabled" ]]
"${COMPOSE[@]}" exec -T target sh -c 'kill -TERM "$(cat /tmp/owlmux-cluster-race-relay.pid)"' 2>/dev/null || true
kill "$RACE_RELAY_PID" 2>/dev/null || true
wait "$RACE_RELAY_PID" 2>/dev/null || true
RACE_RELAY_PID=

machine=$(curl --fail --silent --show-error --max-time 5 \
  -H "Authorization: Bearer $API_KEY" "http://127.0.0.1:18080/api/v1/machines/$machine_id")
python3 -c 'import json,sys; m=json.load(sys.stdin); assert m["lifecycle"] == "disabled" and m["reachability"] == "unknown"' <<<"$machine"
[[ "$(psql_value "SELECT count(*) FROM audit_events WHERE machine_id = '$machine_id' AND action = 'revoke_relay' AND outcome_class = 'success';")" == 1 ]]
curl --fail --silent --show-error --max-time 5 -X POST \
  -H "Authorization: Bearer $API_KEY" \
  "http://127.0.0.1:18080/api/v1/machines/$machine_id/re-enroll" >/dev/null
machine=$(curl --fail --silent --show-error --max-time 5 \
  -H "Authorization: Bearer $API_KEY" "http://127.0.0.1:18080/api/v1/machines/$machine_id")
python3 -c 'import json,sys; m=json.load(sys.stdin); assert m["lifecycle"] == "pending" and m["reachability"] == "unknown"' <<<"$machine"
[[ "$(psql_value "SELECT count(*) FROM relay_enrollments WHERE machine_id = '$machine_id' AND status = 'issued';")" == 0 ]]
curl --fail --silent --show-error --max-time 5 -X POST \
  -H "Authorization: Bearer $API_KEY" \
  "http://127.0.0.1:18080/api/v1/machines/$machine_id/enrollment-token" >"$TMP/cancelled-token.json"
curl --fail --silent --show-error --max-time 5 -X DELETE \
  -H "Authorization: Bearer $API_KEY" \
  "http://127.0.0.1:18080/api/v1/machines/$machine_id/enrollment-token" >/dev/null
[[ "$(psql_value "SELECT count(*) FROM relay_enrollments WHERE machine_id = '$machine_id' AND status = 'issued';")" == 0 ]]
[[ "$(psql_value "SELECT count(*) FROM audit_events WHERE machine_id = '$machine_id' AND action IN ('revoke_relay', 're_enroll', 'cancel_token');")" == 3 ]]
"${COMPOSE[@]}" exec -T target su - owlmux -c '/usr/bin/tmux -L owlmux has-session -t alpha'

"${COMPOSE[@]}" exec -T target sh -c 'kill -TERM "$(cat /tmp/owlmux-cluster-relay.pid)"' 2>/dev/null || true
wait "$RELAY_PID" 2>/dev/null || true
RELAY_PID=
kill -INT "$NODE_A_PID" "$NODE_B_PID"
wait "$NODE_A_PID"
wait "$NODE_B_PID"
NODE_A_PID=
NODE_B_PID=
for _ in $(seq 1 50); do
  [[ "$(psql_value "SELECT count(*) FROM server_nodes WHERE lease_until > clock_timestamp();")" == 0 ]] && break
  sleep 0.2
done
[[ "$(psql_value "SELECT count(*) FROM server_nodes WHERE lease_until > clock_timestamp();")" == 0 ]]
old_api_key=$API_KEY
API_KEY=owlmux_sk_v1_ZWVlZWVlZWVlZWVlZWVlZWVlZWVlZWVlZWVlZWVlZWU
CONFIG_EPOCH=2
start_node a 18080 19443 "$TMP/node-a-rotated.log"
NODE_A_PID=$LAST_PID
wait_ready 18080
old_key_status=$(curl --silent --show-error --max-time 5 -o "$TMP/old-key.body" -w '%{http_code}' \
  -H "Authorization: Bearer $old_api_key" http://127.0.0.1:18080/api/v1/deployment)
[[ "$old_key_status" == 401 ]]
deployment=$(curl --fail --silent --show-error --max-time 5 \
  -H "Authorization: Bearer $API_KEY" http://127.0.0.1:18080/api/v1/deployment)
python3 -c 'import json,sys; deployment=json.load(sys.stdin); assert deployment["config_epoch"] == 2 and deployment["profile"] == "clustered"' <<<"$deployment"
[[ "$(psql_value "SELECT count(*) FROM audit_events WHERE resource_kind = 'deployment' AND action = 'configuration_transition';")" == 1 ]]
"${COMPOSE[@]}" exec -T target su - owlmux -c '/usr/bin/tmux -L owlmux has-session -t alpha'

printf 'Clustered routing E2E passed: coherent node join, remote owner attachment, stale endpoint and TLS denial, owner-loss lease recovery, no remap on restart, configuration and local identity rejection, caller-cancel-safe exact-owner Relay revocation under reconnect, disabled re-enrollment, token cancellation, cold API/configuration rotation, and target tmux survival.\n'
