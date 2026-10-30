#!/bin/sh
# Shared installer interaction. Explicit arguments skip only their own question.
valid_port() {
    case "$1" in ''|*[!0-9]*) return 1 ;; esac
    [ "${#1}" -le 5 ] && [ "$1" -ge 1 ] && [ "$1" -le 65535 ]
}
ask_yes_no() {
    prompt=$1 default_answer=$2
    while :; do
        printf '%s ' "$prompt" >&3
        IFS= read -r answer <&3 || { echo 'Installation input closed.' >&2; exit 1; }
        case "$answer" in
            '') answer=$default_answer ;;
            y|Y|yes|YES) answer=yes ;;
            n|N|no|NO) answer=no ;;
            *) printf '请输入 Y 或 N。\n' >&3; continue ;;
        esac
        return
    done
}
ask_port() {
    port_label=$1 port_default=$2
    ask_yes_no "$port_label 是否使用默认端口 ${port_default}？[Y/n]" yes
    selected_port=$port_default
    if [ "$answer" = no ]; then
        while :; do
            printf '请输入 %s 端口：' "$port_label" >&3
            IFS= read -r selected_port <&3 || exit 1
            if valid_port "$selected_port"; then break; fi
            printf '端口必须是 1–65535。\n' >&3
        done
    fi
}
resolve_options() {
    # This executable came from the verified release. Output is numeric/boolean only.
    install_defaults=$("$agent_binary" install-defaults) || exit 1
    for key in agent_port grpc_port web_port; do
        value=$(printf '%s\n' "$install_defaults" | awk -F= -v key="$key" '$1==key {print $2}')
        valid_port "$value" || { echo "Invalid installed setting: $key" >&2; exit 1; }
        case "$key" in agent_port) previous_agent_port=$value ;; grpc_port) previous_grpc_port=$value ;; web_port) previous_web_port=$value ;; esac
    done
    previous_web_enabled=$(printf '%s\n' "$install_defaults" | awk -F= '$1=="web_enabled" {print $2}')
    case "$previous_web_enabled" in yes|no) ;; *) echo 'Invalid Web state.' >&2; exit 1 ;; esac
    agent_port=${arg_agent_port:-$previous_agent_port}
    grpc_port=${arg_grpc_port:-$previous_grpc_port}
    web_port=${arg_web_port:-$previous_web_port}
    web_enabled=${arg_web:-$previous_web_enabled}
    for port in "$agent_port" "$grpc_port" "$web_port"; do valid_port "$port" || { echo "Invalid port: $port" >&2; exit 1; }; done
    if [ "$update" -eq 0 ] && [ "$non_interactive" -eq 0 ]; then
        if ! ( : </dev/tty ) 2>/dev/null; then
            echo 'No terminal available; pass --non-interactive to use explicit settings/defaults.' >&2
            exit 1
        fi
        exec 3<>/dev/tty
        if [ -z "$arg_agent_port" ]; then ask_port 'Agent HTTP' "$agent_port"; agent_port=$selected_port; fi
        if [ -z "$arg_grpc_port" ]; then ask_port 'Xray 管理 gRPC' "$grpc_port"; grpc_port=$selected_port; fi
        if [ -z "$arg_web" ]; then
            if [ "$previous_web_enabled" = yes ]; then ask_yes_no '是否继续启用 Web 服务？[Y/n]' yes
            else ask_yes_no '是否启用 Web 服务？[y/N]' no; fi
            web_enabled=$answer
        fi
        if [ "$web_enabled" = yes ] && [ -z "$arg_web_port" ]; then ask_port 'Web' "$web_port"; web_port=$selected_port; fi
        exec 3>&-
    fi
    if [ "$agent_port" -eq "$grpc_port" ] || { [ "$web_enabled" = yes ] && { [ "$web_port" -eq "$agent_port" ] || [ "$web_port" -eq "$grpc_port" ]; }; }; then
        echo 'Enabled services must use different ports.' >&2
        exit 1
    fi
}
