Rust Linux Panel MVP：单二进制内嵌 React UI、SQLite 管理认证、多节点出站 WSS、Linux 指标、受限 systemd/Xray 生命周期、VLESS+REALITY 配置修订与回滚、流量/最终一致配额，以及非驻留交互安装器。

提供 Linux x86_64/aarch64 静态 musl 构建。先按 VERIFICATION.md 验证 GitHub OIDC 签名的 SHA256SUMS，再核对平台压缩包与 bootstrap.sh；安装需要自己的 TLS 证书和管理员密码，没有默认凭据。Agent/Server 不以 root 长期运行，root helper 只支持固定受管操作。

普通卸载保留数据；永久清理需要准确输入确认。MVP 配额是最终一致策略，节点失联仍保留最后健康配置。自动 ACME、证书热加载、多管理员 RBAC、通用远程命令不在本版本范围。
