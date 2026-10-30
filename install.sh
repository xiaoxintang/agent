#!/bin/sh
# Download verified, version-matched release artifacts; never build on the server.
set -eu
repo=xiaoxintang/agent
version=latest
update=0
non_interactive=0
arg_agent_port= arg_grpc_port= arg_web_port= arg_web=
usage() {
    echo 'Usage: sh install.sh [--version TAG] [--repo OWNER/REPO] [--update]'
    echo '  [--agent-port PORT] [--grpc-port PORT] [--web-port PORT] [--web yes|no] [--non-interactive]'
}
while [ "$#" -gt 0 ]; do
    case "$1" in
        --update) update=1; shift ;;
        --non-interactive) non_interactive=1; shift ;;
        --version|--repo|--agent-port|--grpc-port|--web-port|--web)
            [ "$#" -ge 2 ] && [ -n "$2" ] || { echo "Missing value for $1" >&2; exit 1; }
            case "$1" in
                --version) version=$2 ;; --repo) repo=$2 ;;
                --agent-port) arg_agent_port=$2 ;; --grpc-port) arg_grpc_port=$2 ;;
                --web-port) arg_web_port=$2 ;; --web) arg_web=$2 ;;
            esac
            shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) echo "Unknown argument: $1" >&2; usage >&2; exit 1 ;;
    esac
done
case "$repo" in ''|/*|*/|*/*/*|*[!A-Za-z0-9_./-]*) echo 'Invalid GitHub repository.' >&2; exit 1 ;; */*) ;; *) echo 'Repository must be OWNER/REPO.' >&2; exit 1 ;; esac
validate_version() {
    case "$1" in *[!A-Za-z0-9._+-]*|'') echo "Invalid version tag: $1" >&2; exit 1 ;; v[0-9]*) ;; *) echo 'Version must be a v-prefixed release tag.' >&2; exit 1 ;; esac
}
[ "$version" = latest ] || validate_version "$version"
for port in "$arg_agent_port" "$arg_grpc_port" "$arg_web_port"; do
    [ -n "$port" ] || continue
    case "$port" in *[!0-9]*|'') echo 'Invalid port.' >&2; exit 1 ;; esac
    [ "${#port}" -le 5 ] && [ "$port" -ge 1 ] && [ "$port" -le 65535 ] || { echo 'Invalid port.' >&2; exit 1; }
done
case "$arg_web" in ''|yes|no) ;; *) echo '--web must be yes or no.' >&2; exit 1 ;; esac
[ "$(id -u)" -eq 0 ] || { echo 'Run this installer as root.' >&2; exit 1; }
[ "$(uname -s)" = Linux ] || { echo 'This installer requires Linux.' >&2; exit 1; }
case "$(uname -m)" in x86_64|amd64) arch=amd64 ;; aarch64|arm64) arch=arm64 ;; *) echo "Unsupported architecture: $(uname -m)" >&2; exit 1 ;; esac
for command in curl tar sha256sum systemctl install awk mktemp; do command -v "$command" >/dev/null 2>&1 || { echo "Required command is missing: $command" >&2; exit 1; }; done
systemctl show-environment >/dev/null 2>&1 || { echo 'A running systemd system is required.' >&2; exit 1; }
if [ "$version" = latest ]; then
    release_url=$(curl --fail --silent --show-error --location --head --proto '=https' --proto-redir '=https' --retry 3 --connect-timeout 15 --max-time 120 --output /dev/null --write-out '%{url_effective}' "https://github.com/$repo/releases/latest")
    version=${release_url##*/}; validate_version "$version"
fi
staging=$(mktemp -d)
trap 'rm -rf "$staging"' EXIT
trap 'exit 1' HUP INT TERM
base="https://github.com/$repo/releases/download/$version"
curl --fail --silent --show-error --location --proto '=https' --proto-redir '=https' --retry 3 --connect-timeout 15 --max-time 120 --output "$staging/SHA256SUMS" "$base/SHA256SUMS"
download_checked() {
    asset=$1
    curl --fail --silent --show-error --location --proto '=https' --proto-redir '=https' --retry 3 --connect-timeout 15 --max-time 300 --output "$staging/$asset" "$base/$asset"
    awk -v name="$asset" '$2==name {print;count++} END {if(count!=1)exit 1}' "$staging/SHA256SUMS" > "$staging/selected.sha256" || { echo "Release checksum is missing or duplicated for $asset" >&2; exit 1; }
    (cd "$staging" && sha256sum --check selected.sha256)
}
extract_checked() {
    archive=$1 prefix=$2 destination=$3
    # Reject traversal, special files and links before expanding optional assets.
    tar -tzf "$archive" | awk -v prefix="$prefix/" 'index($0,prefix)!=1 || $0 ~ /(^|\/)\.\.(\/|$)/ {bad=1} END {exit bad}' || { echo 'Invalid archive paths.' >&2; exit 1; }
    tar -tvzf "$archive" | awk 'substr($0,1,1)!="-" && substr($0,1,1)!="d" {bad=1} END {exit bad}' || { echo 'Archive links or special files are not allowed.' >&2; exit 1; }
    mkdir -p "$destination"
    tar -xzf "$archive" -C "$destination" --strip-components=1
}
name="agent-$version-linux-$arch"
echo "Downloading $repo $version for Linux $arch..."
download_checked "$name.tar.gz"
extract_checked "$staging/$name.tar.gz" "$name" "$staging/package"
for file in agent deploy/install.sh deploy/options.sh deploy/xray-agent.service deploy/xray-web.service; do
    [ -f "$staging/package/$file" ] && [ ! -L "$staging/package/$file" ] || { echo "Package contains an invalid file: $file" >&2; exit 1; }
done
agent_binary="$staging/package/agent"
[ "$("$agent_binary" -v)" = "agent ${version#v}" ] || { echo 'Agent release version mismatch.' >&2; exit 1; }
. "$staging/package/deploy/options.sh"
resolve_options
if [ "$web_enabled" = yes ]; then
    backend_name="web-backend-$version-linux-$arch"
    frontend_name="web-frontend-$version"
    download_checked "$backend_name.tar.gz"
    download_checked "$frontend_name.tar.gz"
    extract_checked "$staging/$backend_name.tar.gz" "$backend_name" "$staging/backend"
    extract_checked "$staging/$frontend_name.tar.gz" "$frontend_name" "$staging/frontend"
    "$agent_binary" verify-web --backend "$staging/backend/agent-web" --frontend "$staging/frontend"
fi
set -- "$staging/package/deploy/install.sh" "$agent_binary" --agent-port "$agent_port" --grpc-port "$grpc_port" --web-port "$web_port" --web "$web_enabled" --non-interactive
if [ "$update" -eq 1 ]; then set -- "$@" --update; fi
if [ "$web_enabled" = yes ]; then set -- "$@" --web-backend "$staging/backend/agent-web" --web-frontend "$staging/frontend"; fi
sh "$@"
echo 'Installed. Run agent -v to check the version, or agent --help for commands.'
