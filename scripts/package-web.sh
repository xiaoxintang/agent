#!/bin/sh
# Invoked only after CI has built the Rust backend.
set -eu
[ "$#" -eq 2 ] || { echo 'Usage: package-web.sh TARGET amd64|arm64' >&2; exit 1; }
target=$1 arch=$2
case "$target:$arch" in x86_64-unknown-linux-musl:amd64|aarch64-unknown-linux-musl:arm64) ;; *) echo 'Invalid target/architecture.' >&2; exit 1 ;; esac
repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
version=$(python3 -c 'import sys,tomllib;print(tomllib.load(open(sys.argv[1],"rb"))["package"]["version"])' "$repo_dir/Cargo.toml")
binary="$repo_dir/target/$target/release/agent-web"
[ "$("$binary" --version)" = "agent-web $version" ] || { echo 'Web backend version mismatch.' >&2; exit 1; }
staging=$(mktemp -d)
trap 'rm -rf "$staging"' EXIT
name="web-backend-v$version-linux-$arch"
mkdir -p "$staging/$name" "$repo_dir/dist"
install -m 0755 "$binary" "$staging/$name/agent-web"
"$binary" --openapi > "$staging/$name/openapi.json"
printf '{"version":"%s","architecture":"%s"}\n' "$version" "$arch" > "$staging/$name/version.json"
tar -czf "$repo_dir/dist/$name.tar.gz" -C "$staging" "$name"
