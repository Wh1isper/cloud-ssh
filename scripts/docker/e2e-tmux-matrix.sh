#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$ROOT"

run_profile() {
  local target=$1
  local expected_version=$2
  local dockerfile=${3:-dev/target.Dockerfile}
  local login_shell=${4:-/bin/bash}
  printf '\nRunning single-node E2E against %s (%s, %s)\n' "$target" "$expected_version" "$login_shell"
  OWLMUX_TARGET_BASE_IMAGE="$target" \
  OWLMUX_TARGET_DOCKERFILE="$dockerfile" \
  OWLMUX_TARGET_LOGIN_SHELL="$login_shell" \
  OWLMUX_EXPECTED_TMUX_VERSION="$expected_version" \
    scripts/docker/e2e-single-node.sh
}

run_profile ubuntu:22.04 'tmux 3.2a'
run_profile debian:12-slim 'tmux 3.3a' dev/target.Dockerfile /bin/dash
run_profile debian:13-slim 'tmux 3.5a'
run_profile debian:13-slim 'tmux 3.7b' dev/target-upstream.Dockerfile

printf '\nSingle-node tmux distribution and current-upstream matrix passed.\n'
