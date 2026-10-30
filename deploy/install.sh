#!/bin/sh
# 在线引导脚本和离线安装包共用的部署入口；安装 Agent 和默认版本的 Xray。
set -eu

if [ "$(id -u)" -ne 0 ]; then
    echo "Run this installer as root." >&2
    exit 1
fi
if [ "$(uname -s)" != Linux ] || [ ! -d /run/systemd/system ]; then
    echo "A running Linux systemd system is required." >&2
    exit 1
fi
if [ "$#" -lt 1 ] || [ "$#" -gt 2 ] || [ ! -f "$1" ] || { [ "$#" -eq 2 ] && [ "$2" != --update ]; }; then
    echo "Usage: sh deploy/install.sh /path/to/precompiled/agent [--update]" >&2
    exit 1
fi
update=0
if [ "$#" -eq 2 ]; then
    update=1
    if [ ! -x /usr/local/bin/agent ]; then
        echo "Agent is not installed; run the installer without --update first." >&2
        exit 1
    fi
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
# 安装命令与 HTTP 服务共用进程锁，先等待旧 Agent 退出，避免并发变更。
systemctl stop xray-agent.service
if [ "$update" -eq 1 ] || /usr/local/bin/agent install-xray; then
    systemctl start xray-agent.service
else
    # Xray 安装失败仍启动管理接口，供查看故障和重试；安装脚本保留失败状态。
    systemctl start xray-agent.service
    echo "Default Xray installation failed; the Agent management service is available for recovery." >&2
    exit 1
fi
echo "Agent started. Open /docs for Swagger UI and enter the generated Bearer token."
