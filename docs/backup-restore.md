# 备份与恢复

备份包含管理员哈希、会话/节点凭据、REALITY 私钥、客户端 UUID 和 TLS 私钥，必须与原机数据同等保护。只存于 root 私有目录并使用受控加密/离线介质；不上传到 Git、普通 artifact 或公共对象存储。保留原版本的已验证二进制和安装 manifest，恢复前验证备份来源与完整性。

MVP 支持单个 Server 写入进程，并以 server.lock 阻止同数据目录并发 Server/初始化。常规冷备份先停止所有使用该数据目录的 Server；需要同时备份本机 Agent 身份时也停止 Agent。Xray 可继续使用最后健康配置，不会因控制面停机而被卸载。

```sh
sudo systemctl stop panel-agent.service panel-server.service
sudo install -d -m 700 /root/panel-backups
# 默认路径；自定义 server_data 应使用 manifest 中准确的原路径。
sudo tar -C / -czf /root/panel-backups/panel-cold-backup.tar.gz \
  var/lib/panel/server var/lib/panel/agent etc/panel var/lib/panel-installer opt/panel/xray
sudo chmod 600 /root/panel-backups/panel-cold-backup.tar.gz
sudo sha256sum /root/panel-backups/panel-cold-backup.tar.gz
sudo systemctl start panel-server.service panel-agent.service
```

未安装的组件/Xray 路径应从命令明确移除；不要把 tar 的“路径不存在”错误当作完整成功。不要在 Server/Agent 仍写入时仅复制 SQLite 主文件，WAL 中可能有尚未 checkpoint 的已提交数据；可使用 SQLite 正规 backup API。目录备份应保留整个停止后的数据库/WAL/归属标记及 uid/gid/mode。

同机恢复：停止相关服务，校验并审阅可信备份的路径、文件类型与所含 manifest；仅恢复同一归属安装内的目标。不得将未经认证的 tar 直接以 root 解包。保留恢复前的另一份快照以便回退，恢复同版本代码与匹配的 root manifest、专用账号 UID/GID、目录 0700、SQLite 0600、TLS key 0600。不要删除归属标记或把未知文件 chown 后冒充受管安装。跨主机 UID/路径不同需要显式迁移与重新注册，不能照搬文件权限数字或覆盖现有主机安装。

恢复旧数据库可能重新激活已经撤销的会话/节点凭据，因此在重新开放 Server 前，使用离线 SQLite 维护工具（例如 Python 标准 sqlite3，仅维护时使用）执行以下事务：

```sql
BEGIN IMMEDIATE;
UPDATE sessions SET revoked_at=unixepoch();
UPDATE enrollments SET consumed_at=COALESCE(consumed_at,unixepoch());
UPDATE node_credentials SET revoked_at=unixepoch();
UPDATE nodes SET status='offline',connection_id=NULL WHERE revoked_at IS NULL;
COMMIT;
```

随后启动 Server，用原管理员密码重新登录，在面板为保留节点签发新的单次 token，以对应非 root Agent 身份重新 enroll，再启动 Agent。保留审计记录；在恢复运维记录中记下来源、时间、原版本和重新注册动作，不记录凭据。检查 HTTPS、数据/入站/客户端、配置修订、Agent 状态和流量过期提示后再恢复访问。

`verify-install.py --backup-restore` 在真实 Linux 服务上停机备份两个 SQLite、注入业务数据变更、恢复、撤销旧会话/凭据并以新 token 注册，检查原业务数据和节点在线状态；这验证冷备份流程，不宣称任意未审阅 tar 或跨主机自动迁移安全。
