# Rust Xray Agent

面向 Linux/systemd 的 HTTP Agent。Agent 与 Xray 均使用 root，支持官方版本安装、失败回滚、成功清理历史版本、配置持久化及 Xray 管理 gRPC。

适配基准为 **Xray v26.10.21**。允许安装其他官方版本，但其配置、协议和 RPC 必须能通过兼容性与就绪检查。上游缺失的 RPC 返回 `501`，不会模拟成功。

## 部署

需要运行中的 systemd 240+、x86_64 或 aarch64，以及可访问 GitHub Release 的网络。

发布首个稳定版本后，一键安装最新版本：

```sh
curl -fsSL https://github.com/xiaoxintang/agent/releases/latest/download/install.sh | sudo sh
```

安装指定版本：

```sh
curl -fsSL https://github.com/xiaoxintang/agent/releases/latest/download/install.sh | sudo sh -s -- --version v0.1.0
```

脚本自动识别 amd64/arm64，下载对应安装包，校验 Release 中的 SHA-256，然后初始化配置并启用 systemd 服务。重复执行用于更新 Agent，保留现有 Token、SQLite 和 Xray 配置。使用 fork 时传入 `--repo OWNER/REPO`。下载脚本不依赖 Rust、Python 或 C 编译器。

离线部署时解压对应 Release 安装包，在解压目录执行：

```sh
sudo sh deploy/install.sh ./agent
```

也可以在源码仓库中使用已有的预编译程序：

```sh
sudo sh deploy/install.sh /path/to/agent
```

脚本不执行构建。程序安装到 `/usr/local/bin/agent`，运行 `agent init` 后启动 `xray-agent.service`。首次初始化向终端输出随机 Bearer Token，重复初始化保留凭据。Token 保存于 `/etc/xray-agent/agent.toml`，权限 `0600`；日志不输出 Token。自定义 unit 设置使用 systemd drop-in；更新会替换由 Agent 管理的 unit。

默认 HTTP 地址为 `0.0.0.0:8080`，Xray gRPC 地址为 `127.0.0.1:10085`。管理接口使用 Bearer Token，HTTP 本身不提供 TLS；远程部署通过已有 HTTPS 反向代理或受信网络访问。

首次通过 HTTP 安装 Xray 后才创建、启用并启动 `xray.service`。Agent 不依赖 Xray 已安装或已启动。已有非本 Agent 管理的 Xray 安装或 unit 会产生冲突，不会直接覆盖。

```sh
export AGENT_URL=http://127.0.0.1:8080
read -r -s AGENT_TOKEN
export AGENT_TOKEN
curl -H "Authorization: Bearer $AGENT_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"version":"v26.10.21"}' "$AGENT_URL/api/v1/xray/install"
```

`read -s` 示例适用于 Bash/Zsh。安装及版本切换为同步请求，包含下载和健康检查，调用方需留出足够的 HTTP 超时时间。客户端断开不会取消已经开始的操作。

## 命令行

安装后可以直接执行：

```sh
agent -v                 # 版本号，也支持 --version、-V
agent version            # 版本号子命令，无需 root
agent --help             # 所有命令与参数
sudo agent init          # 初始化，重复执行保留配置和 Token
sudo agent serve         # 前台运行 HTTP Agent
agent openapi --output openapi.json
```

`prepare-xray`、`restore-xray` 为 systemd 使用的辅助命令。常驻服务由 `systemctl status/start/stop/restart xray-agent.service` 管理；Xray 进程通过 HTTP 管理接口控制。

## GitHub Actions 打包与发布

工作流位于 `.github/workflows/release.yml`：

- 推送 `main`/`master`、提交 PR 或手动触发：执行检查与测试，为 Linux amd64、arm64 构建静态 musl 二进制并上传安装包到 Actions Artifacts。
- 推送 `v*` 标签：上述步骤全部成功后创建 GitHub Release，上传两种架构的安装包、`install.sh` 和 `SHA256SUMS`。
- 标签必须与 `Cargo.toml` 的 `package.version` 一致；例如版本 `0.1.0` 对应标签 `v0.1.0`。预发布标签生成 prerelease，不替换 latest 稳定版本。

