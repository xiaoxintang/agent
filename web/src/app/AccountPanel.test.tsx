import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import AccountPanel, { Management } from "./AccountPanel";
import { MemoryRouter } from "react-router-dom";

const session = {
  admin: { id: "admin-id", username: "admin" },
  csrf_token: "test-csrf",
  expires_at: 9999999999,
};
const response = (value: unknown) => new Response(JSON.stringify(value));

it("shows node metrics, stale data and allows setting the local node", async () => {
  const fetch = vi.fn((url: string, options?: RequestInit) => {
    if (url.endsWith("/local-node") || url.endsWith("/local-node/node-1"))
      return Promise.resolve(
        response({ node_id: options?.method === "PUT" ? "node-1" : null }),
      );
    if (url.endsWith("/nodes/summary"))
      return Promise.resolve(
        response([
          {
            node: { id: "node-1", name: "node one", status: "online" },
            metrics: {
              stale: true,
              cpu_percent: 25,
              memory: { total_bytes: "100", used_bytes: "50" },
            },
          },
          {
            node: { id: "node-2", name: "node two", status: "offline" },
            metrics: {
              stale: true,
              cpu_percent: null,
              memory: null,
            },
          },
        ]),
      );
    throw new Error(`Unexpected test request: ${url}`);
  });
  vi.stubGlobal("fetch", fetch);
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <Management session={session} page="nodes" />
    </QueryClientProvider>,
    { wrapper: MemoryRouter },
  );
  expect(await screen.findByText("25.0%（过期）")).toBeTruthy();
  expect(await screen.findByText("50.0%（过期）")).toBeTruthy();
  expect(screen.getAllByText("暂无数据")).toHaveLength(2);
  expect(
    fetch.mock.calls.filter(([url]) => url.endsWith("/nodes/summary")),
  ).toHaveLength(1);
  expect(fetch.mock.calls.some(([url]) => url.endsWith("/metrics"))).toBe(
    false,
  );
  expect(
    screen.getAllByRole("link", { name: "管理节点" })[1].getAttribute("href"),
  ).toBe("/nodes/node-2/resources");
  fireEvent.click(screen.getAllByRole("button", { name: "设为本机" })[0]);
  await waitFor(() =>
    expect(
      fetch.mock.calls.some(
        ([url, options]) =>
          url.endsWith("/local-node/node-1") &&
          options?.method === "PUT" &&
          (options.headers as Record<string, string>)["X-CSRF-Token"] ===
            "test-csrf",
      ),
    ).toBe(true),
  );
  client.clear();
});

it("logs in, creates and edits a real API-backed node, then logs out", async () => {
  let loggedIn = false;
  let nodes: { id: string; name: string; status: string }[] = [];
  const fetch = vi.fn((url: string, options?: RequestInit) => {
    if (url.endsWith("/auth/me"))
      return Promise.resolve(
        loggedIn ? response(session) : new Response("", { status: 401 }),
      );
    if (url.endsWith("/auth/login")) {
      loggedIn = true;
      return Promise.resolve(response(session));
    }
    if (url.endsWith("/auth/logout")) {
      loggedIn = false;
      return Promise.resolve(new Response(null, { status: 204 }));
    }
    if (url.endsWith("/audit")) return Promise.resolve(response([]));
    if (url.endsWith("/nodes") && options?.method === "POST") {
      const input = JSON.parse(options.body as string) as { name: string };
      nodes = [{ id: "node-id", name: input.name, status: "offline" }];
      return Promise.resolve(response(nodes[0]));
    }
    if (url.endsWith("/nodes/node-id") && options?.method === "PATCH") {
      const input = JSON.parse(options.body as string) as { name: string };
      nodes = [{ ...nodes[0], name: input.name }];
      return Promise.resolve(response(nodes[0]));
    }
    if (url.endsWith("/nodes/summary"))
      return Promise.resolve(
        response(
          nodes.map((node) => ({
            node,
            metrics: { stale: true, cpu_percent: null, memory: null },
          })),
        ),
      );
    throw new Error(`Unexpected test request: ${url}`);
  });
  vi.stubGlobal("fetch", fetch);
  const client = new QueryClient();
  render(
    <QueryClientProvider client={client}>
      <AccountPanel />
    </QueryClientProvider>,
    { wrapper: MemoryRouter },
  );
  const username = await screen.findByLabelText("账号");
  fireEvent.change(username, { target: { value: "admin" } });
  fireEvent.change(screen.getByLabelText("密码"), {
    target: { value: "test-only-password" },
  });
  fireEvent.click(screen.getByRole("button", { name: /登\s*录/ }));
  expect(await screen.findByText("当前账号：admin")).toBeTruthy();
  fireEvent.change(screen.getByLabelText("节点名称"), {
    target: { value: "First node" },
  });
  fireEvent.click(screen.getByRole("button", { name: "创建节点" }));
  expect(await screen.findByText("First node")).toBeTruthy();
  const created = fetch.mock.calls.find(
    ([, options]) =>
      options?.method === "POST" &&
      (options.headers as Record<string, string>)["X-CSRF-Token"] ===
        "test-csrf",
  );
  expect(created).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: /编\s*辑/ }));
  fireEvent.change(screen.getByLabelText("节点名称"), {
    target: { value: "Renamed node" },
  });
  fireEvent.click(screen.getByRole("button", { name: "保存修改" }));
  expect(await screen.findByText("Renamed node")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "退出登录" }));
  await waitFor(() => expect(screen.queryByText("Renamed node")).toBeNull());
  expect(await screen.findByLabelText("密码")).toBeTruthy();
  expect(client.getQueryData(["nodes"])).toBeUndefined();
  client.clear();
});

it("clears the password after a failed login", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn().mockResolvedValue(new Response("", { status: 401 })),
  );
  const client = new QueryClient();
  render(
    <QueryClientProvider client={client}>
      <AccountPanel />
    </QueryClientProvider>,
    { wrapper: MemoryRouter },
  );
  fireEvent.change(await screen.findByLabelText("账号"), {
    target: { value: "admin" },
  });
  fireEvent.change(screen.getByLabelText("密码"), {
    target: { value: "wrong-password" },
  });
  fireEvent.click(screen.getByRole("button", { name: /登\s*录/ }));
  expect(await screen.findByText(/账号或密码错误/)).toBeTruthy();
  expect((screen.getByLabelText("密码") as HTMLInputElement).value).toBe("");
  expect(client.getQueryData(["session"])).toBeNull();
  client.clear();
});

it("returns to login when a protected request finds the session expired", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn((url: string) =>
      Promise.resolve(
        url.endsWith("/auth/me")
          ? response(session)
          : new Response("", { status: 401 }),
      ),
    ),
  );
  const client = new QueryClient();
  render(
    <QueryClientProvider client={client}>
      <AccountPanel />
    </QueryClientProvider>,
    { wrapper: MemoryRouter },
  );
  expect(await screen.findByLabelText("密码")).toBeTruthy();
  expect(client.getQueryData(["session"])).toBeNull();
  client.clear();
});
