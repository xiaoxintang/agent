# 实施进度

## Phase 8 — Harden and release

日期：2026-11-04。状态：开始实施；Phase 7 已独立验收。

初始检查：Phase 0–7 代码已 SSH 签名推送至 xiaoxintang/agent；GitHub CLI 仅临时读取现有 HTTPS credential。工作区没有用户未提交修改，仓库默认分支 main。专用 SSH 标签签名密钥只存仓库外 0600 文件；本次发布使用独立 OIDC 短期身份签署资产。

文件计划：复查私有文件/运行时边界与 systemd 硬化、备份恢复操作及验证、补 Debian/浏览器矩阵、CI/release 签名标签验证与 Linux 双架构打包/签名/发布工作流、发布校验文档与真实安装复验；更新本进度。签名私钥只在仓库外创建/保存，不复用用户 SSH 登录私钥，不引入生产额外服务。成功标准以规范 Phase 8 的真实 HTTPS、无默认凭据/通用执行、可验证签名、两架构实际安装为准；外部发布若受凭据/平台条件阻碍会如实保留未完成项。

实施记录：已禁用三个进程的 core dump，私有文件以 O_NOFOLLOW 打开并核对实际 FD 的类型/UID/权限，Server 通过独占 server.lock 阻止同目录并发进程。真实安全审计发现 rustls-pemfile 已停维护，替换为现有 rustls-pki-types 的 PemObject 解析；cargo audit 当前 0 漏洞/0 警告，pnpm audit 无已知漏洞。Server systemd 模板已独立成文件并由 Installer 渲染使用，避免发布模板与实际部署重复定义。

Debian 13.7/aarch64/systemd 257 原生 HVF VM 已通过完整 TTY/HTTPS/权限/Xray/Stats/故障回滚/升级/修复/卸载/注册/purge，以及冷 SQLite 备份→注入业务变更→恢复→撤销旧会话和节点凭据→重新注册。镜像经 Debian HTTPS SHA512SUMS 核对，实测 SHA-512 为 5d3ecec46fd29605db4084388a1e2eb82fd19a80202898f962840ea73793261de721e505e3877c829ae86c37fdd2f6c205b0b3b1337a89d0d4b98d9081adc0c2；不声称额外 GPG 验证。

浏览器：使用真实 Firefox 和独立 NSS 测试 CA 信任库验证 HTTPS，不使用 ignoreHTTPSErrors。已贯通实际登录、一次性令牌注册、Xray 卸载/安装、入站/客户端创建和关联、配置应用；发现页面嵌套表单共享 DOM id，已为每节点表单命名修正。最后流量读取改用同一浏览器的已验证 TLS fetch，避免 Node API context 不继承 NSS 根；最终真实 Firefox 全链路（HTTPS 登录→令牌注册→Xray 卸载/重装→入站/客户端/关联→配置应用→流量展示）exit 0，证书验证始终开启，NSS 仅信任本次独立测试 CA。验收错误输出曾包含一次性测试 Cookie，测试实例已 purge、会话已失效，已补通用敏感 header/密码/令牌输出脱敏。

发布准备：独立项目标签签名 key 存在维护者仓库外 0700 目录，公钥/allowed-signers 在 deploy；不复用任何节点 SSH key。发布资产将由 GitHub OIDC 短期身份签署 SHA256SUMS Sigstore bundle。Actions 按用户要求使用最新正式版本标签（GitHub release API 核验），actionlint 通过。更新依赖版本后的 CI 正在重跑。已使用既有 Git HTTPS 凭据成功推送 SSH 签名初始提交 f197867，CI https://github.com/xiaoxintang/agent/actions/runs/41763638991 已真实执行，但在 hosted runner 的 /usr/local/bin 可被 runner 写入时正确拒绝不可信祖先。生产 Installer 已补提前拒绝；CI 仅在自身一次性 VM 收紧目录权限，修正后待重跑。Phase 8 发布验收完成；稳定 Release v0.1.0 与验证方式见 docs/release.md。发布为不可变版本；后续产品修复或 action/依赖升级须改变 workspace version、执行新一轮 CI/双架构签名发布并创建新标签，不能重写现有资产。最终本机 `sh scripts/check.sh`、actionlint、cargo audit、pnpm audit 和真实浏览器/Linux 接受测试通过。工作区已清洁，所有实现/Phase 8 复验提交均已推送并 SSH 签名。

最终发布验收：Rust audit 0 vulnerability/0 warning；pnpm dev+prod audit 无已知漏洞；本机 fmt/Clippy/tests、Oxfmt/Oxlint/TS/build/smoke 全通过。Debian 13.7 ARM64 通过来源经认证的公开 Release `bootstrap.sh` 下载真实 GitHub v0.1.0 archive 并安装；健康 HTTP、管理员登录、Agent online、Xray 官方生命周期、VLESS+REALITY revision apply、StatsService 均真实运行成功。该一次性 Debian VM 在后续清理时系统性清除了下载 fixture，所以后端查询验证使用临时密码重置，未保留原首次初始化密码；真实安全对象/表未损坏。最后用固定 /tmp 系统级命令构建的 ARM64 installer 以 typed purge 移除了该专用安装和 Xray，确认原有 services/data 之外无 Panel/Xray 安装文件、数据或凭据残留。

容器/VM test fixtures 中保留 QEMU 原生 HVF Ubuntu ARM64 与 Debian ARM64，仅为可复验资产，不随本次仓库发布。正式 SSH tag 私钥和短期 Git credential 从未进入仓库/资产；标签公钥随 release 发布。没有提交 signing 私钥或声明可复现构建。

遗留边界：v0.1.0 已发布，版本化 Release assets 当前有效；后续改动需提升 workspace 版本、重新验收并签新 tag，不能覆盖现有不可变 release。真实公开 bootstrap 已在专用 Debian 主机验证成功；本地替身分发验证仅作为补充。后续若有源码修复或依赖更新须按新版本发布，不对已签 v0.1.0 资产原位修改。当前工作树仅有下面记录的未跟踪差异时先检查；没有未解决产品功能门槛。

## Phase 7 — Interactive installer

日期：2026-11-04。状态：Phase 7 本机、x86_64/ARM64 Linux 与 TTY/分发测试验收通过；自动进入 Phase 8。

初始检查：仓库仍为未提交源码；现有 Installer 仅有 Xray helper，没有 install/upgrade/repair/uninstall/status。授权 Ubuntu 支持免交互 sudo、systemd 255，磁盘约 46 GiB 可用；未发现 /etc/panel、/var/lib/panel、/opt/panel 或已安装的 panel 二进制/账号。不会改动既有系统服务/用户数据。

文件计划：installer CLI/TTY wizard、严格配置/平台检测、验证本地发布包、归属 manifest/事务备份与恢复、专用账号和 systemd/credential/socket 部署、安装/升级/修复/状态/卸载/显式 purge；Server/Agent 为卸载提供绑定身份的凭据自撤销；systemd 模板、bundle/bootstrap 脚本、隔离测试与真实 Linux 安装验收；文档与本记录。

成功标准：实际安装非 root Server/Agent 和短生命周期 root helper，认证/WSS/Stats/配置串联可用；重复安装拒绝覆盖，已拥有文件才能升级/修复/移除，失败恢复旧文件/数据，普通卸载保留数据，purge 必须准确输入确认并只删除归属范围。安装器自身变更通过外部维护进程完成。先支持管理员明确提供的已验证离线 bundle；网络 bootstrap/签名发布按 Phase 7/8 的依赖继续实现和验证，不把未发布产物说成已发布。

实施记录（验收进行中）：已实现 clap 子命令和 dialoguer TTY 向导、Debian/Ubuntu/systemd/架构/磁盘检测、外部可信 SHA-256 固定离线 bundle、ELF/版本校验、root manifest 与静态文件/SQLite 事务备份恢复、固定专用账号、TLS credential/私有运行目录、非 root Agent、socket helper、维护进程重执行、凭据自撤销及保留身份重新注册。普通卸载保留数据库和 TLS/CA 配置；purge 需准确确认并拒绝未知文件。bootstrap 要求明确的可信 archive SHA-256、固定版本、HTTPS 和四个普通文件白名单；网络发布链路待 Phase 8 的正式产物。

