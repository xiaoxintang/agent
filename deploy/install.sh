#!/bin/sh
# Online/offline deployment of prebuilt, exactly matched components.
set -eu
usage() {
    echo 'Usage: sh deploy/install.sh /path/to/agent [--update] [--non-interactive]'
    echo ' [--agent-port PORT] [--grpc-port PORT] [--web-port PORT] [--web yes|no]'
    echo ' [--web-backend /path/to/agent-web --web-frontend /path/to/frontend]'
}
[ "$#" -gt 0 ] || { usage >&2; exit 1; }
agent_binary=$1; shift
[ -f "$agent_binary" ] || { usage >&2; exit 1; }
update=0 non_interactive=0
arg_agent_port= arg_grpc_port= arg_web_port= arg_web=
web_backend= web_frontend=
while [ "$#" -gt 0 ]; do
    case "$1" in
        --update) update=1; shift ;;
        --non-interactive) non_interactive=1; shift ;;
        --agent-port|--grpc-port|--web-port|--web|--web-backend|--web-frontend)
            [ "$#" -ge 2 ] && [ -n "$2" ] || { echo "Missing value for $1" >&2; exit 1; }
            case "$1" in --agent-port) arg_agent_port=$2 ;; --grpc-port) arg_grpc_port=$2 ;; --web-port) arg_web_port=$2 ;; --web) arg_web=$2 ;; --web-backend) web_backend=$2 ;; --web-frontend) web_frontend=$2 ;; esac
            shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) echo "Unknown argument: $1" >&2; exit 1 ;;
    esac
done
[ "$(id -u)" -eq 0 ] || { echo 'Run this installer as root.' >&2; exit 1; }
[ "$(uname -s)" = Linux ] && [ -d /run/systemd/system ] || { echo 'A running Linux systemd system is required.' >&2; exit 1; }
if [ "$update" -eq 1 ] && [ ! -x /usr/local/bin/agent ]; then echo 'Agent is not installed; run the installer without --update first.' >&2; exit 1; fi
case "$arg_web" in ''|yes|no) ;; *) echo '--web must be yes or no.' >&2; exit 1 ;; esac
script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
. "$script_dir/options.sh"
resolve_options
for unit in xray-agent.service xray-web.service; do
    if [ -e "/etc/systemd/system/$unit" ] && [ "$(head -n 1 "/etc/systemd/system/$unit")" != '# Managed by xray-agent' ]; then
        echo "An existing unmanaged $unit was found; inspect it before replacing it." >&2; exit 1
    fi
done
version=$("$agent_binary" -v)
version=${version#agent }
case "$version" in ''|*[!A-Za-z0-9._+-]*) echo 'Invalid Agent version.' >&2; exit 1 ;; esac
if [ "$web_enabled" = yes ]; then
    [ -n "$web_backend" ] && [ -n "$web_frontend" ] || { echo 'Matching Web backend and frontend artifacts are required.' >&2; exit 1; }
    "$agent_binary" verify-web --backend "$web_backend" --frontend "$web_frontend"
    for command in getent groupadd useradd chown chgrp; do command -v "$command" >/dev/null 2>&1 || { echo "Required command is missing: $command" >&2; exit 1; }; done
fi

# A durable rollback directory survives process termination and machine restart.
transaction=/var/lib/xray-agent/install-transaction
restore_install() {
    echo 'Restoring the previous installation...' >&2
    if [ -e /etc/systemd/system/xray-web.service ]; then systemctl stop xray-web.service || return 1; fi
    if [ -e /etc/systemd/system/xray-agent.service ]; then systemctl stop xray-agent.service || return 1; fi
    if [ -f "$transaction/agent" ]; then
        if [ -e /etc/systemd/system/xray.service ]; then systemctl stop xray.service || return 1; fi
        install -m 0755 "$transaction/agent" /usr/local/bin/agent.new || return 1
        mv -f /usr/local/bin/agent.new /usr/local/bin/agent || return 1
        rm -rf /etc/xray-agent || return 1
        if [ -d "$transaction/config" ]; then cp -a "$transaction/config" /etc/xray-agent || return 1; fi
        for file in state.db state.db-wal state.db-shm; do
            rm -f "/var/lib/xray-agent/$file" || return 1
            if [ -f "$transaction/$file" ]; then cp -a "$transaction/$file" "/var/lib/xray-agent/$file" || return 1; fi
        done
        if [ -f "$transaction/xray-config.json" ]; then cp -a "$transaction/xray-config.json" /etc/xray/config.json || return 1; fi
        for unit in xray-agent.service xray-web.service; do
            rm -f "/etc/systemd/system/$unit" || return 1
            if [ -f "$transaction/$unit" ]; then cp -a "$transaction/$unit" "/etc/systemd/system/$unit" || return 1; fi
        done
        rm -f /opt/xray-web/current || return 1
        if [ -L "$transaction/web-current" ]; then cp -a "$transaction/web-current" /opt/xray-web/current || return 1; fi
        systemctl daemon-reload || return 1
        for service in xray xray-agent xray-web; do
            if [ -f "$transaction/$service.enabled" ]; then systemctl enable "$service.service" || return 1; else systemctl disable "$service.service" 2>/dev/null || :; fi
            if [ -f "$transaction/$service.active" ]; then systemctl start "$service.service" || return 1; fi
        done
    elif [ -x /usr/local/bin/agent ]; then
        # No previous version exists. Keep the recovery Agent, with Web disabled.
        /usr/local/bin/agent init --web no || return 1
        if [ "${new_credentials:-0}" -eq 1 ]; then rm -f /etc/xray-agent/web-credentials.json || return 1; fi
        systemctl disable xray-web.service 2>/dev/null || :
        systemctl start xray-agent.service || return 1
    fi
}
if [ -f "$transaction/active" ]; then
    restore_install || { echo "Recovery failed; backups remain at $transaction" >&2; exit 1; }
