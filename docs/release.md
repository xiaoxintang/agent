# 发布与验证

正式发布只由 `v数字.数字.数字` 格式的稳定版本标签触发，不要求标签签名。发布权限应通过仓库权限和标签保护规则控制。

发布工作流先校验标签，再在原生 Ubuntu x86_64/aarch64 runner 上构建 web、执行完整检查、构建静态 musl ELF，执行真实 TTY/systemd/Xray/故障回滚/分发验证。生产压缩包只包含 bundle.json 和三个二进制，模板/引导脚本另作 release assets；运行不需要 Node 或编译工具。

`SHA256SUMS` 的签名使用 GitHub Actions OIDC 短期身份，Sigstore bundle 包含可验证证书和透明日志证据，不向 CI 上传长期私钥。可信身份严格为当前仓库、工作流路径和版本标签，不使用任意 identity 正则。

取得对应版本的 SHA256SUMS、SHA256SUMS.sigstore.json、所需平台压缩包及 bootstrap.sh 后，使用可信来源安装的 cosign 3.1.4 或兼容版本验证：

```sh
cosign verify-blob \
  --bundle SHA256SUMS.sigstore.json \
  --certificate-identity 'https://github.com/xiaoxintang/agent/.github/workflows/release.yml@refs/tags/v0.1.0' \
  --certificate-oidc-issuer 'https://token.actions.githubusercontent.com' \
  SHA256SUMS
# 只下载部分 assets 时逐个核对所需文件，不把 missing 文件忽略说成全量校验成功。
sha256sum --check SHA256SUMS
```

版本变化时必须同时修改精确 identity 和标签。先认证 checksum manifest，再信任其中摘要；从未知镜像同时下载二进制和 checksum 不能建立可信身份。cosign 本身也必须来自可信安装渠道。不要用 TLS 跳过参数替代身份验证。

bootstrap.sh 会从 GitHub Release 下载当前固定版本的对应平台归档，并检查归档文件结构与包内二进制摘要：

```sh
sudo sh bootstrap.sh
```

引导程序固定 release 版本、GitHub HTTPS 来源、下载大小和路径白名单；不执行下载流，不使用 curl|sh，保留 TTY 给 Installer。非交互配置应是 root-owned 0600 JSON，使用 `--non-interactive --config /root/panel-install.json`。单独调用 Installer 时可以提供 `--bundle-sha256` 对元数据做额外校验。

维护发布时先修改 Cargo workspace 版本，完整验收并提交，然后创建并推送版本标签：

```sh
git tag v0.1.0
git push origin main v0.1.0
```

只有真实 workflow 与 release 资产均成功后，才能称为已发布。公开签名可以验证项目的发布来源与完整性，不等于第三方安全审计，也不声称可复现构建。
