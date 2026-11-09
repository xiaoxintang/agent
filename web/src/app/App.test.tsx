import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { MemoryRouter } from "react-router-dom";
import App from "./App";

const session = {
  admin: { id: "admin", username: "admin" },
  csrf_token: "csrf",
  expires_at: 9999999999,
};

it("shows live health data and recovers from an error on refresh", async () => {
  const fetch = vi
    .fn()
    .mockRejectedValueOnce(new Error("服务不可达"))
    .mockResolvedValue(
      new Response(JSON.stringify({ status: "ok", version: "test-version" })),
    );
  vi.stubGlobal("fetch", (url: string, ...args: unknown[]) =>
    url === "/api/v1/auth/me"
      ? Promise.resolve(new Response(JSON.stringify(session)))
      : url === "/api/v1/local-node"
        ? Promise.resolve(new Response(JSON.stringify({ node_id: null })))
        : fetch(url, ...args),
  );
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={["/overview"]}>
        <App />
      </MemoryRouter>
    </QueryClientProvider>,
  );

  expect(await screen.findByText("服务不可达")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: /刷新状态/ }));
  expect(await screen.findByText("服务正常")).toBeTruthy();
  expect(screen.getByText("版本 test-version")).toBeTruthy();
  expect(screen.queryByText("服务不可达")).toBeNull();
  client.clear();
});

it("does not present stale healthy data after a failed refresh", async () => {
  const fetch = vi
    .fn()
    .mockResolvedValueOnce(
      new Response(JSON.stringify({ status: "ok", version: "test-version" })),
    )
    .mockRejectedValue(new Error("服务不可达"));
  vi.stubGlobal("fetch", (url: string, ...args: unknown[]) =>
    url === "/api/v1/auth/me"
      ? Promise.resolve(new Response(JSON.stringify(session)))
      : url === "/api/v1/local-node"
        ? Promise.resolve(new Response(JSON.stringify({ node_id: null })))
        : fetch(url, ...args),
  );
  const client = new QueryClient();
  render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={["/overview"]}>
        <App />
      </MemoryRouter>
    </QueryClientProvider>,
  );

  expect(await screen.findByText("服务正常")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: /刷新状态/ }));
  expect(await screen.findByText("服务不可达")).toBeTruthy();
  expect(screen.queryByText("服务正常")).toBeNull();
  client.clear();
});

it("requires login before showing a protected page", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn().mockResolvedValue(new Response("", { status: 401 })),
  );
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={["/nodes"]}>
        <App />
      </MemoryRouter>
    </QueryClientProvider>,
  );
  expect(await screen.findByLabelText("密码")).toBeTruthy();
  expect(screen.queryByText("节点管理")).toBeNull();
  client.clear();
});

it("shows a setup link when no local node is associated", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn((url: string) =>
      Promise.resolve(
        new Response(
          JSON.stringify(
            url.endsWith("/auth/me") ? session : { node_id: null },
          ),
        ),
      ),
    ),
  );
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={["/resources"]}>
        <App />
      </MemoryRouter>
    </QueryClientProvider>,
  );
  expect(await screen.findByText("尚未设置本机节点")).toBeTruthy();
  expect(
    screen
      .getByRole("link", { name: "前往节点管理设置本机" })
      .getAttribute("href"),
  ).toBe("/nodes");
  client.clear();
});

it("navigates from overview to the local resources page", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn((url: string) =>
      Promise.resolve(
        new Response(
          JSON.stringify(
            url.endsWith("/auth/me")
              ? session
              : url.endsWith("/local-node")
                ? { node_id: null }
                : { status: "ok", version: "test-version" },
          ),
        ),
      ),
    ),
  );
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={["/overview"]}>
        <App />
      </MemoryRouter>
    </QueryClientProvider>,
  );
  expect(await screen.findByText("服务正常")).toBeTruthy();
  fireEvent.click(screen.getByRole("link", { name: "本机资源" }));
  expect(await screen.findByText("尚未设置本机节点")).toBeTruthy();
  expect(screen.queryByText("服务正常")).toBeNull();
  client.clear();
});

it("manages the selected remote node without changing the local node", async () => {
  const remote = "5b56579d-89b0-40c7-9187-bf7fe4158c4f";
  const fetch = vi.fn((url: string, options?: RequestInit) => {
    if (url.endsWith("/auth/me"))
      return Promise.resolve(new Response(JSON.stringify(session)));
    if (url.endsWith("/local-node"))
      return Promise.resolve(
        new Response(JSON.stringify({ node_id: "local-node" })),
      );
    if (url.endsWith(`/nodes/${remote}`))
      return Promise.resolve(
        new Response(
          JSON.stringify({ id: remote, name: "远程节点", status: "online" }),
        ),
      );
    if (url.endsWith("/xray/status") && options?.method === "POST")
      return Promise.resolve(
        new Response(JSON.stringify({ id: "task-remote" })),
      );
    if (url.endsWith("/tasks/task-remote"))
      return Promise.resolve(
        new Response(JSON.stringify({ status: "succeeded" })),
      );
    if (
      url.endsWith(`/nodes/${remote}/inbounds`) ||
      url.endsWith("/clients") ||
      url.endsWith("/config/revisions")
    )
      return Promise.resolve(new Response("[]"));
    throw new Error(`Unexpected test request: ${url}`);
  });
  vi.stubGlobal("fetch", fetch);
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[`/nodes/${remote}/xray`]}>
        <App />
      </MemoryRouter>
    </QueryClientProvider>,
  );
  expect(await screen.findByText("当前节点：远程节点")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "查询安装状态" }));
  await waitFor(() =>
    expect(
      fetch.mock.calls.some(
        ([url, options]) =>
          url === `/api/v1/nodes/${remote}/xray/status` &&
          options?.method === "POST" &&
          (options.headers as Record<string, string>)["X-CSRF-Token"] ===
            "csrf",
      ),
    ).toBe(true),
  );
  const configuration = screen
    .getAllByRole("link", { name: "配置管理" })
    .find((link) =>
      link.getAttribute("href")?.startsWith(`/nodes/${remote}/`),
    )!;
  fireEvent.click(configuration);
  expect(await screen.findByText("VLESS + REALITY 配置")).toBeTruthy();
  expect(
    fetch.mock.calls.some(
      ([url]) => url === `/api/v1/nodes/${remote}/inbounds`,
    ),
  ).toBe(true);
  expect(
    fetch.mock.calls.some(([url]) => url.includes("/nodes/local-node/")),
  ).toBe(false);
  expect(
    fetch.mock.calls.some(([, options]) => options?.method === "PUT"),
  ).toBe(false);
  client.clear();
});
