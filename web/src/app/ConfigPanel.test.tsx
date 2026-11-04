import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import ConfigPanel from "./ConfigPanel";

it("submits typed configuration and keeps the one-time client UUID out of query caches", async () => {
  const inbound = {
    id: "inbound-1",
    name: "edge",
    listen_address: "127.0.0.1",
    listen_port: 38443,
    public_key: "public-only",
    client_ids: [],
    enabled: true,
  };
  let saved = false;
  const fetch = vi.fn((url: string, options: RequestInit) => {
    const response =
      options.method === "POST" && url.endsWith("/clients")
        ? {
            client: { id: "client-1", display_name: "alice", enabled: true },
            protocol_id: "one-time-secret",
          }
        : url.endsWith("/inbounds")
          ? options.method === "POST"
            ? ((saved = true), inbound)
            : saved
              ? [inbound]
              : []
          : [];
    return Promise.resolve(new Response(JSON.stringify(response)));
  });
  vi.stubGlobal("fetch", fetch);
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <ConfigPanel nodeId="node-1" csrf="csrf-1" />
    </QueryClientProvider>,
  );
  fireEvent.click(screen.getByRole("button", { name: "入站与客户端" }));
  fireEvent.change(screen.getByLabelText("入站名称"), {
    target: { value: "edge" },
  });
  fireEvent.change(screen.getByLabelText("REALITY 目标（域名:端口）"), {
    target: { value: "www.example.com:443" },
  });
  fireEvent.change(screen.getByLabelText("SNI 域名（逗号分隔）"), {
    target: { value: "www.example.com" },
  });
  fireEvent.change(
    screen.getByLabelText("Short ID（偶数位十六进制，逗号分隔）"),
    { target: { value: "abcd" } },
  );
  fireEvent.click(screen.getByRole("button", { name: "保存入站" }));
  expect(await screen.findByText(/公钥：public-only/)).toBeTruthy();
  const call = fetch.mock.calls.find(
    ([, options]) => options.method === "POST",
  );
  expect(call?.[1].headers).toMatchObject({ "X-CSRF-Token": "csrf-1" });
  expect(JSON.parse(call?.[1].body as string)).toMatchObject({
    name: "edge",
    server_names: ["www.example.com"],
    short_ids: ["abcd"],
  });
  fireEvent.change(screen.getByLabelText("客户端名称"), {
    target: { value: "alice" },
  });
  fireEvent.click(screen.getByRole("button", { name: "创建客户端" }));
  expect(await screen.findByText("one-time-secret")).toBeTruthy();
  expect(
    JSON.stringify(
      client
        .getQueryCache()
        .getAll()
        .map((query) => query.state.data),
    ),
  ).not.toContain("one-time-secret");
  fireEvent.click(screen.getByRole("button", { name: "关闭配置" }));
  await waitFor(() => expect(screen.queryByText("one-time-secret")).toBeNull());
  client.clear();
});
