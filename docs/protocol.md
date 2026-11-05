# HTTP 契约

## Phase 4 Xray 生命周期

`POST /api/v1/nodes/{id}/xray/{action}` 沿用管理员 Cookie/CSRF 和任务审计，action 仅为 install/upgrade/start/stop/restart/rollback/uninstall/status。请求为 `{idempotency_key:UUID,version?:XrayVersion}`，install/upgrade 必须提供版本，其余动作禁止版本参数；也可通过 TaskInput 的 `{"xray":operation}` 提交相同封闭枚举。新任务 201、同键同操作重试 200、冲突 409。生命周期任务截止时间为 300 秒，其他任务仍为 60 秒。
仅信任固定的 `26.10.21`、`26.10.31`，Linux x86_64/aarch64；catalog 的 ZIP SHA-256 来自官方 GitHub Release 的 HTTPS 元数据并固定进代码，不支持 latest、任意 URL、路径或 unit。最多 24 MiB 下载、64 MiB 二进制，只读取 ZIP 中的 xray 内容，不对其他条目执行解包。HTTPS 重定向仅限 GitHub 与其 release-assets 主机，证书错误/摘要错误直接失败。
结果是 `{status:"xray",data:{status:"ok",data:{installed,version,previous_version,active}}}` 或 `{status:"xray",data:{status:"error",data:XrayError}}`；报告必须匹配原操作/所选版本，成功或失败记录 xray.operation 审计。初始配置没有监听端口；uninstall 只移除已验证归属的二进制/current/unit，保留配置与数据，不提供 purge。
Agent 通过固定 `/run/panel/xray-helper.sock` 请求短生命周期 helper，验证服务端 peer UID=0；helper 只接受 root-owned `/etc/panel/xray-helper.json` 指定的非 root peer UID。请求包含 UUID task_id、截止时间和封闭 operation，无任何路径/命令/服务名参数，最多 4 KiB。helper 自身也保留 receipt，重复请求只返回缓存，不确定操作返回 interrupted。
生产路径固定 `/opt/panel/xray`、`/var/lib/panel/xray-helper`、`/etc/systemd/system/xray.service`；拒绝符号链接、非归属目录、hash 变化、外来 unit/drop-in 或已有非受管安装，不覆盖用户文件。版本经 xray version 与 run -test 验证后原子切换 current。升级健康失败恢复旧版本；切换检查点中断后，下一次 status/rollback 恢复旧版本；其他新操作先返回 interrupted，要求先确认恢复状态。
wire 升至 v3，Server/Agent/helper 应同步部署。Agent 在长任务期间继续心跳/采样；Server 重发固定 in-flight task ID，不因新队首改变执行对象。已经交给 helper 的操作可能在控制连接中断后完成，撤销节点不强杀正在执行的本机事务；重新接入后查询实际状态，不重做不确定的原任务。

## Phase 3 指标与只读 systemd

`GET /api/v1/nodes/{id}/metrics` 要求管理员会话，沿用 JSON 错误、请求 ID 和 no-store。返回 `{node_id,snapshot,received_at,stale}`；尚无采样时 snapshot/received_at 为 null 且 stale 为 true。节点不存在/已撤销为 404；撤销会删除最新快照。只保留每节点一条快照，不建立逐次采样历史表。
Agent 每 5 秒经 WSS 发 `kind:"metrics"`，payload 为 MetricsSnapshot；非 Linux 发送明确的 unsupported_platform 和 null 值。Server 校验绑定身份、当前连接 generation、字段边界及 5 分钟时间窗口。节点离线、最近心跳超过 45 秒、指标接收超过 15 秒或来自旧连接时 stale 为 true，客户端不能把历史值当作实时状态。

| MetricsSnapshot 字段 | 类型/语义 |
| --- | --- |
| sampled_at | UTC Unix 秒 |
| cpu_percent | number 或 null；聚合 CPU tick 差分的占用率，0–100；首次采样、计数回退或零间隔为 null |
| memory | ByteUsage 或 null；MemTotal 与 MemAvailable，used=total−available |
| disk | ByteUsage 或 null；根文件系统 `/` 的容量，不扫描所有挂载；used=blocks−free，available 不包含保留块 |
| network | NetworkUsage 或 null；非 loopback 接口汇总，包含虚拟接口，不能用作 Xray 计费 |
| issues | 最多 4 项不重复指标的 `{metric,reason}`；读取失败、数据异常、超过上限和平台不支持均显式标记 |

