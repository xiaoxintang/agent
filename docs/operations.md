# 运行说明

## Xray 受管生命周期（Phase 4）

页面“Xray 管理”提供固定版本的安装/升级、启动/停止/重启、上一版本回滚和保留数据的卸载。未经本机管理员配置 helper 时，明确返回 helper_unavailable，不会尝试 sudo 或改用 root Agent。
生产需要本机管理员预置 root-owned `panel-installer`、普通账号 `panel-agent`、`/etc/panel`（root 所有且不可被普通用户写入），以及 root-owned `/etc/panel/xray-helper.json`：`{"agent_uid":实际panel-agent账号的UID}`。UID 不得为 0；配置文件/祖先路径检查归属和写权限。把 `deploy/systemd/panel-xray-helper.socket`、`panel-xray-helper@.service` 安装到标准 unit 目录后由管理员 reload/启用 socket；这些文件现由 Phase 7 Installer 自动按归属部署。
socket 文件只允许 panel-agent 账号连接；systemd Accept=yes 每次请求启动一个 root helper，处理后退出，无长期 root Agent。helper 不能接受自定义 root/命令/unit/下载地址，不能作为通用执行器。生产目录/单位固定为 `/opt/panel/xray`、`/var/lib/panel/xray-helper`、`xray.service`；Xray 服务用 DynamicUser、NoNewPrivileges 和仅 CAP_NET_BIND_SERVICE 运行。root helper 通过 systemd 启动无 capability、独立 DynamicUser 的 10 秒临时验证任务，私有配置通过 LoadCredential 交付；保留 helper 沙箱，不放宽私有文件权限。
仅支持 catalog 中经过摘要固定的官方版本/架构，SHA-256 信任锚在代码，不是下载同源校验文件后盲目信任。版本目录与 manifest 共同记录归属/二进制 hash，current 原子切换；升级前验证现有配置，失败恢复旧版本。切换事务中断时保留检查点，先查询安装状态或回滚恢复上一状态，不重试原先不确定的 task ID。若在落盘/阶段提交时遇到其他不一致，保守拒绝并保留文件，需本机检查，不能通过删 manifest 强行接管。
安装先生成没有监听端口的最小完整配置，Phase 5 再管理实际入站。卸载保留 config.json/manifest 与数据，不删除非受管或已修改文件；远程生命周期不提供数据 purge；本机 Installer 的明确 purge 另有输入式确认。若已有 `/usr/local/bin/xray`、`/usr/bin/xray`、`/etc/xray` 或非归属 unit/drop-in，拒绝接管。

真实 Linux 用户级隔离验收（不接触生产路径）：

```sh
cargo build --release --locked -p panel-installer --example xray_acceptance
./target/release/examples/xray_acceptance
```

此 example 只以普通用户创建临时目录与唯一 `panel-phase4-*.service` 用户 unit，下载校验实际官方 Xray，验证升级健康失败/切换中断回滚与卸载，并检查未受管 sentinel 未改动。用户级验收不等同于生产 root socket/DynamicUser 的完整安装验收，后者由 Phase 7 真实安装验收覆盖。

当前同时提供受管 Xray 生命周期、配置与流量策略；自动证书管理仍不在 MVP 范围。

## 初始化管理员

构建完成后，在本机终端执行：

```sh
./target/release/panel-server init-admin --username admin
PANEL_DEV_HTTP=1 ./target/release/panel-server
```

第一条命令只执行一次，密码输入不回显并需确认，要求 12–128 字节且不含控制字符。管理员名为 3–64 个 ASCII 字母、数字、点、下划线或短横线。没有默认密码，不开放 HTTP 首次设置。再次初始化会失败，不能覆盖已有管理员。
自动化可使用 `--password-stdin`，从标准输入读取一个密码，可带一个末尾换行；不要把密码放进命令行参数或环境变量。CLI 不回显密码，不输出密码哈希。Installer 会以专用服务账号调用此入口；保留已有管理员。

## 配置