fi
rm -rf "$transaction"
install -d -m 0700 "$transaction"
committed=0
finish() {
    code=$1
    trap - EXIT HUP INT TERM
    if [ -f "$transaction/active" ]; then
        if ! restore_install; then echo "Rollback failed; backups remain at $transaction" >&2; exit 1; fi
    elif [ "$committed" -eq 0 ]; then
        for service in xray xray-agent xray-web; do
            if [ -f "$transaction/$service.active" ]; then systemctl start "$service.service" || :; fi
        done
    fi
    if [ -n "${web_stage:-}" ] && [ -d "$web_stage" ]; then rm -rf "$web_stage"; fi
    rm -rf "$transaction"
    exit "$code"
}
trap 'finish $?' EXIT
trap 'exit 1' HUP INT TERM
for service in xray xray-agent xray-web; do
    if systemctl is-active --quiet "$service.service"; then touch "$transaction/$service.active"; fi
    if systemctl is-enabled --quiet "$service.service"; then touch "$transaction/$service.enabled"; fi
done
# Wait for in-flight Web tasks, then stop the Agent before taking SQLite backups.
if [ -e /etc/systemd/system/xray-web.service ]; then systemctl stop xray-web.service; fi
if [ -e /etc/systemd/system/xray-agent.service ] || [ -f "$transaction/xray-agent.active" ]; then systemctl stop xray-agent.service; fi
if [ "$grpc_port" -ne "$previous_grpc_port" ] && [ -e /etc/systemd/system/xray.service ]; then systemctl stop xray.service; fi
if [ -f /usr/local/bin/agent ]; then cp -a /usr/local/bin/agent "$transaction/agent"; fi
if [ -d /etc/xray-agent ]; then cp -a /etc/xray-agent "$transaction/config"; fi
if [ -f /var/lib/xray-agent/state.db ]; then "$agent_binary" install-backup --output "$transaction/state.db"; fi
if [ -f /etc/xray/config.json ]; then cp -a /etc/xray/config.json "$transaction/xray-config.json"; fi
for unit in xray-agent.service xray-web.service; do if [ -f "/etc/systemd/system/$unit" ]; then cp -a "/etc/systemd/system/$unit" "$transaction/$unit"; fi; done
if [ -L /opt/xray-web/current ]; then cp -a /opt/xray-web/current "$transaction/web-current"; fi
touch "$transaction/active"
install -d -m 0755 /usr/local/bin
install -m 0755 "$agent_binary" /usr/local/bin/agent.new
mv -f /usr/local/bin/agent.new /usr/local/bin/agent
new_credentials=0
if [ ! -f /etc/xray-agent/web-credentials.json ]; then new_credentials=1; fi
/usr/local/bin/agent init --agent-port "$agent_port" --grpc-port "$grpc_port" --web-port "$web_port" --web "$web_enabled"
install -m 0644 "$script_dir/xray-agent.service" /etc/systemd/system/xray-agent.service
if [ "$web_enabled" = yes ]; then
    getent group xray-web >/dev/null || groupadd --system xray-web
    id -u xray-web >/dev/null 2>&1 || useradd --system --gid xray-web --home-dir /var/lib/xray-web --shell /usr/sbin/nologin xray-web
    install -d -m 0700 -o xray-web -g xray-web /var/lib/xray-web
    chgrp xray-web /etc/xray-agent
    chmod 0710 /etc/xray-agent
    for file in web.toml web-credentials.json web-local-agent.json; do chgrp xray-web "/etc/xray-agent/$file"; chmod 0640 "/etc/xray-agent/$file"; done
    install -d -m 0755 /opt/xray-web/versions
    web_target="/opt/xray-web/versions/$version"
    web_stage=$(mktemp -d /opt/xray-web/versions/.staging-XXXXXX)
    install -m 0755 "$web_backend" "$web_stage/agent-web"
    cp -R "$web_frontend" "$web_stage/frontend"
    chmod -R a+rX "$web_stage"
    # Keep the old complete directory even when reinstalling the same version.
    if [ -e "$web_target" ]; then web_target="$web_target-$(date +%s)-$$"; fi
    mv "$web_stage" "$web_target"
    rm -f /opt/xray-web/current.new
    ln -s "$web_target" /opt/xray-web/current.new
    mv -Tf /opt/xray-web/current.new /opt/xray-web/current
    install -m 0644 "$script_dir/xray-web.service" /etc/systemd/system/xray-web.service
fi
systemctl daemon-reload
systemctl enable xray-agent.service
xray_failed=0
if [ "$update" -eq 0 ]; then
    if ! /usr/local/bin/agent install-xray; then xray_failed=1; fi
fi
if [ "$grpc_port" -ne "$previous_grpc_port" ] && [ -f "$transaction/xray.active" ]; then systemctl start xray.service; fi
systemctl start xray-agent.service
if [ "$web_enabled" = yes ]; then systemctl enable xray-web.service; systemctl start xray-web.service
elif [ -e /etc/systemd/system/xray-web.service ]; then systemctl disable xray-web.service; fi
/usr/local/bin/agent install-health
rm -f "$transaction/active"
committed=1
echo "Agent: http://<server>:$agent_port/docs"
if [ "$web_enabled" = yes ]; then
    echo "Web: http://<server>:$web_port"
    if [ "$new_credentials" -eq 1 ]; then /usr/local/bin/agent web credentials; else echo 'Existing Web account preserved. View with: sudo agent web credentials'; fi
fi
if [ "$xray_failed" -eq 1 ]; then
    echo 'Default Xray installation failed; the Agent management service is available for recovery.' >&2
    exit 1
fi
