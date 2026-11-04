import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import NodeMetricsPanel from "./NodeMetricsPanel";

function mount() {
  const client = new QueryClient();
  render(
    <QueryClientProvider client={client}>
      <NodeMetricsPanel nodeId="test-node" />
    </QueryClientProvider>,
  );
  return client;
}

it("requests resources only when opened and distinguishes first samples from zero", async () => {
  const fetch = vi.fn().mockResolvedValue(
    new Response(
      JSON.stringify({
        node_id: "test-node",
        stale: false,
        received_at: 100,
        snapshot: {
          sampled_at: 100,
          cpu_percent: null,
          memory: {
            total_bytes: "1073741824",
            used_bytes: "536870912",
            available_bytes: "536870912",
          },
          disk: null,
          network: {
            received_bytes: "1000",
            transmitted_bytes: "2000",
            received_bytes_per_second: null,
            transmitted_bytes_per_second: null,
          },
          issues: [],
        },
      }),
    ),
  );
  vi.stubGlobal("fetch", fetch);
  const client = mount();
  expect(fetch).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "查看资源" }));
  expect(await screen.findByText("首轮采样中")).toBeTruthy();
  expect(screen.getByText("512.00 MiB / 1.00 GiB")).toBeTruthy();
  expect(screen.queryByText("0.0%")).toBeNull();
  expect(fetch).toHaveBeenCalledWith(
    "/api/v1/nodes/test-node/metrics",
    expect.objectContaining({ credentials: "same-origin", cache: "no-store" }),
  );
  fireEvent.click(screen.getByRole("button", { name: "关闭资源" }));
  expect(screen.queryByText("512.00 MiB / 1.00 GiB")).toBeNull();
  client.clear();
});

it("labels stale metrics and unsupported platforms explicitly", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          node_id: "test-node",
          stale: true,
          received_at: 100,
          snapshot: {
            sampled_at: 100,
            cpu_percent: null,
            memory: null,
            disk: null,
            network: null,
            issues: [
              { metric: "cpu", reason: "unsupported_platform" },
              { metric: "memory", reason: "read_failed" },
            ],
          },
        }),
      ),
    ),
  );
  const client = mount();
  fireEvent.click(screen.getByRole("button", { name: "查看资源" }));
  expect(
    await screen.findByText("Agent 离线或采样已过期，以下为历史数据"),
  ).toBeTruthy();
  expect(screen.getByText("当前平台不支持 Linux 采集")).toBeTruthy();
  expect(screen.getByText("内存：读取失败")).toBeTruthy();
  expect(screen.queryByText("0.0%")).toBeNull();
  client.clear();
});

it("shows an absent snapshot and expires the session on unauthorized access", async () => {
  vi.stubGlobal(
    "fetch",
    vi
      .fn()
      .mockResolvedValueOnce(
        new Response(
          JSON.stringify({
            node_id: "test-node",
            snapshot: null,
            stale: true,
            received_at: null,
          }),
        ),
      )
      .mockResolvedValue(new Response("", { status: 401 })),
  );
  const client = mount();
  client.setQueryData(["session"], { admin: { username: "test" } });
  fireEvent.click(screen.getByRole("button", { name: "查看资源" }));
  expect(await screen.findByText("等待 Agent 首次采样")).toBeTruthy();
  await client.refetchQueries({ queryKey: ["metrics", "test-node"] });
  expect(await screen.findByText(/账号或密码错误/)).toBeTruthy();
  expect(client.getQueryData(["session"])).toBeNull();
  client.clear();
});
