import { describe, expect, it, vi } from "vitest";
import { fetchHealth } from "./health";

describe("health contract", () => {
  it("reads the real endpoint without caching", async () => {
    const fetch = vi
      .fn()
      .mockResolvedValue(
        new Response(JSON.stringify({ status: "ok", version: "0.1.0" })),
      );
    vi.stubGlobal("fetch", fetch);
    const signal = new AbortController().signal;
    await expect(fetchHealth(signal)).resolves.toEqual({
      status: "ok",
      version: "0.1.0",
    });
    expect(fetch).toHaveBeenCalledWith("/api/v1/health", {
      signal,
      cache: "no-store",
    });
  });

  it("rejects HTTP errors", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(new Response("", { status: 503 })),
    );
    await expect(fetchHealth()).rejects.toThrow("HTTP 503");
  });

  it("rejects an incompatible response", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(new Response('{"status":"ok"}')),
    );
    await expect(fetchHealth()).rejects.toThrow("响应格式不正确");
  });
});
