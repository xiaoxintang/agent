import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import TrafficPanel from "./TrafficPanel";
it("keeps stale historical traffic visible and renders the bounded hourly chart", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn(() =>
      Promise.resolve(
        new Response(
          JSON.stringify({
            available: false,
            stale: true,
            received_at: 1700000000,
            buckets: [
              {
                bucket_start: 1700000000,
                uplink_bytes: "1048576",
                downlink_bytes: "2097152",
              },
            ],
            clients: [
              {
                client_id: "client-1",
                display_name: "alice",
                uplink_bytes: "1048576",
                downlink_bytes: "2097152",
              },
            ],
          }),
        ),
      ),
    ),
  );
  const client = new QueryClient();
  render(
    <QueryClientProvider client={client}>
      <TrafficPanel nodeId="node-1" />
    </QueryClientProvider>,
  );
  fireEvent.click(screen.getByRole("button", { name: "流量统计" }));
  expect(await screen.findByText(/统计不可用或已过期/)).toBeTruthy();
  expect(
    await screen.findByRole("img", { name: "最近 7 天小时流量图" }),
  ).toBeTruthy();
  expect(await screen.findByText("alice")).toBeTruthy();
  expect(screen.getByText("1.00 MiB")).toBeTruthy();
  expect(screen.getByText("2.00 MiB")).toBeTruthy();
  client.clear();
});
