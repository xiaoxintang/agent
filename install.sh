#!/bin/sh
# 在线引导安装器：只下载已发布的二进制包，本机不执行 Rust 构建。
set -eu

repo=xiaoxintang/agent
version=latest

usage() {
    echo "Usage: sh install.sh [--version v0.1.0] [--repo OWNER/REPO]"
    echo "Download a verified GitHub Release and install the agent systemd service."
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --version|--repo)
            if [ "$#" -lt 2 ] || [ -z "$2" ]; then
                echo "Missing value for $1" >&2
                exit 1
            fi
            case "$1" in --version) version=$2 ;; --repo) repo=$2 ;; esac
            shift 2
            ;;
        -h|--help) usage; exit 0 ;;
        *) echo "Unknown argument: $1" >&2; usage >&2; exit 1 ;;
    esac
done

# 仓库名和标签会拼入下载地址及文件名，先限制其格式再进行网络访问。
case "$repo" in
    ''|/*|*/|*/*/*|*[!A-Za-z0-9_./-]*) echo "Invalid GitHub repository: $repo" >&2; exit 1 ;;
    */*) ;;
    *) echo "Repository must be OWNER/REPO" >&2; exit 1 ;;
esac
validate_version() {
    case "$1" in
        *[!A-Za-z0-9._+-]*|'') echo "Invalid version tag: $1" >&2; exit 1 ;;
        v[0-9]*) ;;
        *) echo "Version must be a tag such as v0.1.0" >&2; exit 1 ;;
    esac
}
if [ "$version" != latest ]; then validate_version "$version"; fi

if [ "$(id -u)" -ne 0 ]; then
    echo "Run this installer as root, for example with sudo sh." >&2
    exit 1
fi
if [ "$(uname -s)" != Linux ]; then
    echo "This installer requires Linux." >&2
    exit 1
fi
case "$(uname -m)" in
    x86_64|amd64) arch=amd64 ;;
    aarch64|arm64) arch=arm64 ;;
    *) echo "Unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac
for command in curl tar sha256sum systemctl install awk mktemp; do
    if ! command -v "$command" >/dev/null 2>&1; then
        echo "Required command is missing: $command" >&2
        exit 1
    fi
done
if ! systemctl show-environment >/dev/null 2>&1; then
    echo "A running systemd system is required." >&2
    exit 1
fi

if [ "$version" = latest ]; then
    # 只解析一次 latest，后续安装包和校验文件都绑定同一个不可变版本标签。
    release_url=$(curl --fail --silent --show-error --location --head \
        --proto '=https' --proto-redir '=https' --retry 3 --connect-timeout 15 --max-time 120 \
        --output /dev/null --write-out '%{url_effective}' "https://github.com/$repo/releases/latest")
    version=${release_url##*/}
    validate_version "$version"
fi

staging=$(mktemp -d)
# 下载、校验和解压均在私有临时目录完成，任何阶段失败都统一清理。
trap 'rm -rf "$staging"' EXIT
trap 'exit 1' HUP INT TERM
name="agent-$version-linux-$arch"
asset="$name.tar.gz"
base="https://github.com/$repo/releases/download/$version"
echo "Downloading $repo $version for Linux $arch..."
curl --fail --silent --show-error --location --proto '=https' --proto-redir '=https' \
    --retry 3 --connect-timeout 15 --max-time 300 --output "$staging/$asset" "$base/$asset"
curl --fail --silent --show-error --location --proto '=https' --proto-redir '=https' \
    --retry 3 --connect-timeout 15 --max-time 120 --output "$staging/SHA256SUMS" "$base/SHA256SUMS"

# 只检查当前架构对应的一条摘要；缺失或重复都拒绝安装，不能直接跳过验证。
awk -v name="$asset" '$2 == name { print; count++ } END { if (count != 1) exit 1 }' \
    "$staging/SHA256SUMS" > "$staging/selected.sha256" || {
        echo "Release checksum is missing or duplicated for $asset" >&2
        exit 1
    }
(cd "$staging" && sha256sum --check selected.sha256)
# 必须先通过摘要校验，再提取明确需要的成员，避免展开包内其他路径。
mkdir "$staging/package"
tar -xzf "$staging/$asset" -C "$staging/package" --strip-components=1 \
    "$name/agent" "$name/deploy/install.sh" "$name/deploy/xray-agent.service"
for file in agent deploy/install.sh deploy/xray-agent.service; do
    if [ ! -f "$staging/package/$file" ] || [ -L "$staging/package/$file" ]; then
        echo "Package contains an invalid file: $file" >&2
        exit 1
    fi
done
# 实际系统写入复用安装包中的本地脚本，保持在线与离线部署行为一致。
sh "$staging/package/deploy/install.sh" "$staging/package/agent"
echo "Installed. Run agent -v to check the version, or agent --help for commands."
