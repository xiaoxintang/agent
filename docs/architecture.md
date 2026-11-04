# 架构说明

架构规范及阶段边界以根目录 `ARCHITECTURE_PLAN.md` 为准。本文记录当前实现与后续设计的边界。

## 当前实现：Phase 8 实施中

```text
Browser -> HTTP(loopback)/HTTPS -> panel-server (Axum + Tokio)
                              |-- GET /api/v1/health
                              |-- auth / nodes / audit -> SQLite
                              |-- / (embedded React + Ant Design)
                              `-- /assets/* (embedded Vite output)

panel-agent -- authenticated outbound WSS --> panel-server
    |                                              |
    `-- private identity + task receipt SQLite      `-- enrollment / credentials / tasks SQLite
```

单个 Cargo workspace，Rust 2024，四个 crate：

| crate | 当前职责 |
| --- | --- |
| panel-common | API/Agent 协议、私有文件边界、统一错误结构、版本和 ts-rs 生成 |
| panel-server | HTTP/用户证书 HTTPS、SQLite、认证/CSRF、enrollment/WSS、节点/任务/审计，内嵌前端 |
| panel-agent | enroll/run、WSS、私有身份/去重、Linux 指标、固定 systemd 状态/InvocationID、StatsService 采样与受控 helper 客户端；拒绝 root |
| panel-installer | Xray 验证下载/归属/生命周期/原子配置与短生命周期 root socket helper；TTY/非交互安装、升级、修复、卸载、输入式 purge 和事务恢复 |

前端使用 React 19、TypeScript 6、Vite 8、Ant Design 6 和 TanStack Query。
健康查询支持错误展示、取消和手动刷新；登录后同一首页展示节点管理和最近审计，未引入全局客户端状态或路由依赖。
出现多页面需求时按计划接入 React Router。未提前创建空页面或未实现的业务模块。

前端工具链使用 Oxc：Oxfmt 0.75.0 格式化，Oxlint 1.90.0 执行 correctness、TypeScript 和 React 检查，Vite 8 与 React 插件使用 Oxc 进行 TS/JSX 转换及 JS 压缩。保留 `tsc` 进行完整类型检查。已移除 ESLint、Prettier 及其专用插件；格式配置保持原有 80 列，忽略生成的契约、锁文件及构建产物。

前端使用 pnpm 12.16.1，由 `web/package.json` 的 `packageManager` 固定版本；`web/pnpm-lock.yaml` 是唯一前端锁文件。沿用 `web/` 单包结构，不额外引入 pnpm workspace。
构建顺序固定为 `pnpm install --frozen-lockfile -> pnpm run build -> cargo build`。Vite 生成 `web/dist`，复制脚本清理旧资源后写入 `crates/server/web-dist`，rust-embed 再编入二进制。
两处产物均忽略提交；锁文件必须提交。Rust 构建脚本对缺少 `index.html` 明确报错，不提供假页面。
启用 rust-embed 的 `debug-embed`，使 debug/release 都使用编译时资源，运行时不依赖源码目录。前端变化需要重新编译服务端。

只有 `/` 是当前已实现的 UI 路由。`/assets/*` 精确匹配嵌入文件；未找到时 404。未来增加 SPA 页面时必须明确登记可回退路径。
`/api/*`、`/agent/*`、未知 UI 路径及私有文件路径均不会返回首页。
HTML 使用 `no-cache`；带 Vite 内容散列的静态文件使用一年 immutable 缓存；健康接口和错误使用 `no-store`。

ts-rs 从 Rust 生成健康、认证、节点、指标、enrollment、任务和审计类型，CI 比较生成结果与提交文件。健康响应保留运行时形状校验。认证状态保存在 TanStack Query 内存，密码和 enrollment token 不进入 Mutation Cache、localStorage 或 sessionStorage；登出/会话失效后清理节点、指标、任务与审计缓存。

SQLite/SQLx 采用 bundled SQLite（不启用动态扩展加载）、WAL、外键、5 秒 busy timeout 和最多 4 个连接，MVP 只支持单个 Server 写入进程。五次迁移编译进二进制，不要求运行目录包含 SQL 文件。还包括 inbounds、全局 clients、client_assignments、config_revisions、流量基线/累计/小时聚合。私有期望配置只在私有 DB 与 WSS/helper 通道流转，公开 REST 和 TS 视图不含私钥。
首次管理员由本地 `panel-server init-admin` 创建，数据库唯一约束保证单管理员，后续 Installer 可复用此入口。密码采用 Argon2id；随机会话仅存 SHA-256 哈希，Cookie 持有原文。CSRF token 从会话原文通过带域前缀的 SHA-256 派生，状态修改同时校验 Origin 和 CSRF。
节点记录与审计在同一 SQLite 事务内写入；删除是撤销，保留审计关联。当前最多 100 个未撤销节点，审计查询最多返回 100 条。审计保留 90 天，在启动/审计写入时物理清理，读取也过滤超期记录。clients 已按全局身份建模，配置修订与流量基线/聚合在对应迁移中实现。

enrollment 只存 token 哈希，10 分钟、单次消费；消费及节点凭据轮换原子提交，浏览器只得到一次性 enrollment token，长期凭据仅经 WSS 发给 Agent 并存其私有身份库。凭据有效 365 天；撤销会终止连接、使 enrollment 和未完成任务失效。
WSS 使用 rustls + ring，支持手动证书或通过 ACME HTTP-01 自动申请、续期及热加载 Let's Encrypt 证书；Agent 使用受信任根/显式私有 CA 验证主机名和有效期。没有 TLS 跳过验证或外部代理。
Agent 协议 v2 有 64 KiB 帧/消息上限、UUID/UTC 时间/连接身份检查，15 秒心跳、45 秒超时。Server/Agent 需同步升级，不接受 v1 连接；既有 AgentStatus 任务和 receipt 形状保持可读。连接 generation 防止旧连接修改新连接状态，启动复位遗留在线记录；读取状态也把超过心跳窗口的 online 记录视为 offline。
Server 使用 SQLite 作为任务队列，先持久化再发送，单连接串行分发，每节点最多 100 个有效未完成任务，无独立队列服务。重连只恢复未完成且未过期任务。Agent 的私有 SQLite receipt 在执行前提交、结果发送前持久化，重复 ID 返回缓存、不同 payload 拒绝；不确定的崩溃中任务标记 interrupted，不自动重做。开放 AgentStatus 和固定白名单 ServiceStatus，返回结果校验原操作/服务；服务结果写入与审计同事务，幂等键复用到不同操作时拒绝并审计。
Agent 单目录用文件锁限制一个进程；超期 receipt 一天后清理，Server 终结任务 7 天后清理。期望配置 revision 的重连协调留到 Phase 5，当前不创建假配置状态。

Linux Agent 用固定 `/proc` 路径和 rustix statvfs 每 5 秒采样 CPU/内存/根文件系统/网络，不扫描进程或挂载，不建立无限缓冲。CPU/网络仅保留一个前样本，首轮/回退/接口变更输出 null；网络最多 128 接口，包含虚拟接口且排除 lo。非 Linux 不模拟指标，显式上报 unavailable。Server 只 upsert 每节点一条快照，经身份、generation、时间及数值校验后返回鉴权 API；15 秒未更新或离线即标历史数据。字节计数为 u64 十进制字符串，避免浏览器精度损失。
systemd 使用 zbus Tokio/Proxy 访问系统总线，只调用固定 unit 的 GetUnit 和三个只读状态属性，没有服务变更/加载接口；未加载不等于未安装。每次查询 3 秒上限，失败原因有类型。无需特权 helper，因此本阶段没有新增提权通道。
Agent main 在创建运行时前拒绝 root，并在 Linux 设置 NoNewPrivileges 使后续线程继承。非 root systemd 模板清空 capability 并限制文件系统/地址族；不自动安装模板。后续特权操作由另行设计的短生命周期、固定操作 helper 承担。

CI 只使用 GitHub Actions 的 Ubuntu runner、Node 和 Rust。Actions 固定完整提交 SHA，最小权限为 `contents: read`。没有新增托管服务或自动发布流程。

## 后续架构约束

服务端管理期望配置，Agent 管理观测状态；Agent 通过出站认证 WSS 连接，断线不影响已运行的 Xray。
不为遥测引入外部数据库。
长期 Agent 以非 root 用户运行；特权操作限于短生命周期的固定操作 helper/installer。
协议使用版本化、有界消息和操作枚举，不接受任意命令。

ACME、systemd 变更、Xray、安装与发布硬化按后续 Phase 实施；健康状态仅表示 HTTP 进程可响应，不探测数据库持续可用性或尚未实现的子系统。
