import { useEffect } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Alert, Button, Card, Menu, Space, Spin, Tag, Typography } from "antd";
import { Link, useParams } from "react-router-dom";
import { ApiError, getNode } from "../api/panel";
import AgentActions from "./AgentActions";
import ConfigPanel from "./ConfigPanel";
import NodeMetricsPanel from "./NodeMetricsPanel";
import TrafficPanel from "./TrafficPanel";
import XrayPanel from "./XrayPanel";

export default function NodePage({
  section,
  csrf,
}: {
  section: "resources" | "xray" | "configuration" | "traffic" | "agent";
  csrf: string;
}) {
  const { nodeId } = useParams<{ nodeId: string }>();
  const client = useQueryClient();
  const node = useQuery({
    queryKey: ["node", nodeId],
    queryFn: ({ signal }) => getNode(nodeId!, signal),
    enabled: !!nodeId,
    retry: false,
    refetchInterval: 5000,
  });
  useEffect(() => {
    if (node.error instanceof ApiError && node.error.status === 401)
      client.setQueryData(["session"], null);
  }, [client, node.error]);
  if (node.isError)
    return (
      <Alert
        type="error"
        title={node.error.message}
        action={<Button onClick={() => void node.refetch()}>重试</Button>}
      />
    );
  if (!node.data) return <Spin aria-label="正在加载节点" />;
  const id = node.data.id;
  const content = {
    resources: <NodeMetricsPanel nodeId={id} page />,
    xray: <XrayPanel nodeId={id} csrf={csrf} page />,
    configuration: <ConfigPanel nodeId={id} csrf={csrf} page />,
    traffic: <TrafficPanel nodeId={id} page />,
    agent: (
      <Card title="Agent 操作">
        <AgentActions nodeId={id} csrf={csrf} />
      </Card>
    ),
  };
  return (
    <div key={id}>
      <Space orientation="vertical" className="section">
        <Link to="/nodes">返回节点列表</Link>
        <Typography.Title level={3}>
          当前节点：{node.data.name}
        </Typography.Title>
        <Space>
          <Typography.Text>{id}</Typography.Text>
          <Tag color={node.data.status === "online" ? "success" : "default"}>
            {node.data.status === "online" ? "在线" : "离线"}
          </Tag>
        </Space>
      </Space>
      <Menu
        mode="horizontal"
        selectedKeys={[section]}
        items={[
          ["resources", "资源"],
          ["xray", "Xray 管理"],
          ["configuration", "配置管理"],
          ["traffic", "流量统计"],
          ["agent", "Agent 操作"],
        ].map(([key, label]) => ({
          key,
          label: (
            <Link to={`/nodes/${encodeURIComponent(id)}/${key}`}>{label}</Link>
          ),
        }))}
      />
      {content[section]}
    </div>
  );
}
