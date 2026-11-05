# Linux Panel

自托管 Linux 多节点管理面板：Rust Server、非 root Agent、受限 Xray helper，以及内嵌 React 管理界面。v0.1.0 已提供签名的 Linux x86_64/aarch64 发布包。

## 用户：验证并一键安装

中央节点需要 Debian/Ubuntu、systemd ≥249 和 root 权限。安装器没有默认密码；公网使用必须启用 HTTPS。每个被管理节点需要独立安装 Agent。

下载并运行一键安装脚本：

```sh
curl -fsSL https://github.com/xiaoxintang/agent/releases/latest/download/bootstrap.sh -o bootstrap.sh
sudo sh bootstrap.sh
```

交互安装会询问浏览器访问地址、首个管理员密码、节点名称及是否安装 Xray，默认自动申请并续期 Let's Encrypt 证书，也可使用现有证书。自动证书要求域名解析到服务器、公网 TCP 80 可达且未被占用；HTTPS 使用所配置的服务端口。密码隐藏输入。安装器创建 Server 和 Agent 专用账号、systemd 服务与受限 helper；首次合并安装会创建本机节点并注册 Agent。

如需 Server 与节点分开部署，在中央机使用 `--component server`，在每个工作节点使用同一验证流程并添加 `--component agent`。向导会询问 WSS 地址和一次性注册 token；token 在面板的节点页面生成。每台机器使用独立身份库，禁止复制另一节点的 Agent 数据目录。

非交互安装使用 root-owned、权限 0600 的配置文件保存配置和密码。字段及示例见[安装与运维说明](docs/operations.md)。安装器也可单独运行：

```sh
sudo panel-installer status
sudo panel-installer upgrade
sudo panel-installer repair --component all
sudo panel-installer uninstall --component all
```

普通卸载保留数据库、TLS 配置及本机 Agent 身份。永久清理必须输入准确确认：

```sh
sudo panel-installer uninstall --component all --purge --remove-xray --confirm 'PURGE PANEL DATA'
```

首次登录后，在节点列表使用“注册 Agent”“Xray 管理”和“入站与客户端”完成日常操作：创建 VLESS+REALITY 入站和客户端、关联后应用配置修订；“流量统计”查看 Xray 统计、小时图和客户端用量。设置配额或到期后，系统会从最后已提交的配置中移除客户端并提交新修订。配额最终一致，节点离线或配置应用失败时不会保证即时断流。完整限制见[运维说明](docs/operations.md)与[安全说明](docs/security.md)。

验证签名、校验清单和归档的完整步骤见[发布验证说明](docs/release.md)。建议先阅读[备份与恢复](docs/backup-restore.md)。

## 开发者：本地运行

依赖：Rust 1.96.0、Node.js 22.23.2 和 pnpm 12.16.1。pnpm 版本由 `web/package.json` 固定。构建 Server 前必须先生成并嵌入前端资源。

```sh
pnpm --dir web install --frozen-lockfile
pnpm --dir web run build
cargo build --workspace --release --locked
./target/release/panel-server init-admin --username admin
```

初始化命令在终端中隐藏输入密码。启动本机开发 Server：

```sh
PANEL_DEV_HTTP=1 PANEL_ORIGIN=http://127.0.0.1:5173 ./target/release/panel-server
```

另一个终端启动 Vite：

```sh
pnpm --dir web run dev
```

打开 <http://127.0.0.1:5173>。Vite 把 `/api` 和 Agent WebSocket 请求代理到本机 Server，也可以通过修改代理目标连接远程 Server。Agent 本地开发需要显式启用 loopback 明文连接：

```sh
./target/release/panel-agent --allow-local-ws --data-dir ./agent-data enroll --server ws://127.0.0.1:8080/agent/v1/connect
./target/release/panel-agent --allow-local-ws --data-dir ./agent-data run
```

测试时在第一个终端保持 Server 运行，使用独立终端执行 Agent 命令。不要把该开发 HTTP/WS 配置公开到网络。真实安装测试只在专用、没有既有 Panel 安装的 Linux VM 上运行。

## 开发者：检查与测试

完整的格式、Lint、Rust/TypeScript 契约、类型检查、单元与集成测试、前后端构建及独立二进制冒烟检查：

```sh
sh scripts/check.sh
```

也可以单独运行前端和 Rust 检查：

```sh
pnpm --dir web run format:check
pnpm --dir web run lint
pnpm --dir web run types:check
pnpm --dir web run typecheck
pnpm --dir web test
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

真实 Linux/systemd 指标和权限边界测试使用 Linux 普通用户运行：

```sh
python3 scripts/verify-linux.py --bin-dir target/release
```

完整安装、升级失败回滚、备份恢复和公开 bootstrap 测试需要一个专用 root Linux VM：

```sh
sudo python3 scripts/verify-install.py --bundle ROOT_OWNED_BUNDLE --sha256 TRUSTED_BUNDLE_JSON_SHA256 --wizard --backup-restore
sudo python3 scripts/verify-bootstrap.py --bundle ROOT_OWNED_BUNDLE --bootstrap scripts/bootstrap.sh --owned-vm
```

这两个验收脚本会创建并清理自己的测试安装，不能对现有生产服务器运行。真实浏览器 HTTPS E2E 由 CI 在上述专用 VM 上执行。安装流程、包制作和安全边界详见[运维](docs/operations.md)、[安全](docs/security.md)、[架构](docs/architecture.md)和[实施进度](docs/progress.md)。