本机完整检查已通过（63 Rust / 23 web），真实 systemd 部署尚在复验，不能标为 Phase 7 完成。首轮暴露 systemd credential 组读取模式与 Server 严格私钥检查冲突，保持严格检查，在 0700 RuntimeDirectory 生成 0600 私钥副本。第二轮确认 x86_64/ARM64 HTTPS、非 root UID/全部线程 NNP/零能力与 socket 权限通过，但 helper stderr 继承 socket；已将其明确路由 journal，避免错误日志混入结构化响应。每次失败均由事务恢复和明确测试数据 purge 清理，无已有用户服务/数据被修改。

开发验收环境：安装本机 QEMU 11.1.3（不注册常驻服务），创建临时原生 HVF ARM64 Ubuntu 24.04.5/systemd 255 VM，只有 SSH key 登录、无默认密码。Canonical HTTPS 镜像 SHA-256 实测为 50e24e1fb79e89116e939d90aa7738f7af48c442fa1bc2510131e0b39c50348e；核对官方 HTTPS SHA256SUMS，不声称已验证额外 GPG 签名。x86_64/aarch64 musl 三个二进制均实际交叉构建成功。因生成的开发产物占空间，使用 Cargo 自带 clean 清理开发缓存，未清除 release 或 Linux 验收包。

最终验收：`sh scripts/check.sh` exit 0，63 Rust / 23 web，格式、Oxc/Clippy、契约/TS、测试、release 构建与独立二进制双 Agent 冒烟通过。真实 Ubuntu x86_64 和原生 HVF ARM64 VM 均通过 install→HTTPS/Agent/UID/全部线程 NNP/零能力→peer 拒绝→官方 Xray root helper 安装→DynamicUser 配置应用→systemd InvocationID/真实 StatsService→升级失败恢复二进制/SQLite 身份和业务数据→正常升级→修复损坏的受管 unit→保留身份及运行中 Xray 的普通卸载→同节点重新注册→输入式 purge。ARM64 额外完整驱动真实 TTY custom 向导执行同一流程。

修正与决策：systemd 多个 seccomp 沙箱选项隐式移除 CAP_SETUID，helper 不放宽沙箱，改由 PID 1 启动限额/自动回收的独立 DynamicUser 临时验证 unit（空 capability、NoNewPrivileges、10 秒、128 MiB）。Go 重新打开 /dev/stdin 对 root 管道缺少权限，改用 LoadCredential 交付单个私有 JSON，非 root 用户级验收仍保留管道。故障注入揭示回滚对未加载 unit 的 reset-failed 不能强制成功，已改为尽力重置、强制检查实际启动；失败包是明确 tests/fixtures 下的真实校验有效/启动返回 42 的测试 ELF，生产代码没有模拟分支。失败 fixture 的真实数据已恢复验证后明确 purge，主机没有原有用户数据被删除。

bootstrap 另在自建 ARM64 VM 的私有 mount namespace 内，通过明确标记的本地 TLS 分发端替代尚未发布的 GitHub 资产：校验证书/主机名、不跳过 TLS、错误 archive 摘要在执行 Installer 前拒绝，正确实际 archive 下载后执行真实 Installer 并验证 HTTPS Server，最后清理。/etc/hosts 仅在该测试进程 mount namespace 绑定替身文件，PID 1/主机原始文件不改动。此测试验证分发/校验/安装逻辑，**不声称实际 GitHub 发布已存在**；真实公开签名发布与从发布安装继续在 Phase 8 完成。CI 已加入相同真实 TTY/安装/故障/权限/bootstrap 测试，尚未声称远程 CI 已运行。

## Phase 6 — Traffic and quotas

日期：2026-11-04。状态：Phase 6 验收通过；自动继续 Phase 7。

初始检查：Phase 5 完整检查与 Ubuntu 真实 Xray 配置应用/健康回滚通过，源码仍未提交；pnpm/Oxc 和四 crate 结构保留。文件计划：common 流量消息/统计客户端和固定上游 proto；配置生成增加仅回环 StatsService；Agent 读取固定 xray.service invocation 并采集计数；Server 第五次迁移、持久基线/小时聚合/配额与到期策略、流量 API；web 客户端配额/到期/用量与图表；真实流量/重启验收、文档与本记录。

成功标准：真实 Xray 产生的可归属流量经 gRPC→Agent→WSS→SQLite 展示为非负增量；重复/乱序采样不重复计费，重启 epoch/计数归零不产生巨大假流量；配额和到期从已提交期望快照移除客户端、生成审计修订且不激活未提交草稿；失联保留最后健康配置并明确最终一致性限制。使用 tonic/prost，无新服务；统计 API 固定回环，不启用无需的 HandlerService。

验收：`sh scripts/check.sh` exit 0，61 项 Rust、23 项前端测试，格式、Oxc/Clippy、TS/契约、release 构建和独立双 Agent 冒烟通过；Linux musl workspace/all-targets Clippy 通过，pnpm 完整审计无已知漏洞。WSS/SQLite 集成验证重复与乱序不重复记账、进程替换归零、配额/到期移除、累计持久化、已提交快照与草稿隔离、审计及 API 字节数脱敏。

真实 Ubuntu 官方 Xray 26.10.31 的独立 server/client 完成真实 VLESS+REALITY 握手、SOCKS→本机临时 echo 的 262,144 字节双向传输，tonic/prost StatsService 读到上下行增量各 262,144，重启计数归零安全；没有替代统计结果的模拟实现。先用小包建立基线，再记受控流量。验收配置仅放行该临时 echo 端口；新版 Xray 默认阻止私网，生产配置未放宽。测试发现 CLI 公钥标签已变为 Password (PublicKey)，按明确标签解析修正；诊断日志只在私有临时文件中，失败输出替换测试凭据，完成后进程/文件删除。既有真实 systemd 生命周期/配置回滚再次通过。

决策与限制：StatsService 固定 127.0.0.1:10085、禁止业务使用该端口，业务代理显式阻止回环目标，不启用 HandlerService。固定官方 proto（含 MPL 许可证与 SHA-256），只实现所需 RPC/字段，无生产 protoc/Go/Node 依赖。7 天小时聚合、独立累计，首次/重启首轮只建基线；不可宣称精确计费或硬配额。线上到期 5 秒轮询、流量 15 秒采样再加应用时间，离线或应用失败会延迟；提高配额后手动应用恢复。图表为内建 SVG，无额外图表包。systemd InvocationID 的系统级部署串联验收留到 Phase 7；此阶段真实流量使用独立进程，进程重启真实验证，归零和 epoch 算法/持久化由实际 RPC 与集成测试共同覆盖。

## Phase 5 — Xray config management

日期：2026-11-03 至 2026-11-04。状态：Phase 5 验收通过；按用户指示自动进入 Phase 6。

文件计划：common VLESS/REALITY 类型、校验/JSON 生成、版本引用与私有任务数据；Server 第四次迁移（inbounds/clients/assignments/config_revisions）、CRUD/关联/修订/应用 API 和审计；Agent 私有配置任务转发；installer helper 的受控配置校验/备份/原子应用/健康回滚；web 入站/客户端/关联/修订界面；真实 Xray 配置验收、文档与进度。

- [x] 真实 VLESS+REALITY 模型，端口/IP/域名/UUID/短 ID/密钥校验，完整 JSON，禁止任意 Xray 配置对象。
- [x] 全局客户端身份与入站关联、敏感字段保护、期望修订持久化和摘要。
- [x] root helper 只接受类型化配置，实际 xray run -test、私有备份、原子替换、重启/liveness、失败回滚。
- [x] 创建入站/客户端并关联、提交修订与结果展示；读取 API 不返回私钥或完整 credential-bearing 配置。
- [x] 真正官方 Xray 校验成功、无效配置不替换、健康失败恢复，以及格式/Lint/类型/构建/测试。

