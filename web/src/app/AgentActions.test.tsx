import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { expect, it, vi } from "vitest";
import AgentActions from "./AgentActions";

function mount() {
  const client = new QueryClient();
  render(
    <QueryClientProvider client={client}>
      <AgentActions nodeId="test-node" csrf="test-csrf" />
    </QueryClientProvider>,
  );
  return client;
}

it("shows enrollment once without caching the token and clears it on close", async () => {
  const token = "a".repeat(64);
  const fetch = vi
    .fn()
    .mockResolvedValue(
      new Response(
        JSON.stringify({ node_id: "test-node", token, expires_at: 9999999999 }),
      ),
    );
  vi.stubGlobal("fetch", fetch);
  const client = mount();
  fireEvent.click(screen.getByRole("button", { name: "注册 Agent" }));
  expect(await screen.findByText(token)).toBeTruthy();
  expect(fetch).toHaveBeenCalledWith(
    "/api/v1/nodes/enrollments",
    expect.objectContaining({
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        "X-CSRF-Token": "test-csrf",
      },
      body: '{"node_id":"test-node"}',
    }),
  );
  expect(
    JSON.stringify(
      client
        .getQueryCache()
        .getAll()
        .map((query) => query.state.data),
    ),
  ).not.toContain(token);
  expect(client.getMutationCache().getAll()).toHaveLength(0);
  fireEvent.click(screen.getByRole("button", { name: "关闭并清除令牌" }));
  await waitFor(() => expect(screen.queryByText(token)).toBeNull());
  client.clear();
});

it("discards a late enrollment response after the modal closes", async () => {
  let finish!: (response: Response) => void;
  vi.stubGlobal(
    "fetch",
    vi.fn(
      () =>
        new Promise<Response>((resolve) => {
          finish = resolve;
        }),
    ),
  );
  const client = mount();
  fireEvent.click(screen.getByRole("button", { name: "注册 Agent" }));
  fireEvent.click(
    await screen.findByRole("button", { name: "关闭并清除令牌" }),
  );
  const token = "b".repeat(64);
  await act(async () => {
    finish(
      new Response(
        JSON.stringify({ node_id: "test-node", token, expires_at: 9999999999 }),
      ),
    );
  });
  expect(screen.queryByText(token)).toBeNull();
  client.clear();
});

it("submits a constrained status task and displays its actual result", async () => {
  const pending = {
    id: "test-task",
    node_id: "test-node",
    operation: "agent_status",
    status: "pending",
    result: null,
  };
  const fetch = vi.fn((url: string) =>
    Promise.resolve(
      new Response(
        JSON.stringify(
          url.endsWith("/tasks/test-task")
            ? {
                ...pending,
                status: "succeeded",
                result: {
                  status: "succeeded",
                  data: { version: "0.1.0", os: "linux", arch: "aarch64" },
                },
              }
            : pending,
        ),
      ),
    ),
  );
  vi.stubGlobal("fetch", fetch);
  const client = mount();
  fireEvent.click(screen.getByRole("button", { name: "查询 Agent 状态" }));
  expect(
    await screen.findByText(/查询成功.*0\.1\.0.*linux.*aarch64/),
  ).toBeTruthy();
  const input = JSON.parse(
    (fetch.mock.calls[0] as unknown as [string, RequestInit])[1].body as string,
  ) as { operation: string; idempotency_key: string };
  expect(input.operation).toBe("agent_status");
  expect(input.idempotency_key).toMatch(/^[0-9a-f-]{36}$/);
  client.clear();
});

it("does not refill the task cache from a response after unmount", async () => {
  let finish!: (response: Response) => void;
  vi.stubGlobal(
    "fetch",
    vi.fn(
      () =>
        new Promise<Response>((resolve) => {
          finish = resolve;
        }),
    ),
  );
  const client = new QueryClient();
  const view = render(
    <QueryClientProvider client={client}>
      <AgentActions nodeId="test-node" csrf="test-csrf" />
    </QueryClientProvider>,
  );
  fireEvent.click(screen.getByRole("button", { name: "查询 Agent 状态" }));
  view.unmount();
  await act(async () => {
    finish(
      new Response(JSON.stringify({ id: "late-task", status: "pending" })),
    );
  });
  expect(client.getQueryData(["task", "late-task"])).toBeUndefined();
  client.clear();
});

it("only offers fixed read-only systemd services and renders the reported state", async () => {
  const fetch = vi.fn((url: string) =>
    Promise.resolve(
      new Response(
        JSON.stringify({
          id: "service-task",
          status: url.endsWith("/tasks/service-task") ? "succeeded" : "pending",
          result: url.endsWith("/tasks/service-task")
            ? {
                status: "service_status",
                data: {
                  service: "xray",
                  load_state: "not_loaded",
                  active_state: null,
                  sub_state: null,
                },
              }
            : null,
        }),
      ),
    ),
  );
  vi.stubGlobal("fetch", fetch);
  const client = mount();
  fireEvent.click(screen.getByRole("button", { name: "查询 Xray 服务" }));
  expect(
    await screen.findByText(/xray\.service.*未加载（不代表未安装）/),
  ).toBeTruthy();
  const [, options] = fetch.mock.calls[0] as unknown as [string, RequestInit];
  expect(JSON.parse(options.body as string).operation).toEqual({
    service_status: { service: "xray" },
  });
  expect(screen.queryByRole("button", { name: /重启|启动|停止/ })).toBeNull();
  client.clear();
});
