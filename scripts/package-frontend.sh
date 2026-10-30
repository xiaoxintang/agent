#!/bin/sh
# Invoked only after CI has built web/dist.
set -eu
repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
version=$(python3 -c 'import sys,tomllib;print(tomllib.load(open(sys.argv[1],"rb"))["package"]["version"])' "$repo_dir/Cargo.toml")
python3 - "$repo_dir/web/dist/version.json" "$version" <<'PY'
import json,sys
assert json.load(open(sys.argv[1]))['version']==sys.argv[2], 'frontend version mismatch'
PY
[ -f "$repo_dir/web/dist/index.html" ] || { echo 'Build the frontend first.' >&2; exit 1; }
staging=$(mktemp -d)
trap 'rm -rf "$staging"' EXIT
name="web-frontend-v$version"
mkdir -p "$staging/$name" "$repo_dir/dist"
cp -R "$repo_dir/web/dist/." "$staging/$name/"
tar -czf "$repo_dir/dist/$name.tar.gz" -C "$staging" "$name"