决策：Server 是期望配置来源，Agent/helper 是实际应用结果来源。优先完整配置+验证重启，不做热更新。配置私密数据只进入服务端私有 DB 和加密 Agent/本机 socket 通道，公开任务记录只含 revision/hash，不序列化私有任务数据；现有任务格式保持兼容，wire 新版需同步升级。REALITY 密钥由安全随机源生成，私钥不经读取 API 回显。

验收：`sh scripts/check.sh` exit 0，59 项 Rust 测试、22 项前端测试，以及 Oxfmt/Oxlint、契约/TypeScript、Rust fmt/Clippy、前后端 release 构建和双 Agent 冒烟通过。Linux musl 全 workspace/all-targets Clippy 通过；pnpm 生产审计无已知漏洞。额外集成测试验证快照隐私、UUID 仅创建显示、幂等/关联/修订结果绑定、旧等待修订取消、最新投递超时重连协调，以及最新期望任务不受 7 天清理影响。

实际 Ubuntu 用户级 systemd/官方 Xray 26.10.21 与 26.10.31 验收通过：生成的 VLESS+REALITY 配置真实 run-test、应用并监听；无效密钥不替换；占用端口导致失败后恢复旧配置及 active；重启中取消应用后从私有检查点恢复；继续保留既有生命周期与未受管文件保护验收。测试发现并修复 systemd start-limit 阻止回滚，以及节点撤销/派发竞争将 canceled 任务重新标为 dispatched 的问题。最新期望修订保留其任务引用，长期离线后仍能协调；其他历史受保留策略限制。

限制：UI 由组件测试覆盖创建操作与一次性凭据清理，未把 jsdom 声称为真实浏览器 E2E；服务健康为 systemd active/liveness，未声称完整 REALITY 远端握手验证。生产 root socket/DynamicUser 安装、ARM64 与签名发布仍在 Phase 7/8 验收范围。

## Phase 4 — Xray installation and lifecycle

日期：2026-11-03。状态：Phase 4 验收完成；按用户最新指示自动继续下一阶段。

文件计划：common 固定 Xray 版本目录/操作/结果与协议；installer 的验证下载、版本安装、manifest/归属、systemd 后端、短生命周期 socket helper 与测试；Agent Unix socket 客户端；Server 任务/API/审计；web 生命周期操作；systemd helper/Xray 模板、真实 Linux 验收和文档。

- [x] 固定官方 v26.10.21/v26.10.31、x86_64/aarch64 ZIP SHA-256，受限 HTTPS 下载及校验后提取唯一 xray 二进制。
- [x] 归属 manifest、路径/权限校验、版本目录和 current 原子切换，拒绝接管已有非受管 Xray。
- [x] 安装/升级/回滚/启动/停止/重启/卸载及健康验证，失败恢复旧版本；保留数据而非 purge。
- [x] Agent 仍非 root/NoNewPrivileges，通过固定 Unix socket 短生命周期 helper，peer UID 和 root-owned config 授权，无任意路径/命令/unit。
- [x] 真实 Linux 用户级临时 systemd/Xray 验收、破损下载/归属/权限和重复任务测试，完成本阶段检查。

决策：官方发布通过 HTTPS 获取，其发布 API SHA-256 作为经代码审查固定到仓库的可信摘要，不声称上游提供了额外签名。生产 helper 固定 /opt/panel/xray、/var/lib/panel/xray-helper、xray.service；四 crate 结构不变，helper 位于 panel-installer，通过 Accept=yes socket 激活每连接一次，不是常驻 root Agent。Phase 5 前仅安装无监听端口的最小完整配置。真实验收仅使用用户级临时 unit/目录，不修改主机系统级服务、用户数据或安装软件。

验收：`sh scripts/check.sh` exit 0：Rust 57 项、前端 21 项，格式、Clippy/Oxlint、类型/契约、前后端构建、双 Agent 冒烟均通过。Linux musl `cargo clippy --target x86_64-unknown-linux-musl --workspace --all-targets --locked -- -D warnings` 通过；pnpm 完整与生产审计均 0 已知漏洞。临时 Zig/官方 Rust target 交叉构建真实 `xray_acceptance` example，在用户提供 Ubuntu 普通用户上实际执行，通过官方两版本下载/摘要、xray version/run-test、安装、启动/停止/重启、升级、上一版本回滚、注入健康失败恢复旧版本、在真实 restart 后取消 future 并由检查点恢复、保留配置卸载和未受管 sentinel 保护。仅创建唯一 panel-phase4-* 用户 unit，完成后停止/删除，不接触系统级 xray.service。

实现中发现 Xray 26.10.31 将指向普通文件的 /dev/stdin 交给扩展名 loader；改用有界配置管道及显式 JSON 格式后，两个固定版本真实校验通过，没有放宽摘要或权限。长任务改为单个受控 future，与心跳/采样并行；Server 重发精确 in-flight ID，防止新队首打断升级。Helper receipt 独立持久化，切换写检查点；失败/中断不重做原请求，未知/损坏状态保留文件并拒绝猜测。

新增受保护 `/nodes/{id}/xray/{action}` API、相同操作幂等/不同操作冲突、300 秒截止、结果版本绑定和 xray.operation 审计；UI 仅提供固定版本/动作，停止/重启/升级/回滚/卸载有具体确认，未配置 helper 明确失败。新增 UID 授权、归档损坏/路径、未受管/软链、API/长任务队列和 UI 测试。生产 root socket/DynamicUser 完整部署尚未在该主机执行，将随 Phase 7 安装验收；这里不把用户级生命周期验收说成 root helper 部署通过。ARM64 Xray 执行、root 安装和正式发布签名仍在后续验收范围。

2026-11-03 用户明确要求后续按 Phase 顺序自主完成，不再等逐阶段确认；每阶段仍需先验收再进入下一阶段。已识别 origin 为 xiaoxintang/agent；`gh auth status` 当前未登录，已向用户异步请求在本机登录，发布前继续其余独立工作，不在聊天索取 token。

## Phase 3 — Linux metrics and systemd

日期：2026-11-03（Asia/Shanghai）。状态：Phase 3 本机与真实 Ubuntu 验收通过，已停止；Phase 4 未开始。

文件计划：common 的指标/受管服务类型、协议和 TS 生成器；Agent metrics/systemd/security 模块与连接/任务分发；Server 第三次迁移、最新指标接收/查询、任务类型和拒绝审计；web 的资源面板/服务状态与测试；非 root systemd 模板、验收脚本、README/protocol/security/architecture/operations 和本进度。

- [x] Linux 真实 CPU/内存/根文件系统/网络采样，每 5 秒，首轮/计数重置不伪造百分比或速率。
- [x] 有界指标消息和每节点一条最新快照、鉴权 API、离线/过期标记及资源 UI。
- [x] zbus 只读访问 systemd，仅 panel-agent.service/xray.service 固定枚举，无任意 unit 或变更接口。
- [x] Agent 拒绝以 root 身份启动，非 root 模板、任务白名单/拒绝审计和越权测试。
- [x] 完整格式/Lint/类型/构建/测试，真实 Ubuntu 节点采集与 systemd 拒绝验收。

决策：直接读取有大小限制的 Linux /proc，rustix 获取根文件系统容量；不引入全量系统信息框架。zbus 仅使用 Tokio/Proxy 功能，状态读取无需 sudo 或特权 helper。未知/缺失服务返回真实 not_loaded，不伪造运行状态。WSS wire 升至 v2，任务结果保留已有 AgentInfo 成功格式并新增服务状态结果，避免损坏旧任务/journal。用户已提供 SSH 验收环境；只用普通用户的临时目录、进程和只读 systemd 查询，不安装/修改现有服务。Linux 交叉工具仅临时用于构建与验收，不成为运行依赖。

### 实施与技术决策

