import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import App from "./App";

it("shows live health data and recovers from an error on refresh", async () => {
  const fetch = vi
    .fn()
    .mockRejectedValueOnce(new Error("服务不可达"))
    .mockResolvedValue(
      new Response(JSON.stringify({ status: "ok", version: "test-version" })),
    );
  vi.stubGlobal("fetch", (url: string, ...args: unknown[]) =>
    url === "/api/v1/auth/me"
      ? Promise.resolve(new Response("", { status: 401 }))
      : fetch(url, ...args),
  );
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <App />
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
      ? Promise.resolve(new Response("", { status: 401 }))
      : fetch(url, ...args),
  );
  const client = new QueryClient();
  render(
    <QueryClientProvider client={client}>
      <App />
    </QueryClientProvider>,
  );

  expect(await screen.findByText("服务正常")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: /刷新状态/ }));
  expect(await screen.findByText("服务不可达")).toBeTruthy();
  expect(screen.queryByText("服务正常")).toBeNull();
  client.clear();
});
