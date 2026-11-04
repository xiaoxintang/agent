# 安全边界

## Phase 7 已实施

- HTTP 默认绑定 `127.0.0.1:8080`，无 TLS 时拒绝公网或局域网监听；用户提供证书/私钥后可显式启用 HTTPS/WSS，公开 TLS 监听必须配置准确 HTTPS Origin。Vite 仅绑定本机。
- 健康接口仅返回状态和软件版本，不返回环境变量、文件路径、凭据或节点数据。
- 管理员只能通过本地 `init-admin` 初始化，无默认密码，不能覆盖已有管理员，不开放 HTTP 首次设置。交互密码不回显，自动化只通过显式 stdin 读取。
- 密码使用带随机盐的 Argon2id v19；验证在有界 blocking 任务中执行。不存在/禁用的用户与错误密码使用同一错误，未知用户也执行等成本哈希验证。
- 会话原文只放在 HttpOnly、SameSite=Strict、默认 Secure 的 Cookie，数据库只保存 SHA-256 哈希，有效期 1 小时；登出撤销当前会话，登录旋转已有 Cookie，禁用管理员立即拒绝其会话。
- 状态修改要求准确 Origin 和绑定当前会话的 CSRF token，固定时间比较。登录要求准确 Origin、JSON 媒体类型；不接受跨站 Fetch、重复会话 Cookie、重复 Origin 或 CSRF 头。
- 登录全局每分钟最多 10 次、同时最多 2 次密码验证，请求体最多 16 KiB。所有业务 API 禁用缓存，所有响应生成新的请求 ID，不信任客户端传入 ID。
- 管理 API 都校验有效会话；节点修改与审计在同一事务中提交，审计失败则业务修改回滚。审计不保存密码、Cookie、会话哈希、CSRF token 或请求体。
- 数据目录新建权限为 0700，SQLite 文件为 0600，拒绝组/其他用户可访问及直接符号链接的数据路径；SQLite 使用 bundled 构建，不开启动态扩展加载。
- 前端密码不进入 Mutation Cache，提交结束清空输入；会话/CSRF 仅在内存持有，不使用 localStorage/sessionStorage，登出及过期后清理管理数据缓存。
- enrollment token 只返回一次、10 分钟、单次使用，消费和凭据轮换在同一事务。节点凭据有效 365 天，Server 只存哈希，原文仅经已认证 WSS 发给 Agent；浏览器没有读取长期凭据的接口。
- Agent 私有 SQLite 保存身份与任务 receipt，目录 0700、文件 0600，文件锁禁止同目录并发进程。凭据不得置于 URL、命令行参数、日志或审计中；认证 header 标记 sensitive，错误输出脱敏。
- Agent 验证正常的 TLS 证书链、主机名和有效期，私有 CA 只能显式添加可信证书，不支持 insecure 跳过验证。WS 明文例外需双方明确开启且仅限字面 loopback IP；不能用该开关跳过 WSS 证书验证。TLS 私钥拒绝组/其他用户可访问权限。
- WebSocket 拒绝查询参数与浏览器 Origin；当前协议 v6、UUID、UTC/时钟窗口、连接绑定身份和 64 KiB 帧/消息限制均校验。每节点只有最新连接可更新状态，45 秒无心跳关闭并标离线。升级时 Server/Agent 必须同步。
- Server 总连接上限 128，每节点最多 100 个有效待执行任务，串行分发，有固定截止时间和重试上限时间窗；Agent 持久去重并拒绝相同 ID 不同 payload。崩溃中不确定任务为 interrupted，不会自动重做。
- 任务使用封闭枚举：AgentStatus、固定服务只读状态、受管 Xray 生命周期和权威配置修订。Server/Agent 共同拒绝任意 unit/method、shell 或其他系统服务的变更。去重的副作用计数只存在于明确标记的测试执行器。节点撤销会关闭连接、撤销凭据、使 enrollment 失效并取消未完成任务。
- Linux systemd 查询通过 zbus 的固定 GetUnit 和三个状态属性，无 sudo、polkit 授权或 root helper；Agent 的只读查询没有 LoadUnit/StartUnit/StopUnit/RestartUnit 调用路径；Xray 变更通过独立受限 helper。未加载明确报告 not_loaded，不冒充 active 或未安装。
- 上述只读查询不变；新增 Xray 变更单独走固定 Unix socket 的短生命周期 root helper。kernel peer UID 双向检查、root-owned 授权配置、固定 root/unit、封闭操作/版本、请求和生命周期锁、receipt/截止时间共同约束；Agent 没有 sudo 或通用文件/命令能力。
- 官方产物仅从受限 HTTPS 主机下载，使用代码内固定 SHA-256，大小上限与唯一二进制读取；不声称上游提供额外签名。未通过校验不运行。生产二进制验证进程降权，实际 Xray 使用 DynamicUser/NoNewPrivileges，不以 root 长期运行。
- managed manifest 检查 UID、权限、真实路径、unit/config/binary hash 和 current 目标；不接管符号链接、修改过的文件或外来安装。卸载仅移除归属已验证的文件并保留配置。切换失败/中断恢复旧版本，不根据未知文件猜测状态。
- Agent enroll/run 在创建 Tokio 运行时和网络连接前拒绝 root。Linux 上先设置 NoNewPrivileges，所有后续线程继承；模板使用非 root 账号、空 capability、ProtectSystem/ProtectHome 等约束。没有关闭此限制的开发开关。
- 指标只读固定 `/proc` 和根文件系统，限制读取大小/接口数量，Server 验证数值、身份及当前连接，只存每节点最新快照。字段缺失、平台不支持、计数重置和历史数据均显式展示，指标不当作 Xray 计费或配额依据。
- 合法服务查询完成与审计在同一事务提交，重复结果不重复写审计；非法任务在 Server 拒绝并审计，不把恶意 unit/method 字符串保存到日志。结果必须匹配原任务和服务枚举。
- enrollment 弹窗关闭时立即卸载，避免组件库关闭动画保留敏感 DOM；异步迟到响应会丢弃。token 不进入 Query/Mutation Cache，登出时也清理任务缓存。
- 静态文件通过 rust-embed 的编译时资源表精确查找，不读取请求指定的磁盘路径。只允许首页和 `/assets/*`。
- 未知 API、Agent 路径、私有文件及缺失资源均返回 JSON 404。已知接口不支持的方法返回 JSON 405。
- 所有响应包含 `X-Content-Type-Options: nosniff`、`Referrer-Policy: no-referrer` 和 CSP。
- CSP 默认拒绝其他源，脚本和网络请求仅允许同源，禁止 object、base 修改和 iframe 嵌入。Ant Design 运行时插入样式及 style 属性，因此当前 `style-src` 使用 `'unsafe-inline'`；脚本不允许内联或 eval。后续硬化时评估 nonce 和 style 属性的兼容性。
- 不生成或发布 source map；没有远程 shell、通用文件编辑、任意 systemd 变更或 sudo 入口；只有固定操作的短期 root helper。
- Agent 无参数仍只输出版本，enroll/run 仅写自身私有目录、读取本机指标/服务状态并出站连接；以普通用户运行，不获取 root 权限。Installer 提供真实 install/upgrade/repair/uninstall/status，只有本机 root 可维护归属已验证的固定部署；普通卸载保留数据，purge 需要准确文字确认。
- Cargo/pnpm 锁文件固定依赖解析，CI 使用 `--locked`/`pnpm install --frozen-lockfile`；pnpm 版本由 `packageManager` 固定，GitHub Actions 固定 SHA；常规 CI 仅读仓库，发布 job 单独持有 release 写入和短期 OIDC 权限。

