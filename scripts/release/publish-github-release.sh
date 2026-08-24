#!/usr/bin/env bash
set -euo pipefail

release_tag="${1:?usage: publish-github-release.sh <tag> <version> <prerelease> <asset>...}"
version="${2:?usage: publish-github-release.sh <tag> <version> <prerelease> <asset>...}"
prerelease="${3:?usage: publish-github-release.sh <tag> <version> <prerelease> <asset>...}"
shift 3
(($# > 0)) || {
    echo "at least one release asset is required" >&2
    exit 1
}
[[ "$prerelease" == true || "$prerelease" == false ]]
: "${GH_REPO:?GH_REPO is required}"

release_json="$(mktemp)"
release_list="$(mktemp)"
release_error="$(mktemp)"

load_release() {
    if gh api "repos/$GH_REPO/releases/tags/$release_tag" >"$release_json" 2>"$release_error"; then
        return 0
    fi
    if ! grep -Fq 'HTTP 404' "$release_error"; then
        cat "$release_error" >&2
        return 2
    fi

    if ! gh api --paginate --slurp "repos/$GH_REPO/releases?per_page=100" >"$release_list" 2>"$release_error"; then
        cat "$release_error" >&2
        return 2
    fi
    local match_count
    match_count="$(jq --exit-status --arg tag "$release_tag" '[.[][] | select(.tag_name == $tag)] | length' "$release_list")" || return 2
    case "$match_count" in
        0)
            return 1
            ;;
        1)
            jq --exit-status --arg tag "$release_tag" '[.[][] | select(.tag_name == $tag)][0]' "$release_list" >"$release_json" || return 2
            return 0
            ;;
        *)
            echo "multiple GitHub Releases found for $release_tag" >&2
            return 2
            ;;
    esac
}

wait_for_created_release() {
    local attempt
    local load_status
    local max_attempts=15
    for ((attempt = 1; attempt <= max_attempts; attempt++)); do
        load_status=0
        load_release || load_status=$?
        case "$load_status" in
            0)
                return 0
                ;;
            1)
                if ((attempt < max_attempts)); then
                    sleep 2
                fi
                ;;
            *)
                return "$load_status"
                ;;
        esac
    done
    return 1
}

validate_release_identity() {
    local actual_tag
    local actual_prerelease
    actual_tag="$(jq --exit-status --raw-output '.tag_name' "$release_json")"
    actual_prerelease="$(jq --exit-status --raw-output '.prerelease' "$release_json")"
    if [[ "$actual_tag" != "$release_tag" || "$actual_prerelease" != "$prerelease" ]]; then
        echo "release identity mismatch for $release_tag" >&2
        echo "expected tag=$release_tag prerelease=$prerelease" >&2
        echo "actual tag=$actual_tag prerelease=$actual_prerelease" >&2
        return 1
    fi
}

declare -A expected_assets=()
for asset in "$@"; do
    [[ -f "$asset" ]]
    name="$(basename "$asset")"
    if [[ -n "${expected_assets[$name]+present}" ]]; then
        echo "duplicate release asset name: $name" >&2
        exit 1
    fi
    expected_assets["$name"]="$asset"
done

load_status=0
load_release || load_status=$?
if [[ "$load_status" -ne 0 ]]; then
    if [[ "$load_status" -ne 1 ]]; then
        exit "$load_status"
    fi
    args=(--verify-tag --draft --title "OwlMux $version" --generate-notes)
    if [[ "$prerelease" == true ]]; then
        args+=(--prerelease --latest=false)
    fi
    create_status=0
    gh release create "$release_tag" "${args[@]}" || create_status=$?
    recovery_status=0
    wait_for_created_release || recovery_status=$?
    if [[ "$recovery_status" -ne 0 ]]; then
        echo "GitHub Release creation failed with status $create_status and no recoverable release exists" >&2
        exit "$recovery_status"
    fi
fi

validate_release_identity

declare -A existing_assets=()
while IFS=$'\t' read -r name digest; do
    [[ -n "$name" ]] || continue
    if [[ -z "${expected_assets[$name]+present}" ]]; then
        echo "unexpected existing release asset: $name" >&2
        exit 1
    fi
    expected_digest="sha256:$(sha256sum "${expected_assets[$name]}" | cut -d ' ' -f 1)"
    if [[ "$digest" != "$expected_digest" ]]; then
        echo "release asset checksum mismatch for $name: expected $expected_digest, found ${digest:-none}" >&2
        exit 1
    fi
    existing_assets["$name"]=true
done < <(jq --raw-output '.assets[] | [.name, (.digest // "")] | @tsv' "$release_json")

for name in "${!expected_assets[@]}"; do
    if [[ -n "${existing_assets[$name]+present}" ]]; then
        continue
    fi
    gh release upload "$release_tag" "${expected_assets[$name]}"
done

load_release
validate_release_identity
if [[ "$(jq --exit-status '.assets | length' "$release_json")" -ne "${#expected_assets[@]}" ]]; then
    echo "release asset count is incomplete for $release_tag" >&2
    exit 1
fi
for name in "${!expected_assets[@]}"; do
    expected_digest="sha256:$(sha256sum "${expected_assets[$name]}" | cut -d ' ' -f 1)"
    actual_digest="$(jq --exit-status --raw-output --arg name "$name" '.assets[] | select(.name == $name) | .digest' "$release_json")"
    if [[ "$actual_digest" != "$expected_digest" ]]; then
        echo "release asset checksum mismatch for $name after upload" >&2
        exit 1
    fi
done

if [[ "$(jq --exit-status --raw-output '.draft' "$release_json")" == true ]]; then
    if [[ "$prerelease" == true ]]; then
        gh release edit "$release_tag" --draft=false --latest=false
    else
        gh release edit "$release_tag" --draft=false
    fi
fi

load_release
validate_release_identity
[[ "$(jq --exit-status --raw-output '.draft' "$release_json")" == false ]]
echo "$release_tag is published with ${#expected_assets[@]} exact assets"