- CPU 读取 `/proc/stat` 第一行（最多 1 KiB）做 tick 差分，不重复计 guest；内存读取 MemTotal/MemAvailable；根盘使用 rustix statvfs("/")，区分 free 与保留块后的 available。
- 网络读取 `/proc/net/dev`（最多 64 KiB、128 个非 lo 接口），保留一个前样本和单调时间，首轮/回退/接口变化清除速率基线；网络与字节字段用字符串保留 u64 精度，不作为 Xray 计费指标。
- 采样每 5 秒通过有界 WSS 消息发送；Server 第三次迁移新增 node_metrics，每节点只 upsert 一行。校验身份/generation/时间和数值，鉴权 GET 返回 snapshot/received_at/stale；15 秒未更新、离线或连接变更显示历史数据，撤销时删除快照。
- zbus 6.2.1（只在 Linux 构建，Tokio/Proxy）通过 system bus 调用固定 GetUnit 和只读属性；3 秒超时。没有 systemctl 子进程、任意 method、LoadUnit/RestartUnit 或 sudo。`not_loaded` 仅表示当前未加载，不能据此断言未安装。
- Agent main 在创建运行时前拒绝 root，并设置 Linux NoNewPrivileges；非 root systemd 模板清空 capabilities，使用 StateDirectory、私有权限及文件系统硬化，不自动部署或创建账号。当前没有需要 root 的操作，不为此引入空 helper。
- ServiceStatus 使用严格受管服务枚举，Server 和 Agent 解析器都拒绝任意 unit/额外 method/服务变更。非法任务产生 task.reject denied，结果必须匹配原任务类型和服务；service.status 审计与结果同事务提交。幂等键复用到不同操作时 409 并审计。
- UI 展示在线总数、资源弹窗、过期/不可用标记和固定服务查询按钮；按需查询资源，不为每行默认发起指标请求。登出清理指标缓存。wire v2 需要 Server/Agent 同步升级，既有身份、任务与 receipt JSON 形状保持可读。
- 新增 `scripts/verify-linux.py`（Python 标准库）作为可复查的真实 Linux/systemd 验收，CI 加入同一命令。脚本只运行私有临时 HTTPS/WSS 和普通用户进程，不安装/启停受管服务，结束删除证书/数据及进程。

### 本机与交叉检查

macOS ARM64，Rust/cargo 1.96.0、Node 22.23.2、pnpm 12.16.1。最终 `sh scripts/check.sh` exit 0，逐项结果：

| 命令 | 结果 |
| --- | --- |
| `pnpm --dir web install --frozen-lockfile` | 通过，未增加前端依赖 |
| `pnpm --dir web run format:check`、`pnpm --dir web run lint` | Oxfmt/Oxlint 通过，无警告 |
| `pnpm --dir web run types:check`、`pnpm --dir web run typecheck` | 契约/TypeScript 通过 |
| `pnpm --dir web test` | 6 个文件、19 项通过 |
| `pnpm --dir web run build` | 生产构建及资源复制通过 |
| `cargo fmt --all -- --check` | 通过 |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 通过，无警告 |
| `cargo test --workspace --locked` | 50 项通过：Agent 8、common 4、Server 单元 2、HTTP 4、Phase 1 17、连接/Phase 3 集成 15 |
| `cargo build --workspace --release --locked` | 四个 crate、三个本机二进制构建通过 |
| `pnpm --dir web run smoke` | 两个独立 Agent 进程、资源上报/平台可用性、原有认证/任务/重启链路通过 |
| `cargo clippy --target x86_64-unknown-linux-musl --workspace --all-targets --locked -- -D warnings` | Linux 专用代码与全部测试目标检查通过，无警告 |
| `cargo build --release --locked --target x86_64-unknown-linux-musl -p panel-agent -p panel-server` | 最终 Linux 静态 PIE 产物构建通过 |
| `pnpm --dir web audit`、`pnpm --dir web audit --prod` | exit 0，无已知漏洞 |
| `sh -n scripts/check.sh`、Python ast.parse 验收脚本 | 语法通过 |
| `postman status` | exit 0，本地 CLI 可用，无远程 workspace 依赖 |

新增测试覆盖：CPU delta/guest tick/回退、内存/磁盘保留块、网络首次/接口变化/重置、平台不可用、指标字段边界/精度、root 身份校验、Agent wire 拒绝越权；Server 指标鉴权/最新行/stale/撤销/跨节点/非法值、错误服务结果、任务类型/幂等冲突和拒绝审计；前端首轮/不可用/stale/登录过期及固定服务请求。测试 fixture 均标注为 transport/unit fixture，不用于冒充真实采样。

交叉构建使用 rustup 官方 Rust 1.96.0 的 musl target、临时 Zig 0.15.2（官方下载压缩包 SHA-256 `3cc2bab367e185cdfb27501c4b30b1b0653c28d9f73df8dc91488e66ece5fa6b`，解压前验证）。C 源由 Zig 编译，最终链接用 Rust 自带 ld.lld，避免两套 CRT 重复；官方与 Homebrew Rust 的缓存分别隔离。没有向远端安装编译工具/软件包，没有把临时交叉工具加入项目或运行依赖。

### 真实 Linux/systemd 验收

在用户提供的 SSH 主机，以 `ubuntu` 普通用户（UID 1001）执行；Ubuntu 24.04.5 LTS、x86_64、Linux 6.17、systemd PID 1。只上传两个静态二进制、验收脚本和 unit 模板至 `/tmp/panel-phase3-upload.oEe1gV`。最终二进制 SHA-256 本机/远端一致：

| 二进制 | SHA-256 |
| --- | --- |
| panel-agent | `3a56f48ef1e18f71fa97484ff3d1a982082cfffdbdf398b5548f320c7994e301` |
| panel-server | `0f560d71432daa0e89676a5b9d9bd5825faec2379c22c0735a9ee61c01fc637a` |

实际执行：

```sh
python3 /tmp/panel-phase3-upload.oEe1gV/verify-linux.py --bin-dir /tmp/panel-phase3-upload.oEe1gV --unit-file /tmp/panel-phase3-upload.oEe1gV/panel-agent.service
```

最终 exit 0。脚本使用正常证书验证的真实 WSS、随机管理员/令牌、两个独立 Server/Agent 进程，得到：

- CPU 约 1.30%；内存总量 `1000349696` 字节；根盘总量 `50884108288` 字节。后两项与独立读取 /proc/meminfo、os.statvfs 完全一致；上下行速率有限且非负，快照没有错误项，SQLite 只保留一行。
- 所有 Agent 线程有效 UID=1001、NoNewPrivs=1、CapEff=0；没有 sudo、root 进程或新增服务配置。
- panel-agent.service/xray.service 均实际未加载，返回 not_loaded/null；没有伪造 active/running。unit 模板只对临时副本执行 systemd-analyze verify，通过后未安装。
- 任意 `ssh.service`、路径穿越 unit、额外 RestartUnit method、service_restart、shell 五类请求均 400 拒绝，任务表无此类任务，task.reject 审计均 denied；两次合法服务状态查询有 service.status 审计。

首次远端验收发现 Ubuntu OpenSSL 默认生成 CA 用途的自签证书，rustls 正确拒绝当服务端证书使用；测试证书改为显式 CA:FALSE/serverAuth 后通过，未放宽证书校验。SSH 短暂连接重置后重试成功。脚本 finally 已停止所有测试进程并删除私有证书/数据库；上传目录和本机临时交叉工具也在最终复查后清理。最终 Linux 二进制保留在被忽略的 `target/x86_64-unknown-linux-musl/release/`，未改动主机既有服务。

### 真实 HTTP 契约检查

另启动本机 release Server 的隔离临时数据库，执行：

```sh
postman request http://127.0.0.1:55887/api/v1/nodes/00000000-0000-4000-8000-000000000000/metrics --timeout 5000 --script-post-request 'pm.test("Metrics require authentication", () => pm.response.to.have.status(401)); pm.test("Uniform error code", () => pm.expect(pm.response.json().error.code).to.equal("unauthorized")); pm.test("Private metrics cannot be cached", () => pm.expect(pm.response.headers.get("Cache-Control")).to.equal("no-store")); pm.test("Server request ID", () => pm.expect(pm.response.headers.get("X-Request-ID")).to.match(/^[0-9a-f-]{36}$/));'
```