Rust 构建缓存复用 Cargo 依赖及 `target` 编译产物，按 amd64/arm64 的 musl target 隔离。`main` 成功构建后保存缓存，标签发布和 PR 读取复用；工具链、Cargo 配置及 lockfile 参与缓存键计算。首次构建需要建立缓存，后续命中缓存时可减少下载和编译时间。

首次发布示例：

```sh
git push origin main
git tag v0.1.0
git push origin v0.1.0
```

若默认分支为 master，相应替换第一条命令的分支名。发布新版本时先更新 Cargo 版本和 lockfile，再提交并推送匹配的标签。工作流使用仓库自带的 `GITHUB_TOKEN`，发布 job 需要仓库允许 `contents: write`。

Release 文件示例：

```text
agent-v0.1.0-linux-amd64.tar.gz
agent-v0.1.0-linux-arm64.tar.gz
install.sh
SHA256SUMS
```

每个安装包包含 `agent`、本地安装脚本、两个 systemd unit、README、生成的 `openapi.json` 及上游许可证信息。CI 会检查 `agent -v` 与 Cargo 版本一致，并执行帮助和 OpenAPI 导出命令。

## 文档与接口

- `/docs`：内置 Swagger UI，点击 **Authorize** 输入 Token。
- `/openapi.json`：由 `utoipa`、接口注册表和固定版本的 Protobuf descriptor 生成的 OpenAPI 3.1。
- `agent openapi --output openapi.json`：离线导出，无需 root 或初始化。

文档端点不需要 Token，也不包含主机配置或凭据。所有管理接口位于 `/api/v1/xray`，均要求鉴权。请求体上限为 4 MiB。

| 接口 | 用途 |
| --- | --- |
| `POST /install` | 下载并切换到指定官方版本，默认 v26.10.21 |
| `GET /versions` | 列出本地版本、活动标记、架构和失败信息 |
| `DELETE /versions/{version}` | 删除非活动且未被未完成事务引用的版本 |
| `GET /status` | 查询进程、版本、配置修订和待确认操作 |
| `GET /capabilities` | 查看适配的 RPC 和上游限制 |
| `POST /start`、`POST /stop`、`POST /restart` | 控制当前活动版本 |
| `GET /config`、`PUT /config` | 读取配置快照，或替换完整配置并重启 |
| `GET/POST /inbounds`、`DELETE/PATCH /inbounds/{tag}` | 入站管理；PATCH 仅接受支持的用户操作 |
| `GET/POST /inbounds/{tag}/users`、`DELETE /inbounds/{tag}/users/{email}` | 用户查询、添加、删除 |
| `GET /inbounds/{tag}/users/count` | 用户数量 |
| `GET/POST /outbounds`、`DELETE/PATCH /outbounds/{tag}` | 出站管理；基准版本没有可执行的 AlterOutbound 操作 |
| `POST /stats/read`、`POST /stats/query` | 读取或查询计数器，可带 reset |
| `GET /stats/online`、`GET /stats/online-ips` | 按 name 查询在线数、IP |
| `GET /stats/online-users`、`POST /stats/users/query`、`GET /stats/system` | 在线用户、流量及运行统计 |
| `GET/POST /routing/rules`、`DELETE /routing/rules/{ruleTag}` | 动态路由规则及负载均衡配置 |
| `POST /routing/test`、`GET /routing/events` | 路由测试及 SSE 订阅 |
| `GET /routing/balancers/{tag}`、`PUT /routing/balancers/{tag}/target` | 负载均衡状态与 override |
| `POST /logger/restart`、`GET /observatory/outbounds` | 日志重开、探测查询 |

用户查询可带 `?email=...`；入站列表可带 `?isOnlyTags=true`；在线统计必须带 `?name=...`；SSE 使用 `?fieldSelectors=inbound,outbound`。路径中的 tag、email 和统计 name 应做 URL 编码。

### 动态修改示例

新增入站直接提交普通 Xray JSON，不提交 Protobuf 或 Base64：

