import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Alert, Button, Descriptions, Spin, Typography } from "antd";
import { ApiError, getMetrics } from "../api/panel";
import PanelContainer from "./PanelContainer";

function bytes(value: string | undefined) {
  if (value === undefined) return "不可用";
  const amount = Number(value);
  if (amount >= 1024 ** 3) return `${(amount / 1024 ** 3).toFixed(2)} GiB`;
  return `${(amount / 1024 ** 2).toFixed(2)} MiB`;
}

function rate(value: number | null | undefined) {
  return value == null
    ? "首轮采样或计数重置"
    : `${(value / 1024).toFixed(1)} KiB/s`;
}

export default function NodeMetricsPanel({
  nodeId,
  page = false,
}: {
  nodeId: string;
  page?: boolean;
}) {
  const client = useQueryClient();
  const [open, setOpen] = useState(page);
  const metrics = useQuery({
    queryKey: ["metrics", nodeId],
    queryFn: () => getMetrics(nodeId),
    enabled: open,
    retry: false,
    refetchInterval: open ? 5000 : false,
  });
  useEffect(() => {
    if (metrics.error instanceof ApiError && metrics.error.status === 401)
      client.setQueryData(["session"], null);
  }, [client, metrics.error]);
  const snapshot = metrics.data?.snapshot;
  const issues = snapshot?.issues ?? [];
  const reason = (name: string) =>
    (
      {
        unsupported_platform: "当前平台不支持 Linux 采集",
        read_failed: "读取失败",
        invalid_data: "采样数据不合法",
        limit_exceeded: "采样量超出上限",
      } as Record<string, string>
    )[name] ?? name;
  const cpuIssue = issues.find((issue) => issue.metric === "cpu");
  return (
    <>
      {!page && (
        <Button size="small" onClick={() => setOpen(true)}>
          查看资源
        </Button>
      )}
      {open && (
        <PanelContainer
          page={page}
          title="节点资源"
          open
          onCancel={() => setOpen(false)}
          footer={<Button onClick={() => setOpen(false)}>关闭资源</Button>}
        >
          {metrics.isPending && <Spin aria-label="正在读取节点资源" />}
          {metrics.error && (
            <Alert type="error" title={metrics.error.message} />
          )}
          {metrics.data && !snapshot && (
            <Alert type="info" title="等待 Agent 首次采样" />
          )}
          {snapshot && (
            <>
              {metrics.data?.stale && (
                <Alert
                  type="warning"
                  title="Agent 离线或采样已过期，以下为历史数据"
                />
              )}
              <Descriptions
                column={1}
                items={[
                  {
                    key: "cpu",
                    label: "CPU",
                    children:
                      snapshot.cpu_percent == null
                        ? cpuIssue
                          ? reason(cpuIssue.reason)
                          : "首轮采样中"
                        : `${snapshot.cpu_percent.toFixed(1)}%`,
                  },
                  {
                    key: "memory",
                    label: "内存（已用 / 总计）",
                    children: `${bytes(snapshot.memory?.used_bytes)} / ${bytes(snapshot.memory?.total_bytes)}`,
                  },
                  {
                    key: "disk",
                    label: "根文件系统（已用 / 总计）",
                    children: `${bytes(snapshot.disk?.used_bytes)} / ${bytes(snapshot.disk?.total_bytes)}`,
                  },
                  {
                    key: "available",
                    label: "根文件系统可用",
                    children: bytes(snapshot.disk?.available_bytes),
                  },
                  {
                    key: "rx",
                    label: "网络接收",
                    children: snapshot.network
                      ? `${rate(snapshot.network.received_bytes_per_second)} · 累计 ${bytes(snapshot.network.received_bytes)}`
                      : "不可用",
                  },
                  {
                    key: "tx",
                    label: "网络发送",
                    children: snapshot.network
                      ? `${rate(snapshot.network.transmitted_bytes_per_second)} · 累计 ${bytes(snapshot.network.transmitted_bytes)}`
                      : "不可用",
                  },
                ]}
              />
              {issues
                .filter((issue) => issue.metric !== "cpu")
                .map((issue) => (
                  <Typography.Paragraph key={issue.metric} type="secondary">
                    {
                      (
                        {
                          memory: "内存",
                          disk: "磁盘",
                          network: "网络",
                        } as Record<string, string>
                      )[issue.metric]
                    }
                    ：{reason(issue.reason)}
                  </Typography.Paragraph>
                ))}
              <Typography.Paragraph type="secondary">
                网络统计汇总非 loopback 接口，包含虚拟接口，不作为 Xray
                流量计费依据。
              </Typography.Paragraph>
            </>
          )}
        </PanelContainer>
      )}
    </>
  );
}