4 个断言通过，返回预期 401。CLI 1.82.0 对 HTTP 4xx 返回 exit 1，即使断言全部通过；记录该行为，不声称此负向检查 exit 0。本机探测进程及数据已删除。授权指标/systemd 完整链路由上述真实 Ubuntu HTTPS/WSS 验收覆盖。

### 限制与下一阶段

- 只完成 Phase 3。没有安装/启停/重启 systemd/Xray、没有 sudo 或特权 helper；Phase 4 的 Xray 安装与生命周期尚未开始，等待用户指示。
- 真实验收主机的两个受管 unit 未加载；未安装测试服务，因此不声称已覆盖实际 active/running unit 的状态分支。后续真实安装验收继续验证该分支。ARM64 Linux、真实浏览器 E2E、ACME、生产签名发布/全量硬化仍未验证；GitHub CI 已配置但尚未触发。
- 资源只保留最新快照、根文件系统和汇总非 lo 网络；不提供历史曲线、全盘扫描、按客户端计量或配额。非 Linux 明确不可用。
- Vite 入口 1,034.76 kB（gzip 328.55 kB），保留原有 500 kB 提示。macOS Server/Agent 约 11/6.9 MiB，Linux musl 约 13/9.6 MiB；未进行发布包或性能验收。
- 无阻塞 Phase 3 的未完成事项，验收通过后停止。

## Phase 2 — Agent enrollment and reliable connection

日期：2026-11-03（Asia/Shanghai）。状态：Phase 2 本地验收通过，已停止；Phase 3 未开始。

文件计划：common 的协议、enrollment/task 类型与私有文件辅助函数、TS 生成器；Server 的第二次迁移、agents 模块、任务/API、连接状态、TLS 配置与入口；Agent 的 CLI/config/store/connection/dispatcher 及测试；web 的 enrollment/状态任务界面与测试；smoke、README、architecture、security、protocol、operations 和本进度。

- [x] enrollment 单次/10 分钟、凭据哈希/撤销、私有 Agent 身份持久化。
- [x] WSS 证书验证、消息版本/身份/时间/64 KiB 上限，明确的 loopback WS 开发例外。
- [x] 15 秒心跳、45 秒超时、ping/pong、指数退避和抖动、重连与离线标记。
- [x] SQLite 持久任务、ACK/结果/结果确认、固定截止时间、幂等提交与 Agent 持久去重。
- [x] UI enrollment 仅展示一次，在线状态、只读 AgentStatus 任务及结果。
- [x] 两个真实 Agent、强制断线恢复、过期/复用 token、错误身份/证书、任务重放等验收与完整检查。

决策：为 WSS 提前接入用户提供证书的基础 rustls HTTPS，不做 ACME/续期或额外代理。Agent 不接受任意命令，Phase 2 唯一实际任务为只读 AgentStatus，返回真实进程版本/OS/架构。去重 journal 在执行前记录 receipt，结果在发送前持久化；执行中崩溃的任务标记 interrupted，不自动重复执行，因此承诺 at-most-once，而不是跨进程崩溃的 exactly-once。副作用执行次数用明确的测试执行器验证。配置 revision 在 Phase 5 出现前不伪造；重连只恢复未终结且未过期任务。

### 技术决策与实现边界

- Server 新增 0002 迁移：enrollments、node_credentials、tasks 以及连接 generation；保留既有数据和 0001 迁移校验。旧在线记录在启动时复位，旧连接结束不能覆盖新连接；读取节点时也校验最后心跳窗口。
- enrollment 在 BEGIN IMMEDIATE 事务中消费并轮换节点凭据，凭据 365 天有效。浏览器只看到一次性 token；长期凭据仅由 WSS 发给 Agent，Server 只保存哈希。节点撤销同时取消任务、失效 token/凭据并终止连接。
- rustls 选择稳定 0.23.46、ring；Axum Server 0.8.0、tokio-tungstenite 0.30.0；不引入代理、队列服务或 ACME。默认 WSS 正常校验证书，私有 CA 显式添加。HTTP/WS 开发例外要求双方明确启用且限 loopback；TLS 模式拒绝开发 Cookie。
- common 提取原有私有目录/文件校验，Server 与 Agent 共用。Agent 使用 0700/0600 的 SQLite 身份/journal 和文件锁，一个目录只允许一个进程。
- 一个真实操作用 connection 中的穷举 match 分发，receipt 在 store；没有为单一用途创建空 dispatcher/reconcile 模块。配置 revision 在 Phase 5 才实现。
- 每节点最多 100 个有效待执行任务，串行分发，60 秒截止、10 秒重发窗口；Server 全局连接上限 128，消息/帧最多 64 KiB。Agent 结果在 ACK/结果发送前持久化，journal 保存至截止后至少一天；Server 终结任务保留 7 天，均在启动/写入时清理。
- UI 每 5 秒刷新节点，任务进行中每秒轮询；token 不进入 Query/Mutation Cache，关闭/登出/组件卸载时清除。测试发现 Ant Design 关闭动画会暂留敏感 DOM，改为关闭即卸载，迟到响应也不能恢复 token。任务创建响应不直接预填查询缓存，避免组件卸载后迟到响应重填已清理的缓存；已补回归测试。
- 所有 wire 消息验证版本、UUID、绑定节点及 UTC 时间窗口，错误输出脱敏，授权头标记 sensitive；没有 shell 或尚未实现的 Linux/Xray 任务。

### 验收记录

环境：macOS ARM64，Rust/cargo 1.96.0、Node.js 22.23.2、pnpm 12.16.1。最终 `sh scripts/check.sh` exit 0：

| 命令 | 结果 |
| --- | --- |
| `pnpm --dir web install --frozen-lockfile` | 通过，未增加前端依赖 |
| `pnpm --dir web run format:check`、`pnpm --dir web run lint` | Oxfmt/Oxlint 通过，无警告 |
| `pnpm --dir web run types:check`、`pnpm --dir web run typecheck` | Rust/TS 契约与类型检查通过 |
| `pnpm --dir web test` | 5 个测试文件，15 项通过 |
| `pnpm --dir web run build` | 生产构建及内嵌资源复制成功 |
| `cargo fmt --all -- --check` | 通过 |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 通过，无警告 |
| `cargo test --workspace --locked` | 37 项通过：Agent 3、common 2、server 单元 2、HTTP 4、Phase 1 集成 17、Phase 2 集成 9 |
| `cargo build --workspace --release --locked` | 四个 crate、三个 release 二进制构建成功 |
| `pnpm --dir web run smoke` | 独立二进制与两个 Agent 进程验收通过 |
| `pnpm --dir web audit`、`pnpm --dir web audit --prod` | exit 0，无已知漏洞 |
| `sh -n scripts/check.sh` | shell 语法通过 |
| `postman status` | exit 0，CLI 可用，无远程 workspace 依赖 |

Phase 2 集成测试使用实际 TLS socket、独立身份/SQLite 和真实 Agent 连接循环，覆盖两个 Agent、强制断线后的退避重连、真实 15 秒心跳、静默 45 秒离线、幂等任务、ACK/结果/重复结果确认、跨节点伪造任务结果拒绝、enrollment 并发单次消费/过期/复用、凭据轮换/过期/撤销、新旧连接竞争、错误 node_id/版本、超大帧/二进制帧、不可信根和错误证书主机名、任务过期及未知 shell 操作拒绝。整组真实计时测试约 46 秒。

Agent journal 测试在真实 SQLite 上用明确标记的测试专用 AtomicUsize 副作用计数器验证：同任务重复、进程 store 关闭/重新打开后重放，计数均保持 1；过期、执行中断、内容冲突不调用执行器。生产仅返回真实 AgentInfo，没有测试执行器或虚构副作用。

