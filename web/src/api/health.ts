import type { HealthResponse } from "../types/generated";

export async function fetchHealth(
  signal?: AbortSignal,
): Promise<HealthResponse> {
  const response = await fetch("/api/v1/health", { signal, cache: "no-store" });
  if (!response.ok) {
    throw new Error(`健康检查失败（HTTP ${response.status}）`);
  }
  const data: unknown = await response.json();
  if (
    typeof data !== "object" ||
    data === null ||
    !("status" in data) ||
    data.status !== "ok" ||
    !("version" in data) ||
    typeof data.version !== "string"
  ) {
    throw new Error("健康检查响应格式不正确");
  }
  return { status: data.status, version: data.version };
}
