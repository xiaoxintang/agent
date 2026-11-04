// Real browser + real Linux installer fixture. No traces, videos or credential screenshots.
import assert from "node:assert/strict";
import { firefox, expect } from "@playwright/test";
import { execFileSync, spawnSync } from "node:child_process";
import { readFile, writeFile, mkdtemp, chmod, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

const statePath = process.env.PANEL_E2E_STATE_FILE;
assert.ok(statePath, "PANEL_E2E_STATE_FILE must name a private fixture file");
const state = JSON.parse(await readFile(statePath, "utf8"));
const folder = await mkdtemp(join(tmpdir(), "panel-browser-"));
await chmod(folder, 0o700);
const certificate = join(folder, "peer.pem");
await writeFile(certificate, state.certificate_pem, { mode: 0o600 });
execFileSync("certutil", ["-N", "-d", `sql:${folder}`, "--empty-password"]);
execFileSync("certutil", [
  "-A",
  "-d",
  `sql:${folder}`,
  "-n",
  "Panel isolated E2E peer",
  "-t",
  "C,,",
  "-i",
  certificate,
]);
const quote = (value) => `'${value.replaceAll("'", "'\\''")}'`;
const context = await firefox.launchPersistentContext(folder, {
  headless: true,
  ignoreHTTPSErrors: false,
  firefoxUserPrefs: { "security.enterprise_roots.enabled": false },
});
const page = await context.newPage();
page.setDefaultTimeout(45000);
const errors = [];
page.on("pageerror", (error) => errors.push(error.message));
async function confirm() {
  await page
    .locator(".ant-popconfirm")
    .getByRole("button", { name: /OK|确/ })
    .click();
}
try {
  await page.goto(state.origin);
  await page.getByLabel("账号", { exact: true }).fill(state.username);
  await page.getByLabel("密码", { exact: true }).fill(state.password);
  await page.getByRole("button", { name: /登\s*录/ }).click();
  const row = page.locator(`tr[data-row-key="${state.node_id}"]`);
  await expect(row.getByText("在线", { exact: true })).toBeVisible();
  await row.getByRole("button", { name: "注册 Agent", exact: true }).click();
  const enrollment = page.getByRole("dialog", {
    name: "注册 Agent",
    exact: true,
  });
  const tokenElement = enrollment
    .locator(".ant-typography")
    .filter({ hasText: /^[a-f0-9]{64}$/ })
    .first();
  await expect(tokenElement).toBeVisible();
  const token = (await tokenElement.innerText()).trim();
  assert.match(token, /^[a-f0-9]{64}$/);
  await enrollment.getByRole("button", { name: "关闭并清除令牌" }).click();
  await expect(enrollment).toHaveCount(0);
  const remote = `import sys,subprocess\ntoken=sys.stdin.read()\nsubprocess.run(['/usr/bin/systemctl','stop','panel-agent.service'],check=True)\nsubprocess.run(['/usr/sbin/runuser','-u','panel-agent','--','/usr/local/bin/panel-agent','--data-dir','/var/lib/panel/agent','--ca-cert','/etc/panel/agent-ca.pem','enroll','--server',${JSON.stringify(state.agent_server)},'--token-stdin'],input=token,text=True,check=True)\nsubprocess.run(['/usr/bin/systemctl','start','panel-agent.service'],check=True)`;
  const enrolled = spawnSync(
    "ssh",
    [...state.ssh_args, `sudo -n python3 -c ${quote(remote)}`],
    { input: token, encoding: "utf8", timeout: 60000 },
  );
  assert.equal(
    enrolled.status,
    0,
    "Fixture Agent re-enrollment failed; credentials redacted",
  );
  await expect(row.getByText("在线", { exact: true })).toBeVisible();
  await row.getByRole("button", { name: "Xray 管理", exact: true }).click();
  const xray = page.getByRole("dialog", { name: "受管 Xray", exact: true });
  await xray.getByRole("button", { name: "卸载并保留数据" }).click();
  await confirm();
  await expect(xray.getByText(/未安装（保留的数据/)).toBeVisible({
    timeout: 120000,
  });
  await xray.getByRole("button", { name: "安装所选版本" }).click();
  await expect(xray.getByText(/已安装 26\.10\.31.*运行中/)).toBeVisible({
    timeout: 240000,
  });
  await xray.getByRole("button", { name: "关闭 Xray 管理" }).click();
  await row.getByRole("button", { name: "入站与客户端", exact: true }).click();
  const config = page.getByRole("dialog", {
    name: "VLESS + REALITY 配置",
    exact: true,
  });
  await config.getByLabel("入站名称", { exact: true }).fill("Browser inbound");
  await config.getByLabel("监听 IP", { exact: true }).fill("127.0.0.1");
  await config
    .getByLabel("监听端口", { exact: true })
    .fill(String(state.browser_port));
  await config
    .getByLabel("REALITY 目标（域名:端口）", { exact: true })
    .fill("www.example.com:443");
  await config
    .getByLabel("SNI 域名（逗号分隔）", { exact: true })
    .fill("www.example.com");
  await config
    .getByLabel("Short ID（偶数位十六进制，逗号分隔）", { exact: true })
    .fill("abcd");
  await config.getByRole("button", { name: "保存入站", exact: true }).click();
  await expect(
    config.getByRole("button", { name: "编辑 Browser inbound", exact: true }),
  ).toBeVisible();
  await config.getByLabel("客户端名称", { exact: true }).fill("Browser client");
  await config.getByRole("button", { name: "创建客户端", exact: true }).click();
  await expect(
    config.getByText("客户端 UUID 仅此时显示，请安全保存", { exact: true }),
  ).toBeVisible();
  await config
    .getByRole("combobox", { name: "Browser inbound 的客户端", exact: true })
    .click();
  await page
    .getByRole("option", { name: "Browser client", exact: true })
    .click();
  await page.keyboard.press("Escape");
  await config
    .getByRole("button", { name: "应用配置修订", exact: true })
    .click();
  await confirm();
  await expect(config.getByText(/修订 2 · applied/)).toBeVisible({
    timeout: 120000,
  });
  await config.getByRole("button", { name: "关闭配置", exact: true }).click();
  await expect(config).toHaveCount(0);
  assert.equal(
    (await page.evaluate(() => JSON.stringify(localStorage))).includes(token),
    false,
  );
  await row.getByRole("button", { name: "流量统计", exact: true }).click();
  const traffic = page.getByRole("dialog", { name: "节点流量", exact: true });
  await expect(traffic.getByText(/统计不可用或已过期/)).toHaveCount(0, {
    timeout: 45000,
  });
  await expect(traffic.getByText(/最近 7 天每小时/)).toBeVisible();
  await traffic.getByRole("button", { name: "关闭流量", exact: true }).click();
  assert.deepEqual(errors, []);
  console.log(
    JSON.stringify({
      real_firefox_https: true,
      explicit_peer_trust_without_tls_bypass: true,
      browser_login_enrollment_xray_install_inbound_client_assignment_apply_traffic: true,
    }),
  );
} finally {
  await context.close();
  await rm(folder, { recursive: true, force: true });
  const signal = spawnSync(
    "ssh",
    [...state.ssh_args, `sudo -n touch ${quote(state.browser_done)}`],
    { encoding: "utf8", timeout: 15000 },
  );
  assert.equal(signal.status, 0, "Browser fixture completion signal failed");
}
