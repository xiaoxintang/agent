import type {
  AuditEntry,
  LoginRequest,
  Node,
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
  ) {
    super(message);
  }
}

async function request<T>(
  path: string,
  method = "GET",
  body?: unknown,
  csrf?: string,
): Promise<T> {
  const response = await fetch(`/api/v1${path}`, {
    method,
    credentials: "same-origin",
    cache: "no-store",
    headers: {
      ...(body === undefined ? {} : { "Content-Type": "application/json" }),
      ...(csrf ? { "X-CSRF-Token": csrf } : {}),
    },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  });
  if (!response.ok) {
    const messages: Record<number, string> = {
      400: "输入内容不正确，请检查后重试",
      401: "账号或密码错误，或会话已过期",
      403: "请求来源或安全校验失败，请刷新页面后重试",
      404: "记录不存在或已被撤销",
      409: "操作冲突或达到当前数量上限",
      429: "登录尝试过于频繁，请稍后重试",
    };
    throw new ApiError(
      response.status,
      messages[response.status] ?? "请求失败，请稍后重试",
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
export const getNodes = () => request<Node[]>("/nodes");
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
    { operation: "agent_status", idempotency_key: crypto.randomUUID() },
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
    { idempotency_key: crypto.randomUUID(), ...(version ? { version } : {}) },
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
      idempotency_key: crypto.randomUUID(),
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
    { idempotency_key: crypto.randomUUID() },
    csrf,
  );

export const getTraffic = (node: string) =>
  request<NodeTraffic>(`/nodes/${encodeURIComponent(node)}/traffic`);