| 变量 | 默认值 | 限制 |
| --- | --- | --- |
| `PANEL_BIND` | `127.0.0.1:8080` | HTTP 必须为 loopback；TLS 可以显式绑定其他地址 |
| `PANEL_DATA_DIR` | 工作目录下 `data` | 初始化和运行需一致，新目录 0700、数据库 0600；拒绝权限过宽及直接符号链接 |
| `PANEL_ORIGIN` | 实际绑定地址的 origin | HTTP 为 loopback 来源；TLS 为 HTTPS 来源，公开监听须显式设置；无路径/查询/末尾斜杠 |
| `PANEL_DEV_HTTP` | 未设置 | 仅允许显式设置为 `1`，取消 Cookie 的 Secure 属性，供本机 HTTP 浏览器开发 |
| `PANEL_TLS_CERT` / `PANEL_TLS_KEY` | 未设置 | 必须一起提供，启用 rustls HTTPS；私钥文件仅所属用户可读写，例如 0600 |

默认 Cookie 名为 `__Host-panel_session`，包含 Secure、HttpOnly、SameSite=Strict、Path=/，不设置 Domain；本机开发模式改用 `panel_session`，其余属性保持。默认安全 Cookie 模式不保证浏览器接受纯 HTTP 上的 Cookie；HTTP 浏览器开发必须显式使用 `PANEL_DEV_HTTP=1`。
Vite 开发模式设置 `PANEL_ORIGIN=http://127.0.0.1:5173`，浏览器也使用这个准确地址；Vite 代理 `/api` 和 `/agent`。不要通过端口转发或代理公开明文开发服务。

## HTTPS 与 Agent

使用你已取得且主机名正确的证书，私钥权限限制为 0600。以下路径和域名需要替换为实际配置：

```sh
PANEL_TLS_CERT=/etc/panel/cert.pem PANEL_TLS_KEY=/etc/panel/key.pem \
PANEL_BIND=0.0.0.0:8443 PANEL_ORIGIN=https://panel.example:8443 \
./target/release/panel-server
```

TLS 模式禁止 `PANEL_DEV_HTTP`，Secure Cookie 默认开启。非 loopback TLS 监听必须显式提供准确的 HTTPS `PANEL_ORIGIN`。目前不提供 ACME、证书续期/热加载或生产安装脚本；自行维护证书生命周期。
登录页面后创建节点，点击“注册 Agent”生成 10 分钟、单次使用的 token。令牌只在当前弹窗展示；关闭立即清除，不进入查询/Mutation 缓存。每个节点在目标机器上使用一个独立私有目录，并以普通用户运行 Agent：

```sh
panel-agent --data-dir ./agent-data enroll --server wss://panel.example:8443/agent/v1/connect
panel-agent --data-dir ./agent-data run
```

enroll 交互粘贴 token，不回显；自动化可使用 `--token-stdin`，不得把 token 放入 URL/命令行参数。私有 CA 在 enroll 和 run 两条命令上都增加 `--ca-cert /path/to/trusted-ca.pem`，只添加可信根，不跳过主机名或有效期验证。
Agent 默认目录为 `agent-data`，新建目录 0700，身份库/锁文件 0600。身份库包含该节点凭据，必须像密码一样保护；同目录只允许一个 Agent 进程。enroll 后 run 从持久库重连，不再需要 enrollment token。重启时保持相同目录。复制身份库会复制节点身份，不能作为新增节点的方法。
凭据默认 365 天有效；重新给同节点签发并消费 enrollment 可轮换，原凭据和旧连接随即失效。重用目录只允许相同 Server URL 和 node_id，不允许悄悄改绑。若 token 消费后网络/磁盘故障使身份未保存，重新签发 token，不尝试复用已消费的 token。

纯本机开发需双方显式开启明文例外：Server 使用 `PANEL_DEV_HTTP=1`，Agent 两次命令都加 `--allow-local-ws`，Server URL 为 `ws://127.0.0.1:8080/agent/v1/connect`。Agent 拒绝公开 IP、DNS 名称或 localhost 的明文 WS，不提供 TLS insecure 选项。

Agent 每 15 秒心跳，45 秒超时。网络断开自动带抖动退避重连，认证/协议错误停止并报告脱敏错误。页面每 5 秒刷新节点状态。点击“查询 Agent 状态”创建有 60 秒截止时间的只读任务，返回实际版本/OS/架构；离线时等待重连，超过截止时间过期。
任务 receipt 和结果均持久化；重复任务不会重复执行，崩溃时无法确认的任务返回 interrupted 而不重做。管理员可以显式创建新的只读查询。撤销节点关闭连接、撤销凭据并取消任务，不执行远程卸载。