ByteUsage 是 `{total_bytes,used_bytes,available_bytes}`，字节计数用十进制 u64 **字符串**，避免 JavaScript 精度损失。NetworkUsage 为 `{received_bytes,transmitted_bytes,received_bytes_per_second,transmitted_bytes_per_second}`；前两项字符串，速率为有限非负 number 或 null。首次采样、计数回退、接口集合变化时速率为 null，后续重新建立基线；不伪造 0 或巨大负向/回绕增量。
采集只读取固定路径：`/proc/stat` 首行最多 1 KiB，`/proc/meminfo` 和 `/proc/net/dev` 各最多 64 KiB，网络最多 128 个非 loopback 接口。CPU 不重复累计 guest ticks；网络保留一个前次样本，没有无界缓冲或进程/文件扫描。

新增任务 operation 形状：`{"service_status":{"service":"panel_agent"}}` 或 `{"service_status":{"service":"xray"}}`，其余请求字段沿用 TaskInput。它们只分别映射 `panel-agent.service`、`xray.service`；拒绝额外 method 参数、任意 unit、shell、服务重启/安装任务，Server 和 Agent 共享同一严格枚举。
成功结果为 `{status:"service_status",data:{service,load_state,active_state,sub_state}}`。通过 system bus 调用固定 `GetUnit` 和只读 `LoadState`/`ActiveState`/`SubState` 属性；不调用 LoadUnit/StartUnit/StopUnit/RestartUnit。未加载返回 `load_state:"not_loaded"`、两个状态 null，**不能据此断言未安装**。查询最长 3 秒，失败为 `{status:"unavailable",data:{reason}}`；reason 为 unsupported_platform/systemd_unavailable/access_denied/timeout/privilege_denied，任务状态 failed。
成功/失败服务查询均写 `service.status` 审计；非法任务输入写 `task.reject` denied，只记节点/任务 ID 和 request_id，不保存未信任的 unit/method 原文。结果必须匹配原任务类型和受管服务，不能用另一个服务或 AgentInfo 冒充。相同幂等键对应不同操作时 409 `idempotency_conflict`，同时记录拒绝审计。

wire 已升级到 **v2**，必须一起升级 Server 和 Agent；v1 连接被拒绝。已有 agent_status 的 JSON/结果及 SQLite receipt 形状不变，数据库迁移保留既有身份和任务。

## Phase 2 enrollment、连接与任务

| 方法与路径 | 认证与请求 | 响应 |
| --- | --- | --- |
| POST `/api/v1/nodes/enrollments` | 管理员 Cookie、CSRF；`{node_id}` | `{node_id,token,expires_at}`；随机 token 仅本次返回，10 分钟、单次使用 |
| GET `/api/v1/nodes/{id}/status` | 管理员 Cookie | Node，包括 online/offline、版本、OS/架构、UTC 最后心跳 |
| POST `/api/v1/nodes/{id}/tasks` | 管理员 Cookie、CSRF；`{operation,idempotency_key:UUID}`；operation 见本页 | 新任务 201，同节点/操作/幂等键重试 200；键冲突 409 |
| GET `/api/v1/tasks/{id}` | 管理员 Cookie | TaskRecord；超期未终结任务转 expired |
| GET `/agent/v1/connect` | WebSocket 升级；`Authorization: Enrollment TOKEN` 或 `Bearer CREDENTIAL` | WSS 双向连接；拒绝所有查询参数和无效凭据 |

浏览器会话不能用于 Agent 连接，节点凭据不能用于管理 API。没有读取节点凭据的浏览器接口。新 enrollment 会使此节点旧的未使用 token 失效，但直到新 token 被消费才替换旧节点凭据。消费和凭据签发在单个 SQLite 写事务中完成，长期凭据有效 365 天，可通过重新 enrollment 轮换；撤销节点同时撤销凭据、使 enrollment 失效、取消未完成任务并关闭连接。
若签发后连接中断导致 Agent 未保存凭据，token 仍已消费；需要管理员签发新 token，不允许复用旧 token 来取回凭据。

WSS 使用正常的证书链、主机名和有效期验证；私有 CA 用 `--ca-cert` 显式添加信任，不提供跳过验证开关。开发 WS 仅在 Server 的 `PANEL_DEV_HTTP=1` 和 Agent 的 `--allow-local-ws` 同时明确开启时使用，且 Agent 只接受字面 loopback IP。TLS 监听不允许使用开发 Cookie 模式。

### Wire v3

消息为 JSON，最多 64 KiB；WebSocket 帧和消息均限制大小。共同字段如下：

