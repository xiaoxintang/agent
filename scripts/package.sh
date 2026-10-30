#!/bin/sh
# CI 构建后的打包步骤：验证程序可运行，并把二进制、文档和部署文件一起归档。
set -eu

if [ "$#" -ne 2 ]; then
    echo "Usage: sh scripts/package.sh TARGET amd64|arm64" >&2
    exit 1
fi
target=$1
arch=$2
# 架构名称与 Rust target 必须配套，避免发布名称正确但程序架构错误的安装包。
case "$target:$arch" in
    x86_64-unknown-linux-musl:amd64|aarch64-unknown-linux-musl:arm64) ;;
    *) echo "Unsupported target/architecture combination: $target:$arch" >&2; exit 1 ;;
esac

repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
version=$(python3 -c 'import sys,tomllib; print(tomllib.load(open(sys.argv[1], "rb"))["package"]["version"])' "$repo_dir/Cargo.toml")
binary="$repo_dir/target/$target/release/agent"
if [ ! -x "$binary" ]; then
    echo "Build $target first; executable not found: $binary" >&2
    exit 1
fi
if [ "$("$binary" -v)" != "agent $version" ]; then
    echo "Binary version differs from Cargo.toml" >&2
    exit 1
fi
# 冒烟验证无需初始化；版本、帮助和文档导出不应依赖 root 或运行中的 Xray。
"$binary" --help >/dev/null

staging=$(mktemp -d)
trap 'rm -rf "$staging"' EXIT
trap 'exit 1' HUP INT TERM
name="agent-v$version-linux-$arch"
package="$staging/$name"
mkdir -p "$package/deploy" "$package/licenses" "$repo_dir/dist"
install -m 0755 "$binary" "$package/agent"
install -m 0755 "$repo_dir/deploy/install.sh" "$package/deploy/install.sh"
install -m 0644 "$repo_dir/deploy/options.sh" "$package/deploy/options.sh"
install -m 0644 "$repo_dir/deploy/xray-web.service" "$package/deploy/xray-web.service"
install -m 0644 "$repo_dir/deploy/xray-agent.service" "$package/deploy/xray-agent.service"
install -m 0644 "$repo_dir/deploy/xray.service" "$package/deploy/xray.service"
install -m 0644 "$repo_dir/README.md" "$package/README.md"
install -m 0644 "$repo_dir/proto/LICENSE" "$package/licenses/Xray-core.LICENSE"
install -m 0644 "$repo_dir/proto/upstream.json" "$package/licenses/Xray-core.upstream.json"
# 由即将发布的同一个二进制生成文档，避免归档过期的 OpenAPI 文件。
"$binary" openapi --output "$package/openapi.json"
tar -czf "$repo_dir/dist/$name.tar.gz" -C "$staging" "$name"
echo "Packaged dist/$name.tar.gz"