release 冒烟把三个二进制复制到独立临时目录，清空 PATH、隔离环境/数据目录，以随机管理员密码和两个不同 enrollment token 启动 **两个独立 Agent 二进制进程**。本机 WS 模式双方均显式启用开发例外；两个节点都完成真实状态任务，随后 SIGKILL 一个 Agent、验证离线、同目录重启后重新在线，再撤销另一节点。无需 Node、源码、迁移或前端资源文件；进程/目录自动清理。WSS 的证书与协议行为由上述真实 TLS 集成测试覆盖。

另用 LibreSSL 3.3.6 在专用临时目录生成仅测试使用、含 127.0.0.1/localhost SAN 的一天期证书，私钥为 0600，启动实际 release Server 的 HTTPS 配置入口。Postman 通过显式信任测试证书校验健康接口：

```sh
postman request https://127.0.0.1:60002/api/v1/health --ssl-extra-ca-certs /tmp/panel-phase2-tls.ruvZ0P/cert.pem --timeout 5000 --script-post-request 'pm.test("Verified HTTPS health", () => pm.response.to.have.status(200)); pm.test("Health contract", () => pm.expect(pm.response.json()).to.eql({status:"ok",version:"0.1.0"})); pm.test("No store", () => pm.expect(pm.response.headers.get("Cache-Control")).to.equal("no-store")); pm.test("Request ID", () => pm.expect(pm.response.headers.get("X-Request-ID")).to.match(/^[0-9a-f-]{36}$/));'
```

exit 0，4 个断言通过。实际 Agent 二进制对同一 WSS 服务分别验证：不信任证书时在 TLS 阶段失败；添加 `--ca-cert` 后到达认证阶段并拒绝测试用无效 token，错误不回显 token。未使用 insecure 选项。临时 HTTPS 服务、证书/私钥、身份库和数据库已停止并删除。

### 限制与下一阶段

- 只完成 Phase 2，尚无 Linux 指标、systemd、Xray、特权 helper、ACME 或生产安装。下一阶段是 Phase 3，等待用户指示。
- at-most-once 的崩溃中任务会报告 interrupted，不声称 exactly-once。对尚无配置 revision 的阶段，不伪造 desired/applied 配置协调；相应内容留到 Phase 5。
- 基础证书 TLS 仍需用户维护证书和私钥，尚未验证续期/热加载、签名发布、Linux VM 或真实浏览器 E2E；Ubuntu CI 已配置但未触发。
- Vite 入口 1,023.17 kB，gzip 325.09 kB，保留原有 500 kB 提示；本机 release Server 约 11 MiB、Agent 6.8 MiB、Installer 828 KiB，不能推断 Linux 发布大小。
- 无阻塞 Phase 2 验收的未完成事项。未自动进入 Phase 3。

## Phase 1 — Server auth and SQLite

日期：2026-11-03（Asia/Shanghai）。状态：Phase 1 本地验收通过，已停止；Phase 2 未开始。

计划修改：Cargo 依赖与锁文件；common 的认证/节点/审计类型及生成器；server 的配置、入口、app、error、db、auth、api、迁移与集成测试；web 的认证/节点/审计界面、请求与测试、smoke；README、architecture、security、protocol 与本进度文档。

- [x] SQLite WAL/外键/5 秒 busy timeout/小连接池，私有数据目录，嵌入迁移。
- [x] `panel-server init-admin --username NAME`，隐藏输入或显式 stdin 密码，只允许首次初始化。
- [x] Argon2id、1 小时会话、仅存 token 哈希、撤销/过期、限流、CSRF/Origin 和请求 ID。
- [x] 受保护的节点 CRUD 和审计查询；撤销代替物理删除；修改和审计同事务。
- [x] 登录/登出、节点管理、审计界面和 Rust 生成的 TS 契约。
- [x] 迁移幂等、未授权/跨站拒绝、凭据脱敏、原子审计等测试，完整检查脚本和独立二进制冒烟通过。

决策：仅创建本阶段使用的 admins、sessions、nodes、audit_logs；未来客户端按全局身份模型设计，业务表在对应阶段创建。初始化暂由本地 Server CLI 提供，未来 Installer 调用此入口，不开放 HTTP 首次设置。HTTP 仍限制 loopback，默认 Secure Cookie，只有显式 `PANEL_DEV_HTTP=1` 才允许不带 Secure 的本机开发 Cookie。`PANEL_ORIGIN` 固定浏览器来源，状态修改需准确 Origin 和会话 CSRF token；登录用 Origin + JSON 防护。审计保留 90 天，每次审计写入及启动时清理；节点不做连接，新增状态为 offline。

### 技术决策与修正

- SQLx 0.9.0 使用 bundled SQLite，禁用默认数据库驱动与 SQLite 动态扩展功能；最多 4 个连接。检查运行时依赖树仅启用 SQLite，没有 PostgreSQL/MySQL 服务依赖。
- 单管理员由数据库 singleton 唯一约束保证；会话使用 CSPRNG UUID 组合生成原文，仅保存 SHA-256 哈希。CSRF 使用带域前缀的会话原文哈希派生，固定时间比较。
- 登录全局每分钟 10 次，最多同时执行 2 次 Argon2 校验，使用 blocking 线程避免阻塞异步运行时；未知账号也做等成本验证。会话 1 小时，成功登录旋转已有会话；登录写事务使用 BEGIN IMMEDIATE，避免并发读取后升级写锁的 SQLite snapshot 冲突。
- MVP 限制 100 个未撤销节点；创建使用单条 INSERT/COUNT 条件原子限制，并发创建也不能突破上限。审计查询只返回最近 100 条，保留 90 天。此阶段不提供审计分页或密码/其他会话管理。
- 前端登录密码不进入 TanStack Mutation Cache，失败后清空输入。测试发现登出时旧组件可能重新填充缓存，现改为管理界面卸载后清理，已通过回归测试。
- 本机 Rust 1.96/macOS release 构建曾出现 SQLx 过程宏 dylib 的 LINKEDIT 加载错误。通过 `[profile.release.build-override] strip = "none"` 只关闭构建依赖的剥离后构建成功；发布二进制仍保留原有 debuginfo 剥离设置。未升级工具链或增加运行服务。
- jsdom 缺少布局 API，测试环境提供 ResizeObserver、matchMedia 和无伪元素的样式读取；这些仅用于组件测试，不进入生产入口，不替代真实后端。

### 验收记录

环境：macOS ARM64，Rust/cargo 1.96.0、Node.js 22.23.2、pnpm 12.16.1。`sh scripts/check.sh` 最终 exit 0，包含：

| 命令 | 最终结果 |
| --- | --- |
| `pnpm --dir web install --frozen-lockfile` | 锁文件安装通过 |
| `pnpm --dir web run format:check` | Oxfmt 通过 |
| `pnpm --dir web run lint` | Oxlint 通过，无警告 |
| `pnpm --dir web run types:check` | Rust/TS 契约一致 |
| `pnpm --dir web run typecheck` | TypeScript 通过 |
| `pnpm --dir web test` | 4 个测试文件，11 项通过 |
| `pnpm --dir web run build` | Vite 生产构建及 web-dist 复制成功 |
| `cargo fmt --all -- --check` | 通过 |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 通过，无警告 |
| `cargo test --workspace --locked` | 3 项单元测试、4 项原有 HTTP 测试、17 项 Phase 1 集成测试，共 24 项通过 |
| `cargo build --workspace --release --locked` | 四个 crate、三个 release 二进制构建成功 |
| `pnpm --dir web run smoke` | 独立二进制与真实 HTTP 功能链路通过 |
| `pnpm --dir web audit`、`pnpm --dir web audit --prod` | 均 exit 0，无已知漏洞 |
| `sh -n scripts/check.sh` | shell 语法通过 |
| `postman status` | exit 0，CLI 可用，无远程 workspace 依赖 |