## Linux 采样与 systemd 边界

当前 wire 协议为 v6，升级时一起替换 Server 和 Agent，保留原有私有数据目录，数据库自动执行五次迁移。旧协议连接会被拒绝。现有账号、身份、AgentStatus 任务和 receipt 仍可读取。
Agent enroll/run **拒绝 root**，必须以普通用户执行；Linux 启动时设置 NoNewPrivileges，采样和 D-Bus 查询都不需要 sudo。不要授予 Agent sudo/polkit 服务管理权限或 capability。本阶段不存在特权 helper 或服务变更接口。
在节点列表点击“查看资源”，每 5 秒读取最新 CPU/内存/根文件系统/网络快照。CPU 为整机 tick 差分，内存已用=MemTotal−MemAvailable，磁盘是 `/` 所在文件系统。根盘可用空间可能少于总量减已用，因为保留块不计入可用。网络汇总非 lo 接口、包含虚拟接口，不作为实际外网流量或 Xray 计费用量。
首次/计数回退/网络接口变化时 CPU 或网络速率为“采样中/计数重置”；非 Linux、读取失败或超出采样边界时显示不可用，不能将 null 当成 0。Server 每节点只保留最新快照，15 秒未更新、节点离线或连接变更时显示历史数据；没有时序存储和遥测保留服务。
“查询 Agent 服务”仅查询 `panel-agent.service`，“查询 Xray 服务”仅查询 `xray.service`，经 system bus 读取状态，不加载/安装或改变服务。`not_loaded` 表示 systemd 当前没有加载对应 unit，不能证明 unit 文件不存在；没有 systemd 或总线策略不允许时显示明确原因。每次状态任务与失败/越权请求都有审计，不能传任意 unit 或 method。

`deploy/systemd/panel-agent.service` 是供后续安装使用的模板，本阶段不自动部署。它要求预先存在的 `panel-agent` 普通账号、`/usr/local/bin/panel-agent` 和以同账号 enrollment 的 `/var/lib/panel/agent/agent.sqlite3`。使用 StateDirectory/0700、UMask=0077、NoNewPrivileges、空 capability、ProtectSystem/ProtectHome/PrivateDevices 等约束。私有 CA 应放在该账号可读的位置并在 ExecStart 显式加 `--ca-cert`；不要通过提升权限解决证书/目录访问问题。
Installer 现已实现专用账号、验证二进制、systemd 与 socket 部署。底层只读指标验收仍使用独立临时进程；完整安装验收仅在确认没有已有 Panel 的专用测试主机运行。

## Linux 验收

原生 Linux release 构建后，以普通用户在真实 systemd 主机运行：

```sh
python3 scripts/verify-linux.py --bin-dir target/release
```

该脚本只使用 Python 标准库及已有 OpenSSL/systemd 工具：创建私有临时目录和一天期测试证书，启动随机 loopback HTTPS/WSS、初始化随机管理员并 enrollment；检查真实 `/proc`/statvfs 指标、所有 Agent 线程的 UID/NoNewPrivs/CapEff、两个固定 unit 状态及 5 类越权拒绝与审计。unit 模板只验证临时副本，不安装或启动服务。finally 停止自己的测试进程并删除测试证书/数据；不需要 root，也不安装依赖。GitHub Ubuntu CI 已加入此命令，但本次结果以实际执行记录为准。

## 会话、节点与审计

会话固定 1 小时，到期后重新登录；成功登录会旋转请求中已有的会话 Cookie。退出只撤销当前会话。尚无修改密码、管理其他会话或多管理员功能。
管理 API 都要求会话；修改还需准确 Origin 和 `X-CSRF-Token`。登录及 `/auth/me` 返回 CSRF token，客户端只在内存使用，不存储会话 token。
登录全局每分钟最多 10 次（含已通过 Origin 校验的无效 JSON 请求），最多同时校验 2 个密码；限流不依赖 Redis，重启会重置内存计数。
最多 100 个未撤销节点；创建后为 offline，只有认证连接及心跳才更新 online 和观测信息。撤销保留数据库记录和审计，不执行远程卸载。
审计只保存动作、结果、账号/节点 ID、时间及请求 ID，保留 90 天。启动或每次写入审计时清理过期记录，空闲时不会定时擦除；查询始终过滤超期记录并只返回最近 100 条。过期/撤销会话在启动及成功登录时清理。

