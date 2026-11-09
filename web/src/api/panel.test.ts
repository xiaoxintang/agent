import { expect, it, vi } from "vitest";
import {
  ApiError,
  applyConfig,
  createNode,
  createServiceStatusTask,
  createStatusTask,
  createXrayTask,
  getSession,
  getNodeSummaries,
  login,
  logout,
} from "./panel";

it("creates task UUIDs when randomUUID is unavailable", async () => {
  const getRandomValues = crypto.getRandomValues.bind(crypto);
  vi.stubGlobal("crypto", { getRandomValues });
  const fetch = vi
    .fn()
    .mockImplementation(() => Promise.resolve(new Response("{}")));
  vi.stubGlobal("fetch", fetch);
  await createStatusTask("node", "csrf");
  await createServiceStatusTask("node", "xray", "csrf");
  await createXrayTask("node", "status", "csrf");
  await applyConfig("node", "csrf");
  const keys = fetch.mock.calls.map(
    (call) => JSON.parse(call[1].body).idempotency_key as string,
  );
  expect(keys).toHaveLength(4);
  expect(new Set(keys).size).toBe(4);
  for (const key of keys) {
    expect(key).toMatch(
      /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/,
    );
  }
});

it("treats an expired or absent session as logged out", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn().mockResolvedValue(new Response("", { status: 401 })),
  );
  await expect(getSession()).resolves.toBeNull();
});

it("uses HttpOnly cookies and includes CSRF on state changes", async () => {
  const fetch = vi
    .fn()
    .mockResolvedValueOnce(
      new Response(JSON.stringify({ id: "node-id", name: "Node" })),
    )
    .mockResolvedValueOnce(new Response(null, { status: 204 }));
  vi.stubGlobal("fetch", fetch);
  await createNode({ name: "Node" }, "test-csrf");
  expect(fetch).toHaveBeenNthCalledWith(1, "/api/v1/nodes", {
    method: "POST",
    credentials: "same-origin",
    cache: "no-store",
    headers: {
      "Content-Type": "application/json",
      "X-CSRF-Token": "test-csrf",
    },
    body: '{"name":"Node"}',
  });
  await expect(logout("test-csrf")).resolves.toBeUndefined();
  expect(fetch).toHaveBeenNthCalledWith(2, "/api/v1/auth/logout", {
    method: "POST",
    credentials: "same-origin",
    cache: "no-store",
    headers: { "X-CSRF-Token": "test-csrf" },
  });
});

it("does not expose server details or credentials in login errors", async () => {
  vi.stubGlobal(
    "fetch",
    vi
      .fn()
      .mockResolvedValue(
        new Response("private server details", { status: 401 }),
      ),
  );
  const result = login({ username: "admin", password: "test-password" });
  await expect(result).rejects.toBeInstanceOf(ApiError);
  await expect(result).rejects.toThrow("账号或密码错误");
});

it("uses safe business errors and preserves the request ID for diagnostics", async () => {
  const requestId = "7a5d65fa-c3c9-4243-8d22-a852a57685dc";
  vi.stubGlobal(
    "fetch",
    vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          error: { code: "task_queue_full", message: "private-token-password" },
        }),
        { status: 409, headers: { "X-Request-ID": requestId } },
      ),
    ),
  );
  const error = await createStatusTask("node", "csrf").catch(
    (error: unknown) => error,
  );
  expect(error).toMatchObject({
    status: 409,
    code: "task_queue_full",
    requestId,
  });
  expect((error as Error).message).toContain("任务队列已满");
  expect((error as Error).message).toContain(requestId);
  expect((error as Error).message).not.toContain("private-token-password");
});

it("does not display unknown error codes or malformed server bodies", async () => {
  for (const body of [
    "private-server-error",
    JSON.stringify({
      error: { code: "__proto__", message: "private-server-error" },
    }),
  ]) {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(new Response(body, { status: 500 })),
    );
    await expect(createStatusTask("node", "csrf")).rejects.toThrow(
      "请求失败，请稍后重试",
    );
  }
});

it("passes cancellation through to the batch summary request", async () => {
  const fetch = vi.fn().mockResolvedValue(new Response("[]"));
  vi.stubGlobal("fetch", fetch);
  const abort = new AbortController();
  await getNodeSummaries(abort.signal);
  expect(fetch).toHaveBeenCalledWith(
    "/api/v1/nodes/summary",
    expect.objectContaining({ signal: abort.signal }),
  );
});
