import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import XrayPanel from "./XrayPanel";

it("only submits a pinned release with CSRF and displays the reported installation", async () => {
  const fetch = vi.fn((url: string) =>
    Promise.resolve(
      new Response(
        JSON.stringify({
          id: "xray-task",
          status: url.endsWith("/tasks/xray-task") ? "succeeded" : "pending",
          result: url.endsWith("/tasks/xray-task")
            ? {
                status: "xray",
                data: {
                  status: "ok",
                  data: {
                    installed: true,
                    version: "26.10.31",
                    previous_version: null,
                    active: true,
                  },
                },
              }
            : null,
        }),
      ),
    ),
  );
  vi.stubGlobal("fetch", fetch);
  const client = new QueryClient();
  render(
    <QueryClientProvider client={client}>
      <XrayPanel nodeId="test-node" csrf="test-csrf" />
    </QueryClientProvider>,
  );
  fireEvent.click(screen.getByRole("button", { name: "Xray 管理" }));
  fireEvent.click(screen.getByRole("button", { name: "安装所选版本" }));
  expect(await screen.findByText(/已安装 26\.10\.31.*运行中/)).toBeTruthy();
  const [url, options] = fetch.mock.calls[0] as unknown as [
    string,
    RequestInit,
  ];
  expect(url).toBe("/api/v1/nodes/test-node/xray/install");
  expect(options.headers).toEqual({
    "Content-Type": "application/json",
    "X-CSRF-Token": "test-csrf",
  });
  expect(JSON.parse(options.body as string).version).toBe("26.10.31");
  client.clear();
});

it("shows helper unavailability without claiming a successful install", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn((url: string) =>
      Promise.resolve(
        new Response(
          JSON.stringify({
            id: "xray-task",
            status: url.endsWith("/tasks/xray-task") ? "failed" : "pending",
            result: url.endsWith("/tasks/xray-task")
              ? {
                  status: "xray",
                  data: { status: "error", data: "helper_unavailable" },
                }
              : null,
          }),
        ),
      ),
    ),
  );
  const client = new QueryClient();
  render(
    <QueryClientProvider client={client}>
      <XrayPanel nodeId="test-node" csrf="test-csrf" />
    </QueryClientProvider>,
  );
  fireEvent.click(screen.getByRole("button", { name: "Xray 管理" }));
  fireEvent.click(screen.getByRole("button", { name: "查询安装状态" }));
  expect(await screen.findByText(/节点尚未配置受限 Xray helper/)).toBeTruthy();
  expect(screen.queryByText(/已安装 26/)).toBeNull();
  client.clear();
});