SQLite 迁移内嵌且自动执行，使用 WAL、外键、5 秒 busy timeout、最多 4 个连接。仅支持单个 Server 写入实例。不要在数据库运行时手工复制单个主文件当作完整备份；正式备份/恢复流程留到硬化阶段。

## 配置管理（Phase 5）

在节点的“入站与客户端”创建 VLESS + REALITY 入站，填写监听 IP/端口、可用 TLS 1.3 目标、SNI 和偶数位十六进制 Short ID。服务端生成 X25519 密钥；读取 API 仅返回公钥。客户端使用全局稳定身份，可关联多个入站；创建时一次性显示的协议 UUID 需要安全保存，关闭窗口后不会再次读取。保存/删除/停用只是修改期望模型，点击“应用配置修订”才会排队执行并重启 Xray。

配置和任务数据库、helper 备份含私钥与客户端凭据，必须限制访问。公开任务与修订接口仅显示 revision/hash/结果，不提供原始 JSON 下载。每节点最多 16 个入站、配置中最多 256 条客户端关联；保留最近 20 个非等待修订及尚在执行的修订，已结束任务按既有 7 天策略清理，但最新期望修订的任务引用保留，以便长期离线后仍可协调。

helper 从类型化模型生成 JSON，核对摘要与单调修订，使用已验证的本机 Xray 通过管道执行实际 `run -test -format json -config /dev/stdin`。之后写入私有备份与恢复检查点，原子替换配置并重启；健康检查失败恢复旧配置与运行状态。中断后先查询 Xray 状态触发检查点恢复，再提交新修订。服务健康目前是 systemd active/liveness，不代表 REALITY 外部目标可达或完整客户端握手成功。

Agent 重连只重新排队最新的、因投递超时而未确认的期望修订；明确校验或健康失败不会无限重试。服务离线期间保留本机最后健康配置，不因控制面断开停止 Xray。Agent 与 Server 使用协议 v4，应同步升级。Xray 的 DynamicUser unit 只增加低端口绑定所需的 CAP_NET_BIND_SERVICE；Agent 仍不具备 capability。

## 流量与配额（Phase 6）

应用新修订后，受管 Xray 的 StatsService 固定监听 `127.0.0.1:10085`，该端口不可用于业务入站。只启用 StatsService，不启用 HandlerService；VLESS 出站阻止回环目标，避免通过受管代理访问本机统计接口。统计客户端不重置 Xray 计数，使用固定版本官方 protobuf 的字段与 RPC 路径，通过 tonic/prost 读取，响应限制 64 KiB/512 个计数。

Agent 每 15 秒采样，以固定 `xray.service` 的 systemd InvocationID 标识进程；采样前后核对 invocation，重启夹在读取中时不提交。服务端以节点/客户端持久化基线，对重复、乱序、进程替换、计数下降和异常跳变只建基线，不能产生负值或巨额假增量。首轮及重启后首轮不收费，因此可能少计这些窗口；失联后若 Xray 没重启，可从原基线继续计算累计差值。仅接收该节点已关联客户端的数据。

每小时聚合保留 7 天，浏览器图表以服务端收件时间归桶；节点/客户端累计和全局客户端 used_bytes 不随图表清理归零。删除客户端同时删除对应统计；没有自动月度重置。REST 字节数用十进制字符串，UI 以 MiB 展示，数据库持久累计值在 JavaScript 安全整数上限饱和，以避免溢出。可设置总 quota_bytes（正数字符串，空为不限）、UTC 秒 expiry_at（空为不过期）；UI 到期输入按浏览器本地时区转换。

