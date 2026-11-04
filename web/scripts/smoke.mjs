import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { once } from "node:events";
import { copyFile, mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { randomBytes, randomUUID } from "node:crypto";

// Run copied release binaries outside the checkout, with no executable search path.
const root = fileURLToPath(new URL("../../", import.meta.url));
const workspace = await readFile(join(root, "Cargo.toml"), "utf8");
const version = workspace.match(/^version = "([^"]+)"/m)?.[1];
assert.ok(version, "Workspace version is missing");
const directory = await mkdtemp(join(tmpdir(), "panel-smoke-"));
const env = {
  ...process.env,
  PATH: "",
  PANEL_BIND: "127.0.0.1:0",
  PANEL_DATA_DIR: join(directory, "data"),
  PANEL_DEV_HTTP: "1",
};
delete env.PANEL_ORIGIN;
delete env.PANEL_TLS_CERT;
delete env.PANEL_TLS_KEY;
let server;
const agents = [];

function cli(name, args, expectedStatus = 0, overrides = {}, input) {
  const result = spawnSync(join(directory, name), args, {
    cwd: directory,
    env: { ...env, ...overrides },
    encoding: "utf8",
    timeout: 5000,
    input,
  });
  assert.ifError(result.error);
  assert.equal(result.status, expectedStatus, result.stderr);
  return result.stdout + result.stderr;
}

