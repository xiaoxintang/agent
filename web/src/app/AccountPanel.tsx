import { useEffect, useState } from "react";
import {
  useMutation,
  useQueries,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import {
  Alert,
  Button,
  Card,
  Flex,
  Form,
  Input,
  Popconfirm,
  Space,
  Spin,
  Table,
  Typography,
} from "antd";
import {
  ApiError,
  createNode,
  deleteNode,
  getAudit,
  getNodes,
  getMetrics,
  getLocalNode,
  setLocalNode,
  getSession,
  login,
  logout,
  updateNode,
} from "../api/panel";
import type {
  AuditEntry,
  LoginRequest,
  Node,
  NodeInput,
  SessionResponse,
} from "../types/generated";
import AgentActions from "./AgentActions";

export function LoginForm() {
  const client = useQueryClient();
  const [form] = Form.useForm<LoginRequest>();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  async function submit(input: LoginRequest) {
    setBusy(true);
    setError(undefined);
    try {
      client.setQueryData(["session"], await login(input));
    } catch (error) {
      setError(error instanceof Error ? error.message : "登录失败");
    } finally {
      form.resetFields(["password"]);
      setBusy(false);
    }
  }
  return (
    <Card title="管理员登录" className="section">
      {error && <Alert type="error" title={error} showIcon />}
      <Form
        form={form}
        layout="vertical"
        onFinish={(input) => void submit(input)}
      >
        <Form.Item
          label="账号"
          name="username"
          rules={[{ required: true, message: "请输入账号" }]}
        >
          <Input autoComplete="username" maxLength={64} />
        </Form.Item>
        <Form.Item
          label="密码"
          name="password"
          rules={[{ required: true, message: "请输入密码" }]}
        >
          <Input.Password autoComplete="current-password" maxLength={128} />
        </Form.Item>
        <Button type="primary" htmlType="submit" loading={busy}>
          登录
        </Button>
      </Form>
    </Card>
  );
}

const actions: Record<string, string> = {
  "admin.bootstrap": "初始化管理员",
  "auth.login": "登录",
  "auth.logout": "退出登录",
  "node.create": "创建节点",
  "node.update": "更新节点",
  "node.revoke": "撤销节点",
  "node.local.set": "设置本机节点",
  "node.enrollment.issue": "生成 Agent 注册令牌",
  "agent.enroll": "Agent 注册",
  "task.create": "创建 Agent 任务",
  "task.reject": "拒绝任务请求",
  "service.status": "查询受管服务状态",
  "xray.operation": "Xray 生命周期操作",
};

export function Management({
  session,
  page = "all",
}: {
  session: SessionResponse;
  page?: "nodes" | "audit" | "all";
}) {
  const client = useQueryClient();
  const nodes = useQuery({
    queryKey: ["nodes"],
    queryFn: getNodes,
    enabled: page !== "audit",
    retry: false,
    refetchInterval: 5000,
  });
  const audit = useQuery({
    queryKey: ["audit"],
    queryFn: getAudit,
    enabled: page !== "nodes",
    retry: false,
  });
  const local = useQuery({
    queryKey: ["local-node"],
    queryFn: getLocalNode,
    enabled: page === "nodes",
    retry: false,
  });
  const metrics = useQueries({
    queries: (page === "nodes" ? (nodes.data ?? []) : []).map((node) => ({
      queryKey: ["metrics", node.id],
      queryFn: () => getMetrics(node.id),
      retry: false,
      refetchInterval: 5000,
    })),
  });
  const markLocal = useMutation({
    mutationFn: (id: string) => setLocalNode(id, session.csrf_token),
    onSuccess: () =>
      void client.invalidateQueries({ queryKey: ["local-node"] }),
  });
  const [form] = Form.useForm<NodeInput>();
  const [editing, setEditing] = useState<string | null>(null);
  const refresh = () => {
    void client.invalidateQueries({ queryKey: ["nodes"] });
    void client.invalidateQueries({ queryKey: ["audit"] });
    void client.invalidateQueries({ queryKey: ["local-node"] });
  };
  const save = useMutation({
    mutationFn: (input: NodeInput) =>
      editing
        ? updateNode(editing, input, session.csrf_token)
        : createNode(input, session.csrf_token),
    onSuccess: () => {
      setEditing(null);
      form.resetFields();
      refresh();
    },
  });
  const remove = useMutation({
    mutationFn: (id: string) => deleteNode(id, session.csrf_token),
    onSuccess: refresh,
  });
  const exit = useMutation({
    mutationFn: () => logout(session.csrf_token),
    onSuccess: () => {
      client.setQueryData(["session"], null);
    },
  });
  const errors = [
    nodes.error,
    audit.error,
    save.error,
    remove.error,
    exit.error,
    local.error,
    markLocal.error,
    ...metrics
      .map((result) => result.error)
      .filter((error) => error instanceof ApiError && error.status === 401),
  ];
  const expired = errors.some(
    (error) => error instanceof ApiError && error.status === 401,
  );
  useEffect(() => {
    if (expired) {
      client.setQueryData(["session"], null);
    }
  }, [client, expired]);

  return (
    <>
      {page === "all" && (
        <Flex
          className="section"
          justify="space-between"
          align="center"
          gap="small"
          wrap
        >
          <Typography.Text>当前账号：{session.admin.username}</Typography.Text>
          <Button onClick={() => exit.mutate()} loading={exit.isPending}>
            退出登录
          </Button>
        </Flex>
      )}
      {errors
        .filter((error): error is Error => error instanceof Error)
        .map((error, index) => (
          <Alert
            key={index}
            className="section"
            type="error"
            title={error.message}
            showIcon
          />
        ))}
      {page !== "audit" && (
        <Card
          title="节点管理"
          className="section"
          extra={<Button onClick={refresh}>刷新列表</Button>}
        >
          <Typography.Paragraph>
            为节点生成一次性注册令牌，接入 Agent 后查看在线状态和进程信息。
          </Typography.Paragraph>
          {nodes.isSuccess && (
            <Typography.Paragraph>
              节点 {nodes.data.length} · 在线{" "}
              {nodes.data.filter((node) => node.status === "online").length}
            </Typography.Paragraph>
          )}
          <Form
            form={form}
            layout="vertical"
            onFinish={(input) => save.mutate(input)}
          >
            <Form.Item
              label="节点名称"
              name="name"
              rules={[
                {
                  required: true,
                  whitespace: true,
                  max: 64,
                  message: "请输入 1–64 个字符的节点名称",
                },
              ]}
            >
              <Input maxLength={64} />
            </Form.Item>
            <Space>
              <Button type="primary" htmlType="submit" loading={save.isPending}>
                {editing ? "保存修改" : "创建节点"}
              </Button>
              {editing && (
                <Button
                  onClick={() => {
                    setEditing(null);
                    form.resetFields();
                  }}
                >
                  取消编辑
                </Button>
              )}
            </Space>
          </Form>
          <Table<Node>
            className="section"
            rowKey="id"
            loading={nodes.isPending}
            dataSource={nodes.data ?? []}
            size="small"
            scroll={{ x: 800 }}
            columns={[
              { title: "名称", dataIndex: "name" },
              {
                title: "状态",
                dataIndex: "status",
                render: (value: string) =>
                  value === "offline"
                    ? "离线"
                    : value === "online"
                      ? "在线"
                      : value,
              },
              {
                title: "CPU",
                key: "cpu",
                render: (_, node) => {
                  const result =
                    metrics[
                      (nodes.data ?? []).findIndex(
                        (item) => item.id === node.id,
                      )
                    ];
                  if (!result) return "—";
                  if (result.isError) return "获取失败";
                  if (
                    !result.data?.snapshot ||
                    result.data.snapshot.cpu_percent == null
                  )
                    return "暂无数据";
                  return `${result.data.snapshot.cpu_percent.toFixed(1)}%${result.data.stale ? "（过期）" : ""}`;
                },
              },
              {
                title: "内存",
                key: "memory",
                render: (_, node) => {
                  const result =
                    metrics[
                      (nodes.data ?? []).findIndex(
                        (item) => item.id === node.id,
                      )
                    ];
                  if (!result) return "—";
                  if (result.isError) return "获取失败";
                  const memory = result.data?.snapshot?.memory;
                  if (!memory || !Number(memory.total_bytes)) return "暂无数据";
                  return `${((Number(memory.used_bytes) / Number(memory.total_bytes)) * 100).toFixed(1)}%${result.data?.stale ? "（过期）" : ""}`;
                },
              },
              {
                title: "操作",
                key: "actions",
                render: (_, node) => (
                  <Space>
                    <AgentActions
                      nodeId={node.id}
                      csrf={session.csrf_token}
                      registrationOnly
                    />
                    {page === "nodes" &&
                      (local.data?.node_id === node.id ? (
                        "本机"
                      ) : (
                        <Button
                          size="small"
                          loading={markLocal.isPending}
                          onClick={() => markLocal.mutate(node.id)}
                        >
                          设为本机
                        </Button>
                      ))}
                    <Button
                      size="small"
                      onClick={() => {
                        setEditing(node.id);
                        form.setFieldsValue({ name: node.name });
                        save.reset();
                      }}
                    >
                      编辑
                    </Button>
                    <Popconfirm
                      title="撤销此节点？"
                      description="撤销后将从节点列表移除，审计记录会保留。"
                      onConfirm={() => remove.mutateAsync(node.id)}
                    >
                      <Button size="small" danger loading={remove.isPending}>
                        撤销
                      </Button>
                    </Popconfirm>
                  </Space>
                ),
              },
            ]}
          />
        </Card>
      )}
      {page !== "nodes" && (
        <Card title="最近审计记录" className="section">
          <Table<AuditEntry>
            rowKey="id"
            loading={audit.isPending}
            dataSource={audit.data ?? []}
            size="small"
            scroll={{ x: 480 }}
            columns={[
              {
                title: "操作",
                dataIndex: "action",
                render: (value: string) => actions[value] ?? value,
              },
              {
                title: "结果",
                dataIndex: "result",
                render: (value: string) =>
                  value === "success" ? "成功" : "拒绝",
              },
              {
                title: "时间",
                dataIndex: "created_at",
                render: (value: number) =>
                  new Date(value * 1000).toLocaleString(),
              },
            ]}
          />
        </Card>
      )}
    </>
  );
}

export default function AccountPanel() {
  const client = useQueryClient();
  const session = useQuery({
    queryKey: ["session"],
    queryFn: getSession,
    retry: false,
  });
  useEffect(() => {
    if (session.isSuccess && session.data === null) {
      client.removeQueries({ queryKey: ["nodes"] });
      client.removeQueries({ queryKey: ["audit"] });
      client.removeQueries({ queryKey: ["task"] });
      client.removeQueries({ queryKey: ["metrics"] });
    }
  }, [client, session.data, session.isSuccess]);
  if (session.isPending) return <Spin aria-label="正在检查登录状态" />;
  if (session.isError)
    return (
      <Alert
        className="section"
        type="error"
        title={session.error.message}
        action={
          <Button onClick={() => void session.refetch()}>重试登录状态</Button>
        }
      />
    );
  return session.data ? <Management session={session.data} /> : <LoginForm />;
}
