import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Alert, Button, Modal, Space, Spin, Typography } from "antd";
import {
  ApiError,
  createStatusTask,
  getTask,
  issueEnrollment,
  createServiceStatusTask,
} from "../api/panel";
import type { EnrollmentResponse, ManagedService } from "../types/generated";

export default function AgentActions({
  nodeId,
  csrf,
}: {
  nodeId: string;
  csrf: string;
}) {
  const client = useQueryClient();
  const [open, setOpen] = useState(false);
  const [enrollment, setEnrollment] = useState<EnrollmentResponse | null>(null);
  const [error, setError] = useState<string>();
  const [taskId, setTaskId] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const epoch = useRef(0);
  useEffect(
    () => () => {
      epoch.current += 1;
    },
    [],
  );
  const task = useQuery({
    queryKey: ["task", taskId],
    queryFn: () => getTask(taskId!),
    enabled: taskId !== null,
    retry: false,
    refetchInterval: (query) =>
      ["pending", "dispatched", "acked"].includes(
        query.state.data?.status ?? "pending",
      )
        ? 1000
        : false,
  });
  useEffect(() => {
    if (task.error instanceof ApiError && task.error.status === 401)
      client.setQueryData(["session"], null);
  }, [client, task.error]);
  const fail = (error: unknown) => {
    if (error instanceof ApiError && error.status === 401)
      client.setQueryData(["session"], null);
    setError(error instanceof Error ? error.message : "操作失败");
  };
  async function enroll() {
    const active = ++epoch.current;
    setEnrollment(null);
    setError(undefined);
    setOpen(true);
    try {
      const result = await issueEnrollment(nodeId, csrf);
      if (active === epoch.current) setEnrollment(result);
    } catch (error) {
      if (active === epoch.current) fail(error);
    }
  }
  async function probe(service?: ManagedService) {
    setCreating(true);
    setError(undefined);
    try {
      const result = service
        ? await createServiceStatusTask(nodeId, service, csrf)
        : await createStatusTask(nodeId, csrf);
      setTaskId(result.id);
    } catch (error) {
      fail(error);
    } finally {
      setCreating(false);
    }
  }
  function close() {
    epoch.current += 1;
    setOpen(false);
    setEnrollment(null);
    setError(undefined);
  }
  const terminal =
    task.data && !["pending", "dispatched", "acked"].includes(task.data.status);
  const url = `${window.location.protocol === "https:" ? "wss" : "ws"}://${window.location.host}/agent/v1/connect`;
  const development =
    window.location.protocol !== "https:" ? " --allow-local-ws" : "";
  return (
    <Space orientation="vertical" size="small">
      <Space>
        <Button size="small" onClick={() => void enroll()}>
          注册 Agent
        </Button>
        <Button
          size="small"
          loading={creating}
          disabled={!!taskId && !terminal}
          onClick={() => void probe()}
        >
          查询 Agent 状态
        </Button>
        <Button
          size="small"
          loading={creating}
          disabled={!!taskId && !terminal}
          onClick={() => void probe("panel_agent")}
        >
          查询 Agent 服务
        </Button>
        <Button
          size="small"
          loading={creating}
          disabled={!!taskId && !terminal}
          onClick={() => void probe("xray")}
        >
          查询 Xray 服务
        </Button>
      </Space>
      {!open && error && (
        <Typography.Text type="danger">{error}</Typography.Text>
      )}
      {task.error && (
        <Typography.Text type="danger">{task.error.message}</Typography.Text>
      )}
      {task.data && (
        <Typography.Text>
          {(
            {
              pending: "等待 Agent",
              dispatched: "已发送",
              acked: "已接收",
              succeeded: "查询成功",
              failed: "查询失败",
              expired: "任务已过期",
              canceled: "任务已取消",
            } as Record<string, string>
          )[task.data.status] ?? task.data.status}
          {task.data.result?.status === "succeeded" &&
            ` · ${task.data.result.data.version} / ${task.data.result.data.os} / ${task.data.result.data.arch}`}
          {task.data.result?.status === "service_status" &&
            ` · ${task.data.result.data.service === "panel_agent" ? "panel-agent.service" : "xray.service"} · ${task.data.result.data.load_state === "not_loaded" ? "未加载（不代表未安装）" : task.data.result.data.load_state} / ${task.data.result.data.active_state ?? "—"} / ${task.data.result.data.sub_state ?? "—"}`}
          {task.data.result?.status === "unavailable" &&
            ` · ${({ unsupported_platform: "当前平台不支持 systemd", systemd_unavailable: "systemd 总线不可用", access_denied: "本机策略拒绝查询", timeout: "systemd 查询超时", privilege_denied: "Agent 必须以普通用户运行" } as Record<string, string>)[task.data.result.data.reason]}`}
        </Typography.Text>
      )}
      {open && (
        <Modal
          title="注册 Agent"
          open={open}
          onCancel={close}
          footer={<Button onClick={close}>关闭并清除令牌</Button>}
        >
          <Alert
            type="warning"
            showIcon
            title="令牌仅展示这一次，10 分钟内有效。使用后将替换此节点原有的 Agent 凭据。"
          />
          {error && <Alert type="error" title={error} />}
          {!enrollment && !error && <Spin aria-label="正在生成注册令牌" />}
          {enrollment && (
            <>
              <Typography.Paragraph copyable>
                {enrollment.token}
              </Typography.Paragraph>
              <Typography.Paragraph>
                在目标主机运行以下命令，并按提示粘贴令牌：
              </Typography.Paragraph>
              <Typography.Paragraph
                code
                copyable
              >{`panel-agent${development} enroll --server ${url}`}</Typography.Paragraph>
              <Typography.Paragraph
                code
                copyable
              >{`panel-agent${development} run`}</Typography.Paragraph>
              <Typography.Paragraph>
                每个节点使用独立数据目录。私有 CA 需要在 Agent
                配置信任的证书；不要跳过证书验证。
              </Typography.Paragraph>
            </>
          )}
        </Modal>
      )}
    </Space>
  );
}