try {
  for (const name of ["panel-server", "panel-agent", "panel-installer"]) {
    await copyFile(join(root, "target/release", name), join(directory, name));
  }
  assert.equal(cli("panel-agent", []).trim(), `panel-agent ${version}`);
  assert.equal(
    cli("panel-agent", ["--version"]).trim(),
    `panel-agent ${version}`,
  );
  assert.match(cli("panel-installer", ["--help"]), /Usage:/);
  assert.equal(
    cli("panel-installer", ["--version"]).trim(),
    `panel-installer ${version}`,
  );
  assert.match(
    cli("panel-installer", ["install"], 1),
    /requires Linux|requires root|requires a TTY/,
  );
  assert.match(
    cli("panel-server", [], 1, { PANEL_BIND: "0.0.0.0:8080" }),
    /loopback/,
  );
  const password = randomBytes(24).toString("hex");
  const initArgs = [
    "init-admin",
    "--username",
    "smoke-admin",
    "--password-stdin",
  ];
  assert.match(
    cli("panel-server", initArgs, 0, {}, `${password}\n`),
    /Administrator initialized/,
  );
  assert.match(
    cli("panel-server", initArgs, 1, {}, `${password}\n`),
    /already initialized/,
  );

  server = spawn(join(directory, "panel-server"), [], {
    cwd: directory,
    env,
    stdio: ["ignore", "pipe", "inherit"],
  });
  let timer;
  const base = await new Promise((resolve, reject) => {
    timer = setTimeout(
      () => reject(new Error("Server startup timed out")),
      10000,
    );
    let output = "";
    server.stdout.on("data", (chunk) => {
      output += chunk;
      const match = output.match(/listening on (http:\/\/127\.0\.0\.1:\d+)/);
      if (match) resolve(match[1]);
    });
    server.once("error", reject);
    server.once("exit", (code) => reject(new Error(`Server exited: ${code}`)));
  }).finally(() => clearTimeout(timer));

  assert.match(cli("panel-server", [], 1), /already in use/);

  const get = (path) =>
    fetch(`${base}${path}`, { signal: AbortSignal.timeout(5000) });
  const health = await get("/api/v1/health");
  assert.equal(health.status, 200);
  assert.deepEqual(await health.json(), { status: "ok", version });
  const index = await get("/");
  assert.equal(index.status, 200);
  assert.equal(index.headers.get("cache-control"), "no-cache");
  const html = await index.text();
  const assets = [...html.matchAll(/(?:src|href)="(\/assets\/[^"]+)"/g)];
  assert.ok(assets.length >= 2, "Expected built JS and CSS assets");
  for (const [, path] of assets) {
    const asset = await get(path);
    assert.equal(asset.status, 200, path);
    assert.equal(
      asset.headers.get("cache-control"),
      "public, max-age=31536000, immutable",
    );
    assert.ok((await asset.arrayBuffer()).byteLength > 0);
  }
  for (const path of [
    "/api/v1/unknown",
    "/assets/missing.js",
    "/.git/config",
    "/nodes",
  ]) {
    const response = await get(path);
    assert.equal(response.status, 404, path);
    assert.equal((await response.json()).error.code, "not_found");
  }
  assert.equal((await get("/api/v1/nodes")).status, 401);
  const loggedIn = await fetch(`${base}/api/v1/auth/login`, {
    method: "POST",
    headers: { Origin: base, "Content-Type": "application/json" },
    body: JSON.stringify({ username: "smoke-admin", password }),
  });
  assert.equal(loggedIn.status, 200);
  const cookie = loggedIn.headers.get("set-cookie").split(";")[0];
  const session = await loggedIn.json();
  const headers = {
    Cookie: cookie,
    Origin: base,
    "X-CSRF-Token": session.csrf_token,
    "Content-Type": "application/json",
  };
  const api = (path, method = "GET", body) =>
    fetch(`${base}/api/v1${path}`, {
      method,
      headers,
      ...(body === undefined ? {} : { body: JSON.stringify(body) }),
      signal: AbortSignal.timeout(5000),
    });
  const created = await api("/nodes", "POST", { name: "Smoke node" });
  assert.equal(created.status, 201);
  const node = await created.json();
  assert.equal(node.status, "offline");
  const changed = await api(`/nodes/${node.id}`, "PATCH", {
    name: "Renamed smoke node",
  });
  assert.equal(changed.status, 200);
  assert.equal((await changed.json()).name, "Renamed smoke node");
  assert.equal((await api(`/nodes/${node.id}`, "DELETE")).status, 204);
  const entries = await (await api("/audit")).json();
  assert.ok(entries.some((entry) => entry.action === "node.revoke"));
  assert.ok(!JSON.stringify(entries).includes(password));
  const waitFor = async (check) => {
    const deadline = Date.now() + 12000;
    while (!(await check())) {
      assert.ok(Date.now() < deadline, "Agent smoke condition timed out");
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
  };
  const agentNodes = [];
  for (const number of [1, 2]) {
    const response = await api("/nodes", "POST", { name: `Agent ${number}` });
    assert.equal(response.status, 201);
    const node = await response.json();
    agentNodes.push(node.id);
    const registration = await api("/nodes/enrollments", "POST", {
      node_id: node.id,
    });
    assert.equal(registration.status, 200);
    const { token } = await registration.json();
    const agentDirectory = join(env.PANEL_DATA_DIR, `agent-${number}`);
    const args = ["--allow-local-ws", "--data-dir", agentDirectory];
    assert.match(
      cli(
        "panel-agent",
        [
          ...args,
          "enroll",
          "--server",
          base.replace("http:", "ws:") + "/agent/v1/connect",
          "--token-stdin",
        ],
        0,
        {},
        `${token}\n`,
      ),
      /Agent enrolled/,
    );
    const child = spawn(join(directory, "panel-agent"), [...args, "run"], {
      cwd: directory,
      env,
      stdio: ["ignore", "ignore", "pipe"],
    });
    child.stderr.resume();
    agents.push({ child, args });
  }
  await waitFor(async () =>
    (await (await api("/nodes")).json())
      .filter((entry) => agentNodes.includes(entry.id))
      .every((entry) => entry.status === "online"),
  );
  for (const id of agentNodes) {
    const probe = await api(`/nodes/${id}/tasks`, "POST", {
      operation: "agent_status",
      idempotency_key: randomUUID(),
    });
    assert.equal(probe.status, 201);
    const task = await probe.json();
    await waitFor(
      async () =>
        (await (await api(`/tasks/${task.id}`)).json()).status === "succeeded",
    );
    await waitFor(async () => {
      const response = await api(`/nodes/${id}/metrics`);
      assert.equal(response.status, 200);
      const metrics = await response.json();
      if (!metrics.snapshot || metrics.stale) return false;
      if (process.platform === "linux") {
        assert.ok(Number(metrics.snapshot.memory.total_bytes) > 0);
      } else {
        assert.equal(metrics.snapshot.memory, null);
        assert.ok(
          metrics.snapshot.issues.some(
            (issue) => issue.reason === "unsupported_platform",
          ),
        );
      }
      return true;
    });
  }
  const stopped = once(agents[0].child, "exit");
  agents[0].child.kill("SIGKILL");
  await stopped;
  await waitFor(
    async () =>
      (await (await api(`/nodes/${agentNodes[0]}/status`)).json()).status ===
      "offline",
  );
  const restarted = spawn(
    join(directory, "panel-agent"),
    [...agents[0].args, "run"],
    { cwd: directory, env, stdio: ["ignore", "ignore", "pipe"] },
  );
  restarted.stderr.resume();
  agents.push({ child: restarted });
  await waitFor(
    async () =>
      (await (await api(`/nodes/${agentNodes[0]}/status`)).json()).status ===
      "online",
  );
  assert.equal((await api(`/nodes/${agentNodes[1]}`, "DELETE")).status, 204);
  assert.equal((await api("/auth/logout", "POST")).status, 204);
  assert.equal((await api("/nodes")).status, 401);
  console.log(
    "Release smoke passed: standalone binaries, embedded assets, authentication, two Agent processes, durable enrollment, tasks, agent restart, revocation and CLI safety.",
  );
} finally {
  for (const { child } of agents) {
    if (child.exitCode === null && child.signalCode === null) {
      const exited = once(child, "exit");
      child.kill("SIGKILL");
      await exited;
    }
  }
  if (server && server.exitCode === null && server.signalCode === null) {
    const exited = once(server, "exit");
    server.kill("SIGKILL");
    await exited;
  }
  await rm(directory, { recursive: true, force: true });
}