```sh
curl -H "Authorization: Bearer $AGENT_TOKEN" -H 'Content-Type: application/json' \
  -d '{"tag":"vless-in","listen":"127.0.0.1","port":10086,"protocol":"vless","settings":{"decryption":"none","clients":[]}}' \
  "$AGENT_URL/api/v1/xray/inbounds"

curl -H "Authorization: Bearer $AGENT_TOKEN" -H 'Content-Type: application/json' \
  -d '{"email":"alice@example.com","id":"e6b878f7-893e-482f-a0cc-c575e97fe040","level":0}' \
  "$AGENT_URL/api/v1/xray/inbounds/vless-in/users"

curl -H "Authorization: Bearer $AGENT_TOKEN" -H 'Content-Type: application/json' \
  -d '{"routing":{"rules":[{"ruleTag":"example-direct","domain":["domain:example.com"],"outboundTag":"direct"}]},"shouldAppend":true}' \
  "$AGENT_URL/api/v1/xray/routing/rules"
```

配置类变更返回 `{"revision":2,"response":{}}`。返回成功表示运行时操作、SQLite 提交、启动文件发布均已完成，未执行 Xray 重启。

PATCH 入站支持 `{"operation":"add_user","user":{...}}` 或 `{"operation":"remove_user","email":"..."}`。协议用户使用对应原生字段：VLESS/VMess 的 `id`、Trojan/Shadowsocks 的 `password`、Hysteria 的 `auth`、MASQUE 的 `pass`、WireGuard 的 peer 字段。动态用户要求非空 email。

路由 `shouldAppend` 默认为 `false`，会替换规则和 balancer，并清除已有 override；设为 `true` 才会追加。动态路由只接受 `rules`、`balancers`，全局 `domainStrategy`、`script` 需要完整配置替换。普通新增、删除操作均同步修改 SQLite 和启动 JSON。

### 完整配置替换

`GET /config` 返回 `{revision, config, overrides}`。修改其中的 `config` 后，将它单独作为 `PUT /config` 请求体提交。必须保留管理 API 地址、必需的服务、`stats`，并为入站、出站使用唯一非空 tag。

PUT 会执行配置验证并明确重启 Xray，失败则恢复之前的配置。与动态接口不同，任意完整配置无法保证热更新。

## 持久化与异常恢复

SQLite 位于 `/var/lib/xray-agent/state.db`，使用 WAL 和 `synchronous=FULL`。保存完整原生 JSON 快照、修订号、override 和操作阶段。`/etc/xray/config.json` 是已提交配置的生成物，不是第二份权威来源；人工编辑会在下一次生成时被覆盖。

动态变更先生成候选配置，用当前版本的 `xray convert pb` 及 `run -test` 验证，再由 Rust 直接发起 gRPC。调用前保存恢复记录，调用成功后提交快照并原子发布启动文件。配置转换使用临时文件，不运行第二个代理实例。

gRPC 与 SQLite 不具备跨进程原子事务。调用超时、进程中断、提交或发布失败时，接口不会误报成功：

- `/status` 的 `sync_state` 显示 `needs_confirmation` 或 `publication_pending`，并列出未完成操作。
- 禁止继续修改配置或切换版本；查询和停止仍可使用。
- Agent 启动时会对可完整读取的结果尝试核对。无法读取完整内容的路由追加不会仅凭规则 tag 判断成功。
- 未能确认时，由调用方显式 `POST /restart`，按最后已提交配置恢复并清除待确认状态；Agent 不因此自动重启。
- HTTP 超时不代表操作一定未执行，重试前先查询 `/status` 和相关资源。

systemd 的 `ExecStartPre` 使用 `agent prepare-xray` 从数据库生成启动文件；`ExecStartPost` 使用 `agent restore-xray` 恢复 override。Xray 自身崩溃时仍遵循 systemd 的 `Restart=on-failure`；这与 Agent 为一致性问题主动重启不同。

负载均衡 override 没有原生启动配置字段，因此单独持久化并在启动后恢复，存在短暂恢复窗口。流量计数、在线状态、计数器清零、日志重开不作为配置保存。

## 多版本行为