集成测试覆盖迁移重复运行及重新打开、WAL/外键/超时/权限、拒绝软链接、单次管理员初始化、Argon2id、全部管理路由拒绝匿名、Secure Cookie、哈希存储及响应脱敏、准确 Origin、CSRF 绑定、登出/过期/禁用/重复 Cookie、并发登录、Cookie 轮换、节点 CRUD、并发节点上限、非法输入与请求大小、限流、审计保留和审计写入失败时事务回滚。

冒烟将三个 release 二进制复制到独立临时目录，清空 PATH，并使用专用临时数据目录和随机测试密码。验证 CLI 初始化/拒绝覆盖、真实登录/节点创建修改撤销/审计/登出、HTTP 未授权及静态 JS/CSS；运行目录没有源码、迁移或前端资源文件，证明 SQL 和静态资源已内嵌，运行不需要 Node。进程与测试目录已自动清理。

另启动专用临时数据库的真实服务，在本机端口 56956 执行以下 Postman 检查：

```sh
postman request http://127.0.0.1:56956/api/v1/auth/me --timeout 5000 --script-post-request 'pm.test("Unauthenticated is 401", () => pm.response.to.have.status(401)); pm.test("Uniform error", () => pm.expect(pm.response.json()).to.eql({error:{code:"unauthorized",message:"Authentication required or credentials invalid"}})); pm.test("No store", () => pm.expect(pm.response.headers.get("Cache-Control")).to.equal("no-store")); pm.test("Server request ID", () => pm.expect(pm.response.headers.get("X-Request-ID")).to.match(/^[0-9a-f-]{36}$/)); pm.test("JSON error", () => pm.expect(pm.response.headers.get("Content-Type")).to.include("application/json"));'
postman request POST http://127.0.0.1:56956/api/v1/auth/login --timeout 5000 -H 'Origin:http://evil.invalid' -H 'Content-Type:application/json' --body '{}' --script-post-request 'pm.test("Cross site is 403", () => pm.response.to.have.status(403)); pm.test("Origin rejection code", () => pm.expect(pm.response.json().error.code).to.equal("csrf_failed")); pm.test("No store", () => pm.expect(pm.response.headers.get("Cache-Control")).to.equal("no-store"));'
```

实际返回预期的 401、403，分别 5 和 3 个断言通过。Postman CLI 1.82.0 对 HTTP 4xx 仍返回 exit 1，即使断言全部通过；如实记录该行为，不声称这些负向检查 exit 0。临时服务已停止，数据库目录已删除。

### 限制与下一阶段

- 只完成 Phase 1；无默认账号密码、无 Agent enrollment/WSS、无 Xray 或远程执行。下一阶段为 Phase 2，等待用户指示。
- 仍无生产 TLS，只允许 loopback；浏览器 HTTP 开发必须显式使用开发模式。没有实现生产安装、密码恢复、改密、多管理员或其他会话管理。
- 前端构建入口 992.36 kB，gzip 315.24 kB，保留 Vite 500 kB 提示，未调高阈值。当前 macOS ARM64 Server 约 7.2 MiB；不能据此推断 Linux 发布大小。
- 测试包含 jsdom 组件与真实进程 HTTP；未执行真实浏览器 E2E 或 Linux VM 测试。GitHub Ubuntu CI 已配置但未触发，未声称远程 CI 或生产安全审计完成。
- 无阻塞 Phase 1 验收的未完成事项。初次生成类型、临时目录权限、浏览器测试 API、登出缓存及 release 构建问题均已修正并复验。

## Phase 0 — Repository bootstrap

完成日期：2026-11-03（Asia/Shanghai）。状态：Phase 0 本地验收通过，已停止；等待用户验收。

### 初始检查

- 仓库没有提交记录；仅有未跟踪 `.gitignore`、`ARCHITECTURE_PLAN.md` 及被忽略的 IDE 目录。
- 无现有代码、依赖、测试或 CI。保留架构计划内容；仅扩展现有 ignore 规则。
- 按用户要求只执行 Phase 0。本次明确授权构建，覆盖默认“不自动构建”的工作约定。

### 实施清单

- [x] Rust 2024 workspace：server、agent、common、installer；固定工具链及依赖锁文件。
- [x] React + TypeScript + Vite + Ant Design + TanStack Query 健康检查页面。
- [x] 前端构建自动复制至 `crates/server/web-dist`，Rust 编译内嵌；缺少产物明确失败。
- [x] Axum `/api/v1/health`、静态资源 MIME/缓存、JSON 404/405、安全响应头。
- [x] Agent 启动打印版本；Installer help/version，无安装副作用。
- [x] ts-rs 类型生成和契约一致性检查。
- [x] 前端健康请求/失败恢复测试；Rust 健康、静态资源、路径及监听限制测试。
- [x] 本地检查脚本及固定 Actions SHA 的基础 CI。
- [x] README、architecture、security、protocol 与本进度文档。
- [x] 格式、Lint、类型检查、测试、构建全部通过。
- [x] 干净目录按文档顺序构建，独立二进制冒烟验证通过。

### 技术决策

1. Phase 0 HTTP 强制 loopback；不提前实现 TLS 或业务鉴权，也不开放公网监听。
2. 所有静态资源在 debug/release 都编译内嵌；不创建假页面。`web-dist` 是生成目录而非源码，构建前必须生成。
3. 只实现 `/` UI 路由，后续按需接入 React Router；避免空业务页面、空模块、Zustand 和额外服务。
4. TanStack Query 管理真实健康查询。Rust 类型是契约来源，前端运行时另校验 JSON。
5. Ant Design 动态样式需要当前 CSP 允许内联 style；脚本保持同源且禁止内联/eval，详见 security.md。
6. 暂不引入 SQLx、dialoguer、WebSocket 或 systemd 库；各自在对应 Phase 有真实用途时加入。
7. CLI 不暴露尚不可用的安装/连接操作；明确显示阶段限制。
8. 基础 CI 在 Ubuntu 运行与本地相同的检查脚本；签名发布、跨架构打包和 VM 测试留到计划阶段。

### 初次验收记录（Oxc 迁移前）

执行环境：macOS aarch64；Rust/cargo 1.96.0，Node.js 22.23.2，npm 10.9.8。

`sh scripts/check.sh` 在工作目录和干净源码快照中均以 exit 0 完成；脚本中的实际检查如下：

| 命令 | 结果 |
| --- | --- |
| `npm --prefix web ci` | 锁文件安装通过；开发依赖告警见下文 |
| `npm --prefix web run format:check` | Prettier 通过 |
| `npm --prefix web run lint` | ESLint 通过，0 警告 |
| `npm --prefix web run types:check` | ts-rs 生成结果与提交候选文件一致 |
| `npm --prefix web run typecheck` | TypeScript 通过 |
| `npm --prefix web test` | 2 个测试文件、5 项测试全部通过 |
| `npm --prefix web run build` | Vite 生产构建及 web-dist 复制成功 |
| `cargo fmt --all -- --check` | 通过 |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 通过，0 警告 |
| `cargo test --workspace --locked` | 2 项单元测试 + 4 项 HTTP 集成测试全部通过 |
| `cargo build --workspace --release --locked` | 四个 crate、三个 release 二进制构建成功 |
| `npm --prefix web run smoke` | 独立二进制、真实 HTTP/JS/CSS、路由边界和 CLI 安全检查通过 |
| `sh -n scripts/check.sh` | shell 语法检查通过 |
| `npm --prefix web audit --omit=dev` | exit 0，生产依赖 0 已知漏洞 |
| `npm audit --json`（web 下） | exit 1，12 个中危开发依赖条目，均源于 minimatch 公告 |
| `postman status` | exit 0，CLI 可用，未初始化远程 workspace、未登录；本阶段不需要远程发布 |

干净构建复验方式：将 `git ls-files --cached --others --exclude-standard -z` 列出的源码复制到 `/tmp/panel-phase0.GI5Yus`。开始前确认 `target`、`web/node_modules`、`web/dist`、`crates/server/web-dist` 都不存在，再执行相同的完整检查脚本；未复用工作目录构建产物。依赖下载缓存可复用。复验后已删除该临时源码目录。

