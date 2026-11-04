import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Alert,
  Button,
  Modal,
  Popconfirm,
  Select,
  Space,
  Typography,
} from "antd";
import { ApiError, createXrayTask, getTask } from "../api/panel";
import type { XrayVersion } from "../types/generated";

export default function XrayPanel({
  nodeId,
  csrf,
}: {
  nodeId: string;
  csrf: string;
}) {
  const client = useQueryClient();
  const [open, setOpen] = useState(false);
  const [version, setVersion] = useState<XrayVersion>("26.10.31");
  const [taskId, setTaskId] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const task = useQuery({
    queryKey: ["task", taskId],
    queryFn: () => getTask(taskId!),
    enabled: !!taskId,
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
  const pending =
    busy ||
    (!!taskId &&
      !["succeeded", "failed", "expired", "canceled"].includes(
        task.data?.status ?? "pending",
      ));
  async function submit(action: Parameters<typeof createXrayTask>[1]) {
    setBusy(true);
    setError(undefined);
    try {
      const response = await createXrayTask(
        nodeId,
        action,
        csrf,
        action === "install" || action === "upgrade" ? version : undefined,
      );
      setTaskId(response.id);
    } catch (error) {
      if (error instanceof ApiError && error.status === 401)
        client.setQueryData(["session"], null);
      setError(error instanceof Error ? error.message : "提交失败");
    } finally {
      setBusy(false);
    }
  }
  const result = task.data?.result;
  const report =
    result?.status === "xray" && result.data.status === "ok"
      ? result.data.data
      : null;
  const failure =
    result?.status === "xray" && result.data.status === "error"
      ? result.data.data
      : null;
  const errors: Record<string, string> = {
    helper_unavailable: "节点尚未配置受限 Xray helper，请由本机管理员完成配置",
    unauthorized: "本机 helper 拒绝了操作权限",
    unowned_installation: "检测到非受管或已修改的文件，已拒绝接管",
    verification_failed: "官方产物或配置校验失败",
    download_failed: "官方产物下载失败",
    service_failed: "服务操作失败；升级会尝试恢复旧版本",
    rollback_failed: "恢复旧版本失败，请在节点本机检查",
    invalid_state: "当前安装状态不允许此操作",
    interrupted: "操作中断，请先查询状态再决定是否提交新操作",
    unsupported_platform: "当前平台不支持 Xray 管理",
    expired: "操作已过期",
  };
  return (
    <>
      <Button size="small" onClick={() => setOpen(true)}>
        Xray 管理
      </Button>
      {open && (
        <Modal
          title="受管 Xray"
          open
          onCancel={() => setOpen(false)}
          footer={
            <Button onClick={() => setOpen(false)}>关闭 Xray 管理</Button>
          }
        >
          <Alert
            type="info"
            title="仅管理本项目拥有的 Xray。安装初始配置没有监听端口；不会接管已有的非受管安装。"
          />
          <Space orientation="vertical" size="middle">
            <Select
              aria-label="Xray 版本"
              value={version}
              onChange={setVersion}
              options={[
                { value: "26.10.31", label: "26.10.31" },
                { value: "26.10.21", label: "26.10.21" },
              ]}
            />
            <Space wrap>
              <Button disabled={pending} onClick={() => void submit("status")}>
                查询安装状态
              </Button>
              <Button disabled={pending} onClick={() => void submit("install")}>
                安装所选版本
              </Button>
              <Popconfirm
                title="升级到所选版本？运行中的服务将重启。"
                onConfirm={() => submit("upgrade")}
              >
                <Button disabled={pending}>升级</Button>
              </Popconfirm>
              <Button disabled={pending} onClick={() => void submit("start")}>
                启动
              </Button>
              <Popconfirm
                title="停止受管 Xray？"
                onConfirm={() => submit("stop")}
              >
                <Button disabled={pending}>停止</Button>
              </Popconfirm>
              <Popconfirm
                title="重启受管 Xray？"
                onConfirm={() => submit("restart")}
              >
                <Button disabled={pending}>重启</Button>
              </Popconfirm>
              <Popconfirm
                title="恢复上一版本？运行中的服务将重启。"
                onConfirm={() => submit("rollback")}
              >
                <Button disabled={pending}>回滚上一版本</Button>
              </Popconfirm>
              <Popconfirm
                title="卸载受管二进制与服务？配置和数据会保留。"
                onConfirm={() => submit("uninstall")}
              >
                <Button danger disabled={pending}>
                  卸载并保留数据
                </Button>
              </Popconfirm>
            </Space>
            {error && <Alert type="error" title={error} />}
            {task.error && <Alert type="error" title={task.error.message} />}
            {task.data && (
              <Typography.Text>任务：{task.data.status}</Typography.Text>
            )}
            {report && (
              <Typography.Paragraph>
                {report.installed
                  ? `已安装 ${report.version} · ${report.active ? "运行中" : "已停止"} · 上一版本 ${report.previous_version ?? "无"}`
                  : "未安装（保留的数据不会自动删除）"}
              </Typography.Paragraph>
            )}
            {failure && (
              <Alert type="error" title={errors[failure] ?? failure} />
            )}
          </Space>
        </Modal>
      )}
    </>
  );
}
