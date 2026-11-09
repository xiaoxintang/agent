import type {
  AuditEntry,
  LoginRequest,
  Node,
  NodeSummary,
  NodeInput,
  SessionResponse,
  EnrollmentResponse,
  TaskRecord,
  ManagedService,
  NodeMetrics,
  XrayVersion,
  InboundInput,
  InboundView,
  ClientInput,
  ClientView,
  CreatedClient,
  RevisionView,
  NodeTraffic,
} from "../types/generated";

export class ApiError extends Error {
  constructor(
    public readonly status: number,
    message: string,
    public readonly code?: string,
    public readonly requestId?: string,
  ) {
    super(requestId ? `${message}（请求 ID：${requestId}）` : message);
  }
}

function randomUuid(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  bytes[6] = (bytes[6] & 0x0f) | 0x40;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  const hex = Array.from(bytes, (byte) =>
    byte.toString(16).padStart(2, "0"),
  ).join("");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

async function request<T>(
  path: string,
  method = "GET",
  body?: unknown,
  csrf?: string,
  signal?: AbortSignal,
): Promise<T> {
  const response = await fetch(`/api/v1${path}`, {
    method,
    credentials: "same-origin",
    cache: "no-store",
    ...(signal ? { signal } : {}),
    headers: {
      ...(body === undefined ? {} : { "Content-Type": "application/json" }),
      ...(csrf ? { "X-CSRF-Token": csrf } : {}),
    },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  });
  if (!response.ok) {
    let code: string | undefined;
    try {
      const data: unknown = await response.json();
      if (
        typeof data === "object" &&
        data !== null &&
        "error" in data &&
        typeof data.error === "object" &&
        data.error !== null &&
        "code" in data.error &&
        typeof data.error.code === "string"
      )
        code = data.error.code;
    } catch {
      // Proxies and unavailable servers can return a non-JSON error.
    }
    const codes: Record<string, string> = {
      unauthorized: "账号或密码错误，或会话已过期",
      not_found: "记录不存在或已被撤销",
      method_not_allowed: "当前接口不支持此操作",
      invalid_input: "输入内容不正确，请检查后重试",
      invalid_json: "请求内容格式不正确，或超过大小限制",
      csrf_failed: "安全校验失败，请刷新页面后重试",
      node_limit: "最多支持 100 个有效节点",
      inbound_limit: "该节点的入站数量已达上限",
      client_limit: "客户端数量已达上限",
      task_queue_full: "该节点任务队列已满，请等待已有任务完成",
      idempotency_conflict: "此请求已用于其他操作，请刷新状态后重试",
      conflict: "记录与现有配置冲突，请检查端口等设置",
      tls_required: "Agent 连接需要启用 HTTPS",
      internal_error: "服务处理失败，请使用请求 ID 查看服务端日志",
      rate_limited: "请求过于频繁，请稍后重试",
    };
    const messages: Record<number, string> = {
      400: "输入内容不正确，请检查后重试",
      401: "账号或密码错误，或会话已过期",
      403: "请求来源或安全校验失败，请刷新页面后重试",
      404: "记录不存在或已被撤销",
      409: "操作冲突或达到当前数量上限",
      429: "请求过于频繁，请稍后重试",
    };
    throw new ApiError(
      response.status,
      (code && Object.hasOwn(codes, code) && codes[code]) ||
        messages[response.status] ||
        "请求失败，请稍后重试",
      code && Object.hasOwn(codes, code) ? code : undefined,
      response.headers.get("X-Request-ID") ?? undefined,
    );
  }
  if (response.status === 204) return undefined as T;
  return response.json() as Promise<T>;
}

export async function getSession(): Promise<SessionResponse | null> {
  try {
    return await request<SessionResponse>("/auth/me");
  } catch (error) {
    if (error instanceof ApiError && error.status === 401) return null;
    throw error;
  }
}
export const login = (input: LoginRequest) =>
  request<SessionResponse>("/auth/login", "POST", input);
export const logout = (csrf: string) =>
  request<void>("/auth/logout", "POST", undefined, csrf);
export const getNodeSummaries = (signal?: AbortSignal) =>
  request<NodeSummary[]>("/nodes/summary", "GET", undefined, undefined, signal);
export const getNode = (id: string, signal?: AbortSignal) =>
  request<Node>(
    `/nodes/${encodeURIComponent(id)}`,
    "GET",
    undefined,
    undefined,
    signal,
  );
export const getLocalNode = () =>
  request<{ node_id: string | null }>("/local-node");
export const setLocalNode = (id: string, csrf: string) =>
  request<{ node_id: string }>(
    `/local-node/${encodeURIComponent(id)}`,
    "PUT",
    undefined,
    csrf,
  );
export const getAudit = () => request<AuditEntry[]>("/audit");
export const createNode = (input: NodeInput, csrf: string) =>
  request<Node>("/nodes", "POST", input, csrf);
export const updateNode = (id: string, input: NodeInput, csrf: string) =>
  request<Node>(`/nodes/${encodeURIComponent(id)}`, "PATCH", input, csrf);
export const deleteNode = (id: string, csrf: string) =>
  request<void>(`/nodes/${encodeURIComponent(id)}`, "DELETE", undefined, csrf);
export const issueEnrollment = (node_id: string, csrf: string) =>
  request<EnrollmentResponse>("/nodes/enrollments", "POST", { node_id }, csrf);
export const createStatusTask = (node_id: string, csrf: string) =>
  request<TaskRecord>(
    `/nodes/${encodeURIComponent(node_id)}/tasks`,
    "POST",
    { operation: "agent_status", idempotency_key: randomUuid() },
    csrf,
  );
export const getTask = (id: string) =>
  request<TaskRecord>(`/tasks/${encodeURIComponent(id)}`);
export const getMetrics = (id: string) =>
  request<NodeMetrics>(`/nodes/${encodeURIComponent(id)}/metrics`);
export const createXrayTask = (
  nodeId: string,
  action:
    | "status"
    | "install"
    | "upgrade"
    | "start"
    | "stop"
    | "restart"
    | "rollback"
    | "uninstall",
  csrf: string,
  version?: XrayVersion,
) =>
  request<TaskRecord>(
    `/nodes/${encodeURIComponent(nodeId)}/xray/${action}`,
    "POST",
    { idempotency_key: randomUuid(), ...(version ? { version } : {}) },
    csrf,
  );
export const createServiceStatusTask = (
  nodeId: string,
  service: ManagedService,
  csrf: string,
) =>
  request<TaskRecord>(
    `/nodes/${encodeURIComponent(nodeId)}/tasks`,
    "POST",
    {
      operation: { service_status: { service } },
      idempotency_key: randomUuid(),
    },
    csrf,
  );

export const getInbounds = (node: string) =>
  request<InboundView[]>(`/nodes/${encodeURIComponent(node)}/inbounds`);
export const saveInbound = (
  node: string,
  id: string | null,
  input: InboundInput,
  csrf: string,
) =>
  request<InboundView>(
    id
      ? `/inbounds/${encodeURIComponent(id)}`
      : `/nodes/${encodeURIComponent(node)}/inbounds`,
    id ? "PATCH" : "POST",
    input,
    csrf,
  );
export const removeInbound = (id: string, csrf: string) =>
  request<void>(
    `/inbounds/${encodeURIComponent(id)}`,
    "DELETE",
    undefined,
    csrf,
  );
export const getClients = () => request<ClientView[]>("/clients");
export const createClient = (input: ClientInput, csrf: string) =>
  request<CreatedClient>("/clients", "POST", input, csrf);
export const updateClient = (id: string, input: ClientInput, csrf: string) =>
  request<ClientView>(
    `/clients/${encodeURIComponent(id)}`,
    "PATCH",
    input,
    csrf,
  );
export const removeClient = (id: string, csrf: string) =>
  request<void>(
    `/clients/${encodeURIComponent(id)}`,
    "DELETE",
    undefined,
    csrf,
  );
export const assignClient = (
  inbound: string,
  client: string,
  enabled: boolean,
  csrf: string,
) =>
  request<void>(
    `/inbounds/${encodeURIComponent(inbound)}/clients/${encodeURIComponent(client)}`,
    enabled ? "POST" : "DELETE",
    undefined,
    csrf,
  );
export const getRevisions = (node: string) =>
  request<RevisionView[]>(
    `/nodes/${encodeURIComponent(node)}/config/revisions`,
  );
export const applyConfig = (node: string, csrf: string) =>
  request<RevisionView>(
    `/nodes/${encodeURIComponent(node)}/config/apply`,
    "POST",
    { idempotency_key: randomUuid() },
    csrf,
  );

export const getTraffic = (node: string) =>
  request<NodeTraffic>(`/nodes/${encodeURIComponent(node)}/traffic`);
