#!/bin/sh
# 仅清理本 Agent 的标准部署路径；配置损坏或缺失时仍可卸载。
set -eu

if [ "$(id -u)" -ne 0 ]; then
    echo "Run this uninstaller as root." >&2
    exit 1
fi
if [ "$(uname -s)" != Linux ] || [ ! -d /run/systemd/system ]; then
    echo "A running Linux systemd system is required." >&2
    exit 1
fi
systemctl show-environment >/dev/null

# 在停止服务或删除文件前完成全部归属检查，避免接管其他安装方式的 Xray。
if [ -e /usr/local/bin/xray ] || [ -L /usr/local/bin/xray ]; then
    echo "An unmanaged /usr/local/bin/xray was found; refusing to remove its configuration." >&2
    exit 1
fi
units=
for unit in xray-web.service xray-agent.service xray.service; do
    unit_file="/etc/systemd/system/$unit"
    if [ -e "$unit_file" ] || [ -L "$unit_file" ]; then
        if [ ! -f "$unit_file" ] || [ "$(head -n 1 "$unit_file")" != '# Managed by xray-agent' ]; then
            echo "Unmanaged unit found: $unit_file; uninstall aborted." >&2
            exit 1
        fi
        units="$units $unit"
    fi
    info=$(systemctl show "$unit" --property=LoadState,FragmentPath) || {
        case "$info" in
            *LoadState=not-found*) ;;
            *) echo "Cannot inspect $unit; uninstall aborted." >&2; exit 1 ;;
        esac
    }
    fragment=$(printf '%s\n' "$info" | awk -F= '$1 == "FragmentPath" { print substr($0, 14) }')
    if [ -n "$fragment" ] && [ "$fragment" != "$unit_file" ]; then
        echo "Unmanaged unit found: $fragment; uninstall aborted." >&2
        exit 1
    fi
    # unit 文件可能已被手动删除，但 systemd 仍缓存着服务并运行旧进程。
    if [ "$fragment" = "$unit_file" ] && [ ! -e "$unit_file" ]; then
        units="$units $unit"
    fi
done

# 先等待管理操作完成，再停止 Xray；任一停止失败时保留全部文件。
for unit in $units; do
    systemctl stop "$unit"
done
for unit in $units; do
    if [ -f "/etc/systemd/system/$unit" ]; then
        systemctl disable "$unit"
    fi
done
rm -f -- /etc/systemd/system/xray-web.service /etc/systemd/system/multi-user.target.wants/xray-web.service \
    /etc/systemd/system/xray-agent.service /etc/systemd/system/xray.service \
    /etc/systemd/system/multi-user.target.wants/xray-agent.service \
    /etc/systemd/system/multi-user.target.wants/xray.service
rm -rf -- /etc/systemd/system/xray-web.service.d /run/systemd/system/xray-web.service.d \
    /etc/systemd/system/xray-agent.service.d /etc/systemd/system/xray.service.d \
    /run/systemd/system/xray-agent.service.d /run/systemd/system/xray.service.d
systemctl daemon-reload

# 先移除可执行程序，再删除包含更新锁的状态目录。
rm -f -- /usr/local/bin/agent /usr/local/bin/agent.new
rm -rf -- /etc/xray-agent /etc/xray /opt/xray /var/lib/xray-agent /var/log/xray /run/xray /opt/xray-web /var/lib/xray-web
echo "Agent and managed Xray uninstalled; all configuration and state removed."
