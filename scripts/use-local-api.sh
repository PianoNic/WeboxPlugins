#!/usr/bin/env sh
# Builds the plugins against a local Webox checkout instead of the git dependency: writes the git-ignored .cargo/config.toml
# with a [patch] for collabhost-plugin-api. Usage: scripts/use-local-api.sh ../Webox    (or: scripts/use-local-api.sh --off)
set -eu
root="$(cd "$(dirname "$0")/.." && pwd)"
cfg="$root/.cargo/config.toml"
if [ "${1:-}" = "--off" ]; then rm -f "$cfg"; echo "Using the git dependency."; exit 0; fi
[ -n "${1:-}" ] || { echo "Pass the path of a Webox checkout, e.g. scripts/use-local-api.sh ../Webox" >&2; exit 1; }
api="$(cd "$1/plugin-api" && pwd)"
[ -f "$api/Cargo.toml" ] || { echo "$api/Cargo.toml not found" >&2; exit 1; }
mkdir -p "$root/.cargo"
cat > "$cfg" <<EOF
# Written by scripts/use-local-api.sh (git-ignored): build against a local Webox checkout.
[patch.crates-io]
collabhost-plugin-api = { path = "$api" }
EOF
echo "Using $api"
