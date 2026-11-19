#!/usr/bin/env bash
# Checks the published update feed exactly as installed apps consume it:
# fetches latest.json from the endpoint in tauri.conf.json, expects <version>, then
# downloads every macOS platform archive (with the updater's Accept header) and
# verifies its minisign signature against the embedded public key.
#
# Usage: scripts/verify-update-feed.sh <version>        (requires curl, jq, minisign)
set -euo pipefail

version=${1:?usage: verify-update-feed.sh <version>}
root=$(cd "$(dirname "$0")/.." && pwd)
conf="$root/src-tauri/tauri.conf.json"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

endpoint=$(jq -r '.plugins.updater.endpoints[0]' "$conf")
jq -r '.plugins.updater.pubkey' "$conf" | base64 -d >"$tmp/key.pub"

# The CDN behind releases/latest can lag a few seconds after publishing.
for attempt in 1 2 3 4 5 6; do
  curl -fsSL "$endpoint" -o "$tmp/latest.json"
  [ "$(jq -r .version "$tmp/latest.json")" = "$version" ] && break
  if [ "$attempt" = 6 ]; then
    echo "$endpoint announces $(jq -r .version "$tmp/latest.json"), expected $version" >&2
    exit 1
  fi
  sleep 10
done
echo "feed: $endpoint announces $version"

for platform in darwin-aarch64 darwin-x86_64; do
  url=$(jq -r --arg p "$platform" '.platforms[$p].url // empty' "$tmp/latest.json")
  [ -n "$url" ] || { echo "$platform missing from latest.json" >&2; exit 1; }
  jq -r --arg p "$platform" '.platforms[$p].signature' "$tmp/latest.json" | base64 -d >"$tmp/$platform.minisig"
  curl -fsSL -H "Accept: application/octet-stream" "$url" -o "$tmp/$platform.tar.gz"
  minisign -V -q -p "$tmp/key.pub" -m "$tmp/$platform.tar.gz" -x "$tmp/$platform.minisig"
  echo "$platform: $(wc -c <"$tmp/$platform.tar.gz" | tr -d ' ') bytes, signature ok"
done
