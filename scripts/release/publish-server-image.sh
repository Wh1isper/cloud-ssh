#!/usr/bin/env bash
set -euo pipefail

image="${1:?usage: publish-server-image.sh <image> <version> <revision>}"
version="${2:?usage: publish-server-image.sh <image> <version> <revision>}"
revision="${3:?usage: publish-server-image.sh <image> <version> <revision>}"

verify_image_identity() {
    local reference="$1"
    local actual_version
    local actual_revision
    local actual_user
    actual_version="$(docker image inspect "$reference" --format '{{index .Config.Labels "org.opencontainers.image.version"}}')"
    actual_revision="$(docker image inspect "$reference" --format '{{index .Config.Labels "org.opencontainers.image.revision"}}')"
    actual_user="$(docker image inspect "$reference" --format '{{.Config.User}}')"
    if [[ "$actual_version" != "$version" || "$actual_revision" != "$revision" || "$actual_user" != owlmux ]]; then
        echo "image identity mismatch for $reference" >&2
        echo "expected version=$version revision=$revision user=owlmux" >&2
        echo "actual version=$actual_version revision=$actual_revision user=$actual_user" >&2
        return 1
    fi
}

verify_image_identity "$image"
pull_log="$(mktemp)"
if docker pull "$image" >"$pull_log" 2>&1; then
    verify_image_identity "$image"
    echo "$image already exists with the exact release identity; preserving it"
    exit 0
fi

if ! grep -Eqi 'manifest unknown|not found' "$pull_log"; then
    cat "$pull_log" >&2
    echo "could not establish whether $image already exists" >&2
    exit 1
fi

verify_image_identity "$image"
docker push "$image"
