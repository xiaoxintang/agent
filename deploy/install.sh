#!/bin/sh
# 在线引导脚本和离线安装包共用的部署入口；只安装 Agent，不接管现有 Xray。
set -eu

if [ "$(id -u)" -ne 0 ]; then
    echo "Run this installer as root." >&2
    exit 1
fi
if [ "$(uname -s)" != Linux ] || [ ! -d /run/systemd/system ]; then
    echo "A running Linux systemd system is required." >&2
    exit 1
fi
if [ "$#" -ne 1 ] || [ ! -f "$1" ]; then
    echo "Usage: sh deploy/install.sh /path/to/precompiled/agent" >&2
    exit 1
fi

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
# 只替换本项目拥有的 unit；自定义设置应放在 systemd drop-in 中保留。
if [ -e /etc/systemd/system/xray-agent.service ]; then
    first_line=$(head -n 1 /etc/systemd/system/xray-agent.service)
    if [ "$first_line" != '# Managed by xray-agent' ] && ! cmp -s "$script_dir/xray-agent.service" /etc/systemd/system/xray-agent.service; then
        echo "An existing unmanaged xray-agent.service was found; inspect it before replacing it." >&2
        exit 1
    fi
fi
install -d -m 0755 /usr/local/bin
# 先写新文件再原子替换，避免用户或 systemd 执行到只复制了一部分的程序。
install -m 0755 "$1" /usr/local/bin/agent.new
mv -f /usr/local/bin/agent.new /usr/local/bin/agent
# init 会复用已有配置和 Token；重复安装不会重新生成凭据或覆盖 SQLite 快照。
/usr/local/bin/agent init
install -m 0644 "$script_dir/xray-agent.service" /etc/systemd/system/xray-agent.service
systemctl daemon-reload
systemctl enable xray-agent.service
# 更新二进制后显式重启 Agent，单纯 enable --now 不会替换已经运行的旧进程。
systemctl restart xray-agent.service
echo "Agent started. Open /docs for Swagger UI and enter the generated Bearer token."