```json
{"version":3,"message_id":"UUID","node_id":"UUID","sent_at":"RFC3339 UTC","kind":"heartbeat"}
```

`message_id`、`node_id` 必须为 UUID；当前版本必须为 3；时间必须为 UTC 且与本机相差不超过 5 分钟。连接身份来自 header 认证，所有入站消息的 node_id 必须匹配，不能仅凭消息字段选择节点。kind 为下面的严格枚举，携带数据时放在 payload：

| 方向 | kind | payload |
| --- | --- | --- |
| Server → Agent，仅 enrollment | `enrolled` | `{credential}`，一次发送，Agent 先写私有身份库 |
| Agent → Server | `hello` | `{version,os,arch}`，真实进程信息，字段非空、最多 64 字节 |
| Server → Agent | `welcome` | 无，确认 Hello 和身份绑定 |
| Agent → Server | `heartbeat` | 无，每 15 秒 |
| Agent → Server | `metrics` | MetricsSnapshot，每 5 秒 |
| Server → Agent | `task` | `{id,operation,deadline_at}`，仅固定白名单操作 |
| Agent → Server | `task_ack` | `{task_id}` |
| Agent → Server | `task_result` | `{task_id,result}` |
| Server → Agent | `result_ack` | `{task_id}` |

双方处理 ping/pong；Server 每 15 秒 ping，45 秒无应用心跳则关闭并标离线。Agent 45 秒收不到服务端消息则重连。连接/握手最长 10 秒，写入最长 5 秒。断开后以 1、2、4…60 秒为上限的指数延迟加 0–510 ms 抖动重连；认证失败停止，协议错误停止，网络错误重试。Server 重启将旧在线记录复位，Agent 重新验证身份和握手。
同一节点同时只接受最新连接的消息，旧连接被关闭；旧连接退出不能把新连接改为离线。连接数上限 128，每节点只串行分发一个任务，未终结有效任务上限 100。发送前任务和尝试次数先落盘，10 秒未收到结果可重发同一任务，重连只恢复未终结且未过期任务。

### 任务语义

operation 包含 `agent_status`（真实版本/OS/架构）和上述 service_status。任务截止时间固定为创建后 60 秒；Agent 拒绝超过本机 5 分钟后的任务截止时间，过期任务不执行。指标通过独立消息周期上报，不通过任务队列积压。
TaskRecord 包含 id、node_id、operation、status、attempt_count、created_at、deadline_at、finished_at、result。状态为 pending/dispatched/acked/succeeded/failed/expired/canceled；时间是 UTC Unix 秒。AgentStatus 结果仍为 `{status:"succeeded",data:{version,os,arch}}`；服务查询结果见上文；所有操作还可返回 `{status:"expired"}` 或 `{status:"interrupted"}`。
Agent 在本地 receipt 提交后执行，结果提交后按顺序发送 ACK 和结果。相同任务 ID 与相同 payload 返回缓存结果，内容冲突拒绝；断线、ACK 丢失或重启不能让已记录任务再次执行。若进程在 receipt 和结果之间崩溃，任务标记 interrupted，**不自动重试执行**。此处是 at-most-once，不是跨进程 exactly-once；对中断任务由管理员另建新任务。结果确认不会删除 receipt；超过截止时间一天后才清理，届时重复过期任务仍不能执行。
Server 在启动/创建任务时清理 7 天前终结任务，Agent 在启动/完成任务时清理旧 receipt，不增加队列服务。期望配置 revision 在 Phase 5 实施，当前不伪造配置状态。

## Phase 1 认证、节点和审计

路径统一为 `/api/v1`。除 health/login 外全部要求会话 Cookie；所有 API 响应 `Cache-Control: no-store`，响应均有服务器生成的 `X-Request-ID`。错误保持 `{ "error": { "code": "...", "message": "..." } }`。

| 方法与路径 | 请求 | 成功响应 |
| --- | --- | --- |
| POST `/auth/login` | JSON `{username,password}` | 200 `{admin:{id,username},csrf_token,expires_at}`，设置 HttpOnly、SameSite=Strict、默认 Secure Cookie |
| GET `/auth/me` | Cookie | 200，与 login 相同，不返回 session token 或密码哈希 |
| POST `/auth/logout` | Cookie、`X-CSRF-Token` | 204，撤销当前会话并清除 Cookie |
| GET `/nodes` | Cookie | 200，最多 100 个未撤销节点；创建达到 100 个时返回 409 |
| POST `/nodes` | JSON `{name}`，Cookie/CSRF | 201，Node |
| GET `/nodes/{id}` | UUID，Cookie | 200，Node；不存在或已撤销 404 |
| PATCH `/nodes/{id}` | UUID、JSON `{name}`，Cookie/CSRF | 200，更新后的 Node |
| DELETE `/nodes/{id}` | UUID，Cookie/CSRF | 204，保留记录并标记撤销，重复删除 404 |
| GET `/audit` | Cookie | 200，最近 100 条审计，按创建时间/序号倒序，无凭据字段 |

