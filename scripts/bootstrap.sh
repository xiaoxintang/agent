#!/bin/sh
# Download this file, review it, then run it; stdin remains available to the wizard.
# A trusted archive SHA-256 is required, supplied from the signed release channel.
set -eu
umask 077
panel_bootstrap_version=v0.1.0
panel_bootstrap_sha=
panel_bootstrap_component=all
panel_bootstrap_config=
panel_bootstrap_unattended=0
while [ "$#" -gt 0 ]; do
  case "$1" in
    --version) panel_bootstrap_version=${2:?missing version}; shift 2 ;;
    --sha256) panel_bootstrap_sha=${2:?missing trusted checksum}; shift 2 ;;
    --component) panel_bootstrap_component=${2:?missing component}; shift 2 ;;
    --config) panel_bootstrap_config=${2:?missing config path}; shift 2 ;;
    --non-interactive) panel_bootstrap_unattended=1; shift ;;
    --help) echo 'Usage: sh bootstrap.sh --version v0.1.0 --sha256 TRUSTED_ARCHIVE_SHA256 [--component server|agent|all] [--non-interactive --config ROOT_OWNED_JSON]'; exit 0 ;;
    *) echo 'Unsupported bootstrap option' >&2; exit 1 ;;
  esac
done
case "$panel_bootstrap_version" in v[0-9]*.[0-9]*.[0-9]*) ;; *) echo 'An exact version tag is required' >&2; exit 1 ;; esac
case "$panel_bootstrap_version" in *[!v0-9.-]*|*..*) echo 'Invalid version tag' >&2; exit 1 ;; esac
case "$panel_bootstrap_component" in server|agent|all) ;; *) echo 'Invalid component' >&2; exit 1 ;; esac
if [ "${#panel_bootstrap_sha}" -ne 64 ]; then echo 'Provide a trusted SHA-256 from the signed release instructions using --sha256; never take it from an unauthenticated mirror' >&2; exit 1; fi
case "$panel_bootstrap_sha" in *[!0-9a-fA-F]*) echo 'Invalid SHA-256' >&2; exit 1 ;; esac
if [ "$(uname -s)" != Linux ]; then echo 'Linux is required' >&2; exit 1; fi
if [ "$(id -u)" -ne 0 ]; then echo 'Run the reviewed script with sudo; the long-running Agent will use a dedicated non-root account' >&2; exit 1; fi
case "$(uname -m)" in x86_64) panel_bootstrap_arch=x86_64 ;; aarch64|arm64) panel_bootstrap_arch=aarch64 ;; *) echo 'Unsupported architecture' >&2; exit 1 ;; esac
for panel_bootstrap_tool in curl sha256sum tar sort awk mktemp; do command -v "$panel_bootstrap_tool" >/dev/null 2>&1 || { echo "Required tool missing: $panel_bootstrap_tool" >&2; exit 1; }; done
panel_bootstrap_tmp=$(mktemp -d /tmp/panel-bootstrap.XXXXXXXX)
trap 'rm -rf -- "$panel_bootstrap_tmp"' EXIT HUP INT TERM
panel_bootstrap_url="https://github.com/xiaoxintang/agent/releases/download/$panel_bootstrap_version/panel-linux-$panel_bootstrap_arch-$panel_bootstrap_version.tar.gz"
curl --fail --silent --show-error --location --proto '=https' --proto-redir '=https' --tlsv1.2 --connect-timeout 15 --max-time 300 --max-redirs 3 --max-filesize 157286400 --output "$panel_bootstrap_tmp/bundle.tar.gz" "$panel_bootstrap_url"
printf '%s  %s\n' "$panel_bootstrap_sha" "$panel_bootstrap_tmp/bundle.tar.gz" | sha256sum --check --status
panel_bootstrap_members=$(tar -tzf "$panel_bootstrap_tmp/bundle.tar.gz" | LC_ALL=C sort)
if [ "$panel_bootstrap_members" != 'bundle.json
panel-agent
panel-installer
panel-server' ]; then echo 'Unexpected archive paths or duplicate entries' >&2; exit 1; fi
tar -tvzf "$panel_bootstrap_tmp/bundle.tar.gz" | awk 'substr($0,1,1)!="-" {exit 1}'
tar --extract --gzip --file "$panel_bootstrap_tmp/bundle.tar.gz" --directory "$panel_bootstrap_tmp" --no-same-owner --no-same-permissions --keep-old-files
chmod 700 "$panel_bootstrap_tmp/panel-installer"
panel_bootstrap_metadata=$(sha256sum "$panel_bootstrap_tmp/bundle.json")
panel_bootstrap_metadata=${panel_bootstrap_metadata%% *}
if [ "$panel_bootstrap_unattended" -eq 1 ]; then
  [ -n "$panel_bootstrap_config" ] || { echo '--non-interactive requires --config' >&2; exit 1; }
  "$panel_bootstrap_tmp/panel-installer" install --component "$panel_bootstrap_component" --bundle "$panel_bootstrap_tmp" --bundle-sha256 "$panel_bootstrap_metadata" --non-interactive --config "$panel_bootstrap_config"
else
  [ -t 0 ] || { echo 'Interactive bootstrap needs a TTY: download to a file before running; do not pipe curl into sh' >&2; exit 1; }
  "$panel_bootstrap_tmp/panel-installer" install --component "$panel_bootstrap_component" --bundle "$panel_bootstrap_tmp" --bundle-sha256 "$panel_bootstrap_metadata"
fi
