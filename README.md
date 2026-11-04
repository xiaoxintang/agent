# Linux Panel

轻量化、自托管 Linux 多节点管理面板。架构与分阶段验收标准以
[ARCHITECTURE_PLAN.md](ARCHITECTURE_PLAN.md) 为准。

当前推进到 **Phase 8**：认证/WSS/Linux 指标、受管 Xray 生命周期、VLESS+REALITY 入站/客户端与原子配置修订，以及真实 StatsService 流量、小时图表和最终一致配额/到期策略。Agent 保持非 root，通过固定 socket 的短生命周期 helper 操作 Xray；已实现独立安装器；CI 与双架构真实维护验收已通过，正式签名发布正按 Phase 8 执行。

## 环境

- Rust 1.96.0（Rust 2024）；建议通过 rustup 安装，`rust-toolchain.toml` 锁定工具链并包含 rustfmt、Clippy。
- Node.js 22.23.2、pnpm 12.16.1（开发和构建使用）；`web/package.json` 的 `packageManager` 固定 pnpm 版本，其他 Node 版本范围见同文件。pnpm 安装方式见[官方说明](https://pnpm.io/installation)。
- 首次安装依赖需要访问 npm 和 crates.io。
- 本地支持 macOS/Linux；已在真实 Ubuntu 24.04 x86_64/systemd 节点通过 Phase 3 验收，并配置 Ubuntu CI。构建 bundled SQLite 需要 C 编译器（macOS Command Line Tools 或 Linux C 工具链）。Linux x86_64/aarch64 已执行真实 systemd 安装维护验收；正式签名发布留到 Phase 8。

## 从干净检出构建

必须先构建前端，随后 Rust 将生成的静态资源编译进服务端。缺少前端产物时构建会明确失败。

```sh
cd web
pnpm install --frozen-lockfile
pnpm run build
cd ..
cargo build --workspace --release --locked
# 仅首次运行；交互输入并确认密码，不回显
./target/release/panel-server init-admin --username admin
# 显式开启仅用于 loopback HTTP 的开发 Cookie
PANEL_DEV_HTTP=1 ./target/release/panel-server
```

浏览器访问 <http://127.0.0.1:8080>，使用刚初始化的账号登录，创建、重命名、撤销节点并查看审计。点击“注册 Agent”获取一次性令牌，接入后查看在线状态、Linux 资源和只读服务状态。已有管理员时跳过 `init-admin`；再次初始化会拒绝覆盖。

```sh
curl --fail http://127.0.0.1:8080/api/v1/health
# {"status":"ok","version":"0.1.0"}
./target/release/panel-agent
./target/release/panel-agent --version
./target/release/panel-installer --help
```

`panel-agent` 无子命令时打印版本后退出；Installer 提供 TTY 向导与 install/upgrade/repair/uninstall/status，生产安装方法见[运行说明](docs/operations.md)。
明文 HTTP 的 `PANEL_BIND` 只能设置为 loopback 地址，例如 `127.0.0.1:9090` 或 `[::1]:8080`。提供 `PANEL_TLS_CERT`/`PANEL_TLS_KEY` 后启用 HTTPS；公开 TLS 监听必须设置准确的 HTTPS `PANEL_ORIGIN`，不能启用开发 Cookie 模式。

运行构建后的三个二进制不需要 Node.js、web-dist 目录、Caddy、Nginx、数据库服务或容器。
服务在 `data/panel.sqlite3` 保存 SQLite 数据；新建目录为 0700、数据库为 0600，拒绝权限过宽或直接符号链接的数据路径。可用 `PANEL_DATA_DIR` 指定私有目录，初始化与运行必须使用同一目录。
Cookie 默认使用 Secure、HttpOnly、SameSite=Strict；没有 TLS 的本机浏览器开发显式使用 `PANEL_DEV_HTTP=1`，不能通过转发端口公开该服务。完整配置与会话行为见 [运行说明](docs/operations.md)。

## Agent 接入

跨主机接入先按 [运行说明](docs/operations.md) 配置用户证书 HTTPS/WSS。在页面签发 enrollment 后，在目标机器以普通用户运行，按提示粘贴令牌：

```sh
panel-agent --data-dir ./agent-data enroll --server wss://panel.example:8443/agent/v1/connect
panel-agent --data-dir ./agent-data run
```

替换域名/端口；私有 CA 在两条命令中都添加 `--ca-cert /path/to/trusted-ca.pem`。每个节点使用独立目录，身份库包含凭据，目录/文件权限为 0700/0600；重启用同一目录，不复制到其他节点。
本机明文开发在上述两条命令添加 `--allow-local-ws` 并使用 `ws://127.0.0.1:8080/agent/v1/connect`；Server 同时需要 `PANEL_DEV_HTTP=1`。没有跳过 TLS 证书验证的开关。
Agent enroll/run 必须使用普通用户，root 会被拒绝。Linux 每 5 秒上报真实 CPU/内存/根盘/网络，非 Linux 明确显示不可用；“查询 Agent 服务”和“查询 Xray 服务”仅读取固定 `panel-agent.service`/`xray.service` 的状态，不启动或变更服务。任务有 60 秒截止时间和持久去重。完整协议与 interrupted 语义见 [接口契约](docs/protocol.md)。
当前 wire 版本为 v6，Server/Agent 必须同步升级，保留各自私有数据目录。非 root systemd 模板和真实 Linux 验收方法见 [运行说明](docs/operations.md)。

## 本地开发

先执行一次前端构建，再分别启动 Rust 服务和 Vite：

```sh
cargo run --locked -p panel-server -- init-admin --username admin
PANEL_DEV_HTTP=1 PANEL_ORIGIN=http://127.0.0.1:5173 cargo run --locked -p panel-server
# 另一个终端
cd web
pnpm run dev
```

访问 <http://127.0.0.1:5173>；`/api` 代理到 127.0.0.1:8080。`PANEL_ORIGIN` 必须与浏览器地址中的来源完全一致；若 Vite 改用了其他端口，相应修改此变量。已有管理员时跳过初始化命令。
修改前端后，Vite 会热更新；要更新 Rust 内嵌页面，重新执行 `pnpm run build` 并重启 `cargo run`。

## 检查

`web/` 使用 Oxc 工具链：Oxfmt 负责格式化，Oxlint 负责 Lint（包括 Hooks、依赖数组和 React Refresh 检查）；Vite 8 使用 Oxc 转换 TS/JSX 并压缩 JavaScript。类型检查由 `tsc` 执行，测试使用 Vitest。

以下脚本会安装锁文件依赖并执行格式检查、Lint、类型契约检查、测试、前后端构建及独立二进制冒烟检查，CI 使用相同命令：

```sh
sh scripts/check.sh
```

Linux/systemd 主机额外以普通用户运行 `python3 scripts/verify-linux.py`，验证真实指标、WSS、服务白名单和进程权限边界。

各步骤的确切命令见 [scripts/check.sh](scripts/check.sh)。只修复格式：

```sh
cargo fmt --all
pnpm --dir web run format
```

`crates/common` 中的 Rust 类型是健康、认证、节点、指标、enrollment、任务和审计接口的类型来源。修改后执行：

```sh
pnpm --dir web run types
pnpm --dir web run types:check
```

生成文件随代码提交，不手工修改；契约检查会拒绝过期的 TypeScript 类型。

详见 [架构说明](docs/architecture.md)、[安全边界](docs/security.md)、[接口契约](docs/protocol.md) 和 [实施进度](docs/progress.md)。

签名发布验证见 [release.md](docs/release.md)，冷备份与恢复见 [backup-restore.md](docs/backup-restore.md)。正式发布结果和验收限制以 [进度记录](docs/progress.md) 为准。