Node 包含 UUID id、name、status、agent_version/os/arch、last_seen_at、created_at、revoked_at；新节点初始 offline，连接后由已认证 Agent 更新观测字段。时间为 UTC Unix 秒；审计序号输出字符串。
名称去除首尾空白后为 1–64 个 Unicode 字符且不含控制字符。管理员名为 3–64 个 ASCII 字母、数字、点、下划线或短横线。初始化密码为 12–128 字节且不含控制字符，登录输入最多 128 字节；不在 API/日志/审计返回输入密码。
单管理员、每会话有效期 3600 秒；登录全局最多每分钟 10 次且同时最多 2 次密码校验。登录不区分不存在、禁用的账号与错误密码，均 401。未登录/过期/撤销为 401；CSRF 错误为 403；验证错误 400；JSON 媒体类型错误 415；请求体超过 16 KiB 为 413；限流 429；数据库故障 500，均采用统一错误结构。

明文 HTTP 仍仅允许 loopback；配置证书后的 HTTPS 可使用显式配置的监听地址和浏览器来源。

## 健康检查

`GET /api/v1/health`，无需认证；成功为 HTTP 200、`Content-Type: application/json`、`Cache-Control: no-store`：

```json
{"status":"ok","version":"0.1.0"}
```

`status` 目前唯一值是 `ok`；`version` 是 workspace 软件版本。此检查只证明 HTTP 服务能响应，不探测数据库持续可用性或 Agent。
`HEAD` 返回相同状态及响应头，不带响应体。

## 错误

未实现或不存在的路径返回 HTTP 404：

```json
{"error":{"code":"not_found","message":"Resource not found"}}
```

已知路径的不支持方法返回 HTTP 405，`code` 为 `method_not_allowed`，`message` 为 `Method not allowed`。错误响应均为 JSON 并禁用缓存。
所有未知 `/api` 或 `/agent` 路径都不能回退到 SPA。

成功响应类型定义于 `crates/common/src/lib.rs`，由 ts-rs 生成 `web/src/types/generated.ts`。通过 `pnpm --dir web run types:check` 检查一致性。
请求 ID 为服务端新生成的 UUID，通过 `X-Request-ID` 返回；忽略客户端自带的 ID。管理员 HTTP 操作的审计 metadata 保存同一 ID，CLI/Agent 自身事件可为 null；不保存请求体或凭据。

Phase 5 协议 v4 添加 ApplyConfig(ref) 和 ConfigApplied(ref)，结果须严格绑定 revision/content_hash。TaskCommand 的可选 private_config 仅用于持久私有任务及 WSS 下发，缺省不序列化，旧任务幂等摘要不变；浏览器 TaskRecord 永远没有该字段。helper 接受闭集 Lifecycle/ApplyConfig，单条请求限制 64 KiB；没有通用 JSON、路径、unit 或 shell 参数。

新增受会话/CSRF 保护的 `/nodes/{id}/inbounds`、`/inbounds/{id}`、`/clients`、`/clients/{id}`、`/inbounds/{id}/clients/{client}`、`/nodes/{id}/config/revisions`、`/nodes/{id}/config/apply`。Rust 生成 TS 中的 InboundInput/View、ClientInput/View/CreatedClient、ApplyInput/RevisionView 是契约。客户端 UUID 仅创建响应返回一次，REALITY 私钥不经读取 API 返回。

Phase 6 协议 v5 添加 TrafficReport（available(sample)/unavailable）。TrafficSample 含 invocation ID、UTC 秒采样时间和最多 256 个稳定客户端内部 ID 的绝对上下行计数；无 UUID 协议凭据。服务端校验时间偏差≤60秒、非负安全整数、ID 唯一及节点归属；同节点/客户端时间不前进不重复记账。`GET /nodes/{id}/traffic` 返回 7 天小时聚合、客户端累计、可用性及过期状态；读取需会话，写入只经绑定身份的 Agent WSS。ClientInput/View 扩展可空 expiry_at/quota_bytes，View 提供字符串 used_bytes 与 policy_blocked。