在线统计写入后及服务端每 5 秒检查策略，达限或到期自动从最后已提交期望快照移除客户端，并排队新修订/审计；不会顺便应用未提交的入站草稿。提高配额或延后到期会清除策略限制标记，但需要手动应用配置才能恢复客户端。配置应用采用验证后整份重启，可能短暂中断同节点其他连接。

配额是最终一致限制：通常需一次 15 秒采样及任务执行时间，到期最多再加 5 秒策略轮询；节点离线、统计不可用或应用失败时没有硬性断流保证。离线节点继续最后健康配置，恢复连接后协调最新未确认修订。UI 明确标注统计不可用/过期，保留历史用量，不把无数据当作零流量。现有 Phase 5 配置需重新应用一次以启用统计；Server/Agent 同步升级为协议 v5。

## 安装、维护与卸载（Phase 7）

支持 Debian/Ubuntu、systemd ≥249、Linux x86_64/aarch64。生产只运行 Rust Server/Agent、独立 Xray 和按连接激活的短期 helper；不需要 Node、数据库服务或反向代理。需要 root 的是非驻留 Installer，长期 Server/Agent 始终是专用非 root 账号。

安装向导先选择组件：`Server + Agent` 同机安装面板与节点，`Server` 仅安装面板，`Agent` 仅安装节点并连接已有面板。接着选择配置模式：`Quick` 使用默认数据目录并启用开机启动；`Custom` 允许调整数据目录和开机启动。两种模式都需要配置连接和管理员信息。选择不启用 HTTPS 时可通过 IP 地址使用 HTTP 访问，需填写浏览器实际访问的完整 IP 和端口；密码、会话和传输数据均可能被窃听。同机 Agent 使用 loopback WS，远程 Agent 仍须使用 WSS。

HTTP 向导会根据监听地址或本机路由推荐 IPv4 和监听端口；该值可以修改。NAT 后的公网 IP 或端口可能与推荐值不同，必须填写浏览器实际使用的 Origin。首次同机安装成功注册 Agent 后自动关联本机节点；已有安装可在节点管理中手动设置，关联由所有管理员共享。资源、Xray、配置、流量和 Agent 页面均针对该本机节点。

认证后的 `GET /api/v1/local-node` 返回 `{"node_id":字符串或null}`；`PUT /api/v1/local-node/{id}` 将有效节点设为本机，要求会话、Origin 和 CSRF，并写入审计。撤销本机节点会清除关联。

先取得经可信发布渠道验证的 bundle。`scripts/package.py` 只打包已有 Linux ELF，不自动构建；`bundle.json` 固定版本、架构和三个二进制摘要。Installer 的 `--bundle-sha256` 是**元数据文件**摘要，必须从已经认证的发布/交付渠道取得；bootstrap 的 `--sha256` 则是**整个压缩包**摘要，两者不可混用。不能把同一不可信下载旁的 checksum 当作独立信任锚。

```sh
sudo ./panel-installer install --bundle /root/panel-bundle --bundle-sha256 TRUSTED_BUNDLE_JSON_SHA256
# 无子命令且有 TTY 时也可进入菜单；向导显示计划并确认。
sudo panel-installer status
sudo panel-installer upgrade --component all --bundle /root/new-bundle --bundle-sha256 TRUSTED_NEW_METADATA_SHA256 --version 0.1.0
sudo panel-installer repair --component all
```

自动化使用 root-owned 0600 JSON（祖先目录也不可被其他用户写入；root 所有的 sticky 临时目录除外）：

```json
{
  "settings": {
    "server_bind": "0.0.0.0:8443",
    "server_origin": "https://panel.example:8443",
    "local_http": false,
    "acme_domain": "panel.example",
    "admin_username": "admin",
    "agent_server": "wss://panel.example:8443/agent/v1/connect",
    "agent_name": "first-node",
    "autostart": true,
    "install_xray": false
  },
  "admin_password": null,
  "enrollment_token": null
}
```

