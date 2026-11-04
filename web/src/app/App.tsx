import { useQuery } from "@tanstack/react-query";
import { Alert, Button, Card, Flex, Spin, Tag, Typography } from "antd";
import { fetchHealth } from "../api/health";
import AccountPanel from "./AccountPanel";

export default function App() {
  const health = useQuery({
    queryKey: ["health"],
    queryFn: ({ signal }) => fetchHealth(signal),
    retry: false,
  });

  return (
    <main className="panel">
      <Typography.Title>Linux Panel</Typography.Title>
      <Typography.Paragraph>轻量化 Linux 管理面板</Typography.Paragraph>
      <Card title="服务状态">
        <Flex vertical gap="middle" align="start">
          {health.isPending && <Spin aria-label="正在检查服务" />}
          {health.isError && (
            <Alert type="error" title={health.error.message} showIcon />
          )}
          {health.isSuccess && (
            <Flex align="center" gap="small">
              <Tag color="success">服务正常</Tag>
              <Typography.Text>版本 {health.data.version}</Typography.Text>
            </Flex>
          )}
          <Button
            onClick={() => void health.refetch()}
            loading={health.isFetching}
          >
            刷新状态
          </Button>
        </Flex>
      </Card>
      <AccountPanel />
      <Typography.Paragraph className="scope-note">
        当前支持账号认证、Agent 连接、Linux 资源采样和只读服务状态查询。Xray
        安装与控制将在后续阶段实现。
      </Typography.Paragraph>
    </main>
  );
}