程序和随包资源放在 `/opt/xray/versions/{version}`，`/opt/xray/current` 原子切换。新版本先验证当前配置，再启动；30 秒内等待就绪并连续观察 10 秒。失败回滚到旧版本，保留失败候选供查看或删除。

成功后清理全部非活动版本，配置和 SQLite 不在版本目录中，不受清理影响。清理失败时返回 `cleanup_pending`，保留新版本运行。相同活动版本重复安装不重复下载，会重新检查就绪并清理历史版本。活动版本和未完成事务引用的版本不可删除。

安装操作阶段保存在 SQLite。中断的未提交切换会回滚，已提交切换继续清理，避免旧版本已删除后再次尝试回滚。

## 上游与权限边界

- v26.10.21 管理 API 共 26 个 RPC；代理的 gRPC 传输不属于管理 API。
- v26.10.21 路由订阅没有接入统计通道，SSE 会保留上游错误。
- v26.10.21 定义了 AlterOutbound，但没有具体 OutboundOperation，PATCH 返回 `501`。
- Observatory 需要完整配置中启用探测功能及 `ObservatoryService`；默认不指定外部探测地址。
- 无法在普通 Xray JSON 中准确保存的动态修改会在调用前拒绝，例如将 Shadowsocks 2022 单用户入站动态变为多用户，或删除其最后一个客户端导致启动配置退回单用户模式。
- 查询响应遵循 Protobuf JSON：64 位整数使用十进制字符串，已知 `TypedMessage.value` 展开为对象。

Xray 使用 root。unit 设置 `NoNewPrivileges`、`ProtectSystem=strict`、`ProtectHome`，默认仅保留 `CAP_NET_BIND_SERVICE`。需要 TUN/透明代理时，通过 systemd override 明确增加所需能力；这并不能提供独立用户隔离，Agent 本身也拥有安装及服务管理权限。

```sh
sudo systemctl edit xray.service
```

例如确实需要网络管理能力时配置：

```ini
[Service]
CapabilityBoundingSet=CAP_NET_BIND_SERVICE CAP_NET_ADMIN CAP_NET_RAW
AmbientCapabilities=CAP_NET_BIND_SERVICE CAP_NET_ADMIN CAP_NET_RAW
```

权限或 unit 修改需要重新加载并由操作者显式重启；证书、日志路径也必须满足 unit 的文件系统限制。

## 验证

不触发项目编译的检查：

```sh
python3 scripts/verify_sources.py
cargo fmt --all -- --check
sh -n deploy/install.sh
sh -n install.sh
sh -n scripts/package.sh
python3 -m unittest discover -s tests -p 'test_*.py'
```

发行包测试需要 Python 3.11+，使用临时目录和模拟下载、部署命令验证架构选择、指定版本、校验失败阻断及包内容，不执行本机安装。打包脚本仅由 CI 在构建完成后调用，不自行构建程序。

Rust 测试覆盖 Token、CLI 版本参数、API 鉴权、OpenAPI/RPC 覆盖、编解码、配置变更不重启、超时阻塞写入、数据库提交失败、文件发布失败、版本删除保护及 ZIP 路径处理。GitHub Actions 按本项目的发布流程执行测试和构建；本地仅在用户明确要求时运行构建或编译测试命令。构建使用纯 Rust protox，无需安装 protoc；bundled SQLite 需要 C 编译工具链。

真实 systemd 验收请在独立 Linux 测试机进行：

1. 部署并安装基准版本，确认两个 unit 状态、版本列表与 `/status`。
2. 记录 Xray `InvocationID`，动态增删入站、用户和路由；确认 ID 未变、JSON 与数据库修订已更新。
3. 显式重启，确认配置和 override 恢复。
4. 安装另一兼容版本，确认成功后只保留活动版本；用不兼容配置验证失败回滚。
5. 在 gRPC 调用前后及文件发布阶段中断 Agent，确认恢复状态和显式重启流程。

上游 proto 固定到 `proto/upstream.json` 中的提交并记录逐文件 SHA-256，许可证见 `proto/LICENSE`。构建不访问上游仓库下载 proto 或 Swagger UI 资源。