首次 Server 安装必须在该私有文件填写自己的 12–128 字节密码，或改用 TTY 隐藏输入；`null` 没有默认密码含义，会被拒绝。Agent-only 首次接入需要面板的一次性 enrollment token；all 安装可以使用内存中的管理员密码通过 Server API 创建节点/令牌并注册，随后注销设置会话。私有 CA 可显式设置 `agent_ca`；不跳过 TLS 验证。设置 `local_http:true` 可启用明文 HTTP，Origin 必须是实际访问的 IP；同机 Agent 仍仅允许字面 loopback WS，Server + Agent 须监听 loopback 或所有接口。服务端数据默认 `/var/lib/panel/server`，可在首次安装选择 `/var/lib/panel/` 下的一个其他直接子目录（排除 agent），此后不通过安装器迁移数据路径。

```sh
sudo ./panel-installer install --component all --non-interactive --config /root/panel-install.json --bundle /root/panel-bundle --bundle-sha256 TRUSTED_BUNDLE_JSON_SHA256
```

安装器不会覆盖已有管理员；重复 install 拒绝替换已安装组件，使用 upgrade/repair。二进制和 unit/config 的归属、摘要保存在 `/var/lib/panel-installer/manifest.json`；服务数据另有归属标记。升级停下所选服务，备份静态文件与 SQLite/WAL，原子激活后实际启动并检查 HTTPS/服务状态；失败恢复原文件、身份和数据。持久 journal 在下一次 status/repair/维护命令执行时恢复未完成事务。不要删除 manifest 或检查点来绕过拒绝；未知文件、软链接、外来 unit/drop-in 会使操作停止。

手动 TLS 私钥原件在 `/etc/panel` 为 root-owned 0600。systemd credential 交给服务后，Server 在 0700 RuntimeDirectory 中获得 0600 临时副本，保持 Server 的严格私钥权限校验。Xray 验证由有 10 秒/128 MiB 限额的独立 DynamicUser 临时 unit 完成；通过私有 credential 读取 JSON，验证任务自动回收，helper 沙箱不获得 CAP_SETUID。

自动证书使用 `acme_domain`，必须与 HTTPS Origin 的域名一致；与 `tls_cert`/`tls_key` 互斥。Server 在 IPv4 TCP 80 提供 HTTP-01 验证，HTTPS 仍使用 `server_bind` 的端口，因此服务端口不能为 80。确保域名指向此服务器、公网 TCP 80 和服务端口可访问，服务器可访问 Let's Encrypt 且系统根证书可用；如配置了 AAAA 记录，必须将 IPv6 80 端口转发到验证监听器。申请失败不会降级 HTTP，可用 `journalctl -u panel-server` 排查，修复后可手动重启服务。ACME 账户和证书保存在服务端数据目录的 `acme` 私有子目录，证书到期前 30 天开始续期并热加载，失败每小时重试；重启复用未过期证书。已有手动证书配置继续有效。

```sh
# 保留身份、数据库、TLS/CA 配置、归属元数据和已有 Xray 运行配置
sudo panel-installer uninstall --component all
# 同时移除受管 Xray 二进制/unit，仍保留其数据
sudo panel-installer uninstall --component all --remove-xray
# 永久删除所选 Panel 数据；还选 remove-xray 才删除其受管配置/数据
sudo panel-installer uninstall --component all --purge --remove-xray --confirm 'PURGE PANEL DATA'
```

卸载 Agent 前尽力通过认证 WSS 自撤销本节点凭据；离线时明确提示在面板撤销，不伪造成功。普通卸载保留本地身份；重新安装应在面板为同一节点签发新 enrollment token。Installer 自身升级/删除由私有临时副本的外部维护进程完成。purge 拒绝未知数据/缓存文件，仅移除所选归属数据与账号；空的共享父目录可以保留。

专用空白 Linux 验收机可运行 `sudo python3 scripts/verify-install.py --bundle ROOT_OWNED_BUNDLE --sha256 TRUSTED_METADATA_HASH`，脚本先拒绝已有 Panel/账号，最终用准确确认清理自己的安装；`--wizard` 驱动真实 TTY，`--failure-bundle/--failure-sha256` 注入明确标记的测试失败发布包验证回滚。不要在已有生产安装上运行该测试。

`bootstrap.sh` 必须先下载到文件审阅再执行，保留 TTY stdin；固定 GitHub 版本、HTTPS、外部可信 archive SHA-256 和四个普通文件白名单。实际 GitHub 发布/签名和下载后安装链路在 Phase 8 验收；当前不宣称不存在的发布资产已经可下载。