冒烟脚本将三个 release 二进制复制到独立临时目录，以该目录为 cwd，清空 `PATH` 后启动：Agent 输出版本，Installer help/version 成功且拒绝 `install`，Server 拒绝公网 HTTP 绑定，并能提供健康 JSON、首页及非空 JS/CSS。确认运行不需要 Node 或静态文件目录。临时进程和冒烟目录自动清理。

另启动 `PANEL_BIND=127.0.0.1:0 ./target/release/panel-server`，获得本机端口 52451，执行以下真实 HTTP 检查，exit 0，5 个断言通过；验证后已停止服务：

```sh
postman request http://127.0.0.1:52451/api/v1/health --timeout 5000 --script-post-request 'pm.test("HTTP 200", () => pm.response.to.have.status(200)); pm.test("JSON content type", () => pm.expect(pm.response.headers.get("Content-Type")).to.include("application/json")); pm.test("Health contract", () => pm.expect(pm.response.json()).to.eql({status:"ok",version:"0.1.0"})); pm.test("No cache", () => pm.expect(pm.response.headers.get("Cache-Control")).to.equal("no-store")); pm.test("No MIME sniffing", () => pm.expect(pm.response.headers.get("X-Content-Type-Options")).to.equal("nosniff"));'
```

### 已知告警与验证限制

- 初次验收时，完整 npm 审计存在 12 个中危开发依赖条目，根因是 `minimatch@10.2.6`；已随下述 Oxc 工具链迁移移除。
- Vite 提示入口 JS 超过默认 500 kB：628.09 kB，gzip 201.67 kB。当前只有一个健康页面，保留提示；后续多页面按实际需求分包，不通过调高阈值掩盖提示。
- 本机 release 大小约为 Server 2.3 MiB、Agent 845 KiB、Installer 828 KiB；属于 macOS ARM64 产物，不能用来推断 Linux 发布大小。
- 未在真实浏览器执行 E2E；前端行为由 jsdom 测试覆盖，真实服务由 HTTP/静态资源冒烟覆盖。
- GitHub Ubuntu CI 已配置但尚未触发；Linux x86_64/aarch64 发布及 VM 验证未执行，留到计划阶段。不能将本地通过等同于远程 CI 或 Linux 发布验证通过。
- 无阻塞 Phase 0 的事项。初次发现的生成目录缺失及测试对 Ant Design 动画名称的依赖已修复，全部检查已复验通过。

### 未完成及下一阶段

- Phase 1：SQLite/SQLx 迁移、首个管理员初始化、登录/登出/会话/CSRF、节点 CRUD 和审计日志；未开始。
- Phase 2–8：节点连接、Linux 管理、Xray、流量配额、交互安装、HTTPS 与签名发布均未开始。
- 本阶段验收通过后停止，等待用户验收再实施后续阶段。

## Phase 0 修订 — web 使用 Oxc 工具链

日期：2026-11-03。状态：迁移完成，全部本地检查通过；不进入 Phase 1。

### 范围与技术决策

- 检查确认 Vite 8.3.5 和 `@vitejs/plugin-react` 6.2.1 已使用 Oxc 转换/压缩，无需额外引入编译插件。
- 使用固定版本 Oxfmt 0.75.0 和 Oxlint 1.90.0 替换 Prettier、ESLint 及其专用插件，更新 npm 锁文件。
- `.oxfmtrc.json` 保留原有 80 列格式宽度，忽略生成的 TypeScript 契约、锁文件和 dist；node_modules 默认排除。
- `.oxlintrc.json` 启用 correctness 类别，并明确保留 Hooks 调用规则、依赖数组检查、React Refresh 导出规则与禁止显式 any。无需加载 ESLint JS 插件。
- 保留 `tsc` 进行完整类型检查；不引入实验性类型检查器，不声称 Oxlint 与旧 ESLint 配置完全等价。
- CI 仍执行 `scripts/check.sh`，通过原有 `format:check`/`lint` 命令自动使用新工具。
- 不修改业务逻辑或 Rust 代码；同步 README、architecture、security 和本进度记录。

### 复验结果

- `sh scripts/check.sh`：exit 0。`npm ci`、Oxfmt 格式检查、Oxlint、Rust/TS 契约、TypeScript 类型检查、前端 5 项测试、Rust 6 项测试、Rust 格式/Clippy、前后端构建和独立二进制冒烟检查全部通过。
- 用临时 TSX 错误样例分别验证 `rules-of-hooks`、`exhaustive-deps`、`only-export-components`、`no-explicit-any`、`no-unused-vars`：每项均返回 exit 1 并包含预期规则诊断；临时样例已清理。
- `npm audit`、`npm audit --omit=dev`（web 下）：均 exit 0，0 已知漏洞。检查锁文件确认 ESLint、Prettier、typescript-eslint、旧 React Lint 插件和 minimatch 均已移除。
- `npm ls oxlint oxfmt`：确认 Oxlint 1.90.0、Oxfmt 0.75.0。安装时移除旧工具链 128 个包，新增 5 个包。
- 构建产物名称与大小和迁移前一致；Vite 的 500 kB 包体积提示仍保留。此次仅在 macOS ARM64 本地复验，未触发 GitHub CI。
- 无未完成的迁移事项；Phase 1 及以后阶段仍未开始。

## Phase 0 修订 — 包管理使用 pnpm

日期：2026-11-03。状态：迁移完成，全部本地检查通过；不进入 Phase 1。

### 范围与技术决策

- `web/package.json` 的 `packageManager` 固定 pnpm 12.16.1，与当前已安装版本一致。继续使用 `web/` 单包结构，不增加根目录 package 或 pnpm workspace。
- 从现有 npm 锁文件执行 `pnpm --dir web import`，生成 `web/pnpm-lock.yaml` 后移除 `web/package-lock.json`；不主动升级业务或工具依赖。
- `scripts/check.sh` 改为 `pnpm --dir web`，安装使用 `--frozen-lockfile`。CI 通过固定 SHA 的 `pnpm/action-setup` 读取 `web/package.json` 安装同一 pnpm 版本。
- 同步 Oxfmt 锁文件忽略规则、README、架构/接口/安全文档、Rust 构建提示、类型生成器及生成文件中的命令。
- 按用户最新要求同步 ARCHITECTURE_PLAN.md 中的锁文件路径和两处构建命令；保留此前验收记录中的 npm 历史命令。

### 复验结果

- `pnpm --dir web import`：exit 0。检查确认全部 15 个直接依赖版本与 npm 锁文件一致，随后删除旧锁文件。
- 删除生成的 `web/node_modules` 后执行 `pnpm --dir web install --frozen-lockfile`：exit 0，成功安装 167 个包，验证不依赖 npm 原先的平铺依赖目录。
- `pnpm --dir web run types`、`pnpm --dir web run format`：exit 0，重新生成带 pnpm 提示的契约文件，并格式化包管理配置。
- `sh scripts/check.sh`：exit 0。pnpm 冻结锁文件安装、Oxfmt、Oxlint、Rust/TS 契约、TypeScript、前端 5 项测试、Rust 6 项测试、Rust fmt/Clippy、前后端构建和独立二进制冒烟检查全部通过。
- `pnpm --dir web audit`、`pnpm --dir web audit --prod`：均 exit 0，未发现已知漏洞。
- `sh -n scripts/check.sh`：exit 0。检查当前脚本、文档、生成器和构建提示已使用 pnpm；仅历史验收记录保留 npm 命令，README 的 npm 指依赖 registry。
- CI 的 pnpm/action-setup 固定到 v4.2.0 标签解析后的真实提交 `41ff72655975bd51cab0327fa583b6e92b6d3061`；GitHub CI 尚未触发。
- 构建产物及现有包体积提示与迁移前一致。无未完成的迁移事项；验证环境仍为 macOS ARM64，Phase 1 及以后阶段未开始。