本阶段验证包含迁移幂等、外键/权限、首次初始化、鉴权、会话撤销/过期、CSRF/Origin、限流、并发写入、脱敏、审计原子性及保留策略，并保留原有静态资源与 CLI 安全测试。

## 当前限制

基础用户证书 TLS/WSS 已实现，但没有 ACME、证书续期/热加载；已执行真实 Ubuntu x86_64/ARM64 安装维护验证，正式签名发布与完整发布矩阵仍在 Phase 8 验收。不能通过端口转发公开明文开发服务。`PANEL_DEV_HTTP=1` 使用不带 Secure 的本机开发 Cookie；未设置时默认 Secure，TLS 模式拒绝此开发例外。
任务执行仅承诺持久 receipt 保护下的 at-most-once；进程在执行与保存结果之间崩溃会返回 interrupted，不能声称跨进程 exactly-once。配置修订已实现摘要绑定、私有备份、真实校验与原子应用/失败回滚。helper 不得扩展为通用远程执行入口。
同一台主机的其他本地进程仍能访问 loopback HTTP；其安全边界依赖主机本身。

2026-11-03 改用 Oxlint/Oxfmt 后，原 ESLint 开发工具链及其 `minimatch` 已从锁文件移除。使用 `pnpm --dir web audit` 和 `pnpm --dir web audit --prod` 检查前端依赖，执行结果见 progress.md。审计结果仅反映当日已公布的依赖公告；未声称完成生产安全审计。

## 后续必须遵守

- 生产浏览器和 Agent 必须使用验证证书的 HTTPS/WSS，证书错误直接失败；不引入不安全跳过开关。
- 密码使用 Argon2id；会话使用 Secure、HttpOnly、SameSite Cookie、CSRF 校验、过期和撤销；不设置硬编码默认密码。
- 节点 enrollment 单次、短期；持久凭据可撤销/轮换，服务端保存哈希；不在 URL 或日志泄漏凭据。
- 长期 Agent 不以 root 运行。特权 helper 只能接受固定操作，严格验证参数、真实路径及归属；不接受任意 shell 或用户指定 unit。
- Xray 配置和密钥限制权限、日志脱敏；修改可审计。升级验证签名/可信校验和，配置原子替换并可回滚。
- 不修改或删除不属于本项目的安装文件。purge 必须明确确认，服务端数据删除要求输入式确认。
- 签名私钥不能进入仓库；生产发布需完成 Linux VM、权限边界和签名验证测试。


安装器从外部可信的元数据摘要验证 bundle，再检查每个 ELF 的架构、摘要和真实 --version。配置输入和缓存为 root-owned 私有文件；不把管理员密码/enrollment token 写入 manifest、参数或日志。服务端与 Agent 专用账号均无登录 shell。私有 SQLite、文件与 WAL 在停服务后纳入升级事务；失败/中断恢复经过记录的旧状态。无法在线撤销凭据时明确提示，不声称离线撤销完成。

root helper 保留 systemd 沙箱；部分 seccomp 选项会隐式删除 CAP_SETUID，因此验证不在该进程内切换 UID。PID 1 启动固定前缀 UUID 的临时 DynamicUser 验证 unit，空 capability、NoNewPrivileges、只读系统/私有设备、10 秒和 128 MiB 限额，配置通过一个私有 credential 交付。命令、二进制路径和 unit 属性由受审代码生成，不接受远程任意路径或 unit。stdout 仅传结构化响应，stderr 明确进入 journal，均不记录原始配置或验证器输出。
