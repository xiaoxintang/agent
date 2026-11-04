import { useEffect } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Alert,
  Button,
  Card,
  Flex,
  Layout,
  Menu,
  Spin,
  Tag,
  Typography,
} from "antd";
import {
  Link,
  Navigate,
  Route,
  Routes,
  useLocation,
  useNavigate,
} from "react-router-dom";
import { fetchHealth } from "../api/health";
import { ApiError, getLocalNode, getSession, logout } from "../api/panel";
import { LoginForm, Management } from "./AccountPanel";
import AgentActions from "./AgentActions";
import ConfigPanel from "./ConfigPanel";
import NodeMetricsPanel from "./NodeMetricsPanel";
import TrafficPanel from "./TrafficPanel";
import XrayPanel from "./XrayPanel";

const menu = [
  ["/overview", "服务概览"],
  ["/resources", "本机资源"],
  ["/xray", "Xray 管理"],
  ["/configuration", "配置管理"],
  ["/traffic", "流量统计"],
  ["/agent", "Agent 操作"],
  ["/nodes", "节点管理"],
  ["/audit", "审计记录"],
];

function Overview() {
  const health = useQuery({
    queryKey: ["health"],
    queryFn: ({ signal }) => fetchHealth(signal),
    retry: false,
  });
  return (
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
  );
}

export default function App() {
  const client = useQueryClient();
  const location = useLocation();
  const navigate = useNavigate();
  const session = useQuery({
    queryKey: ["session"],
    queryFn: getSession,
    retry: false,
  });
  const local = useQuery({
    queryKey: ["local-node"],
    queryFn: getLocalNode,
    enabled: !!session.data,
    retry: false,
  });
  const exit = useMutation({
    mutationFn: () => logout(session.data!.csrf_token),
    onSuccess: () => client.setQueryData(["session"], null),
    onError: (error) => {
      if (error instanceof ApiError && error.status === 401)
        client.setQueryData(["session"], null);
    },
  });
  useEffect(() => {
    if (local.error instanceof ApiError && local.error.status === 401)
      client.setQueryData(["session"], null);
  }, [client, local.error]);
  useEffect(() => {
    if (session.isSuccess && session.data === null)
      client.removeQueries({
        predicate: (query) =>
          !["session", "health"].includes(String(query.queryKey[0])),
      });
  }, [client, session.data, session.isSuccess]);
  if (session.isPending)
    return (
      <main className="login-page">
        <Spin aria-label="正在检查登录状态" />
      </main>
    );
  if (session.isError)
    return (
      <main className="login-page">
        <Alert
          type="error"
          title={session.error.message}
          action={
            <Button onClick={() => void session.refetch()}>重试登录状态</Button>
          }
        />
      </main>
    );
  if (!session.data) {
    if (location.pathname !== "/login")
      return (
        <Navigate
          to="/login"
          replace
          state={{ from: location.pathname + location.search }}
        />
      );
    return (
      <main className="login-page">
        <Typography.Title level={2}>Linux Panel</Typography.Title>
        <LoginForm />
      </main>
    );
  }
  if (location.pathname === "/login") {
    const from = (location.state as { from?: string } | null)?.from;
    return (
      <Navigate
        to={
          from?.startsWith("/") && !from.startsWith("//") ? from : "/overview"
        }
        replace
      />
    );
  }
  const localContent = (component: (nodeId: string) => React.ReactNode) => {
    if (local.isPending) return <Spin aria-label="正在加载本机节点" />;
    if (local.isError)
      return (
        <Alert
          type="error"
          title={local.error.message}
          action={<Button onClick={() => void local.refetch()}>重试</Button>}
        />
      );
    if (!local.data?.node_id)
      return (
        <Alert
          type="info"
          title="尚未设置本机节点"
          description={<Link to="/nodes">前往节点管理设置本机</Link>}
        />
      );
    return component(local.data.node_id);
  };
  const csrf = session.data.csrf_token;
  return (
    <Layout className="app-layout">
      <Layout.Sider breakpoint="lg" collapsedWidth="0" className="app-sidebar">
        <Typography.Title level={3} className="app-brand">
          Linux Panel
        </Typography.Title>
        <Menu
          theme="dark"
          mode="inline"
          selectedKeys={[location.pathname]}
          items={menu.map(([key, label]) => ({
            key,
            label: <Link to={key}>{label}</Link>,
          }))}
        />
      </Layout.Sider>
      <Layout>
        <Layout.Header className="app-header">
          <Typography.Text>
            当前账号：{session.data.admin.username}
          </Typography.Text>
          <Button onClick={() => exit.mutate()} loading={exit.isPending}>
            退出登录
          </Button>
        </Layout.Header>
        <Layout.Content className="app-content">
          {exit.error && <Alert type="error" title={exit.error.message} />}
          <Routes>
            <Route path="/" element={<Navigate to="/overview" replace />} />
            <Route path="/overview" element={<Overview />} />
            <Route
              path="/nodes"
              element={<Management session={session.data} page="nodes" />}
            />
            <Route
              path="/audit"
              element={<Management session={session.data} page="audit" />}
            />
            <Route
              path="/resources"
              element={localContent((id) => (
                <NodeMetricsPanel key={id} nodeId={id} page />
              ))}
            />
            <Route
              path="/xray"
              element={localContent((id) => (
                <XrayPanel key={id} nodeId={id} csrf={csrf} page />
              ))}
            />
            <Route
              path="/configuration"
              element={localContent((id) => (
                <ConfigPanel key={id} nodeId={id} csrf={csrf} page />
              ))}
            />
            <Route
              path="/traffic"
              element={localContent((id) => (
                <TrafficPanel key={id} nodeId={id} page />
              ))}
            />
            <Route
              path="/agent"
              element={localContent((id) => (
                <Card title="本机 Agent 操作">
                  <AgentActions key={id} nodeId={id} csrf={csrf} />
                </Card>
              ))}
            />
            <Route
              path="*"
              element={
                <Alert
                  type="warning"
                  title="页面不存在"
                  action={
                    <Button onClick={() => navigate("/overview")}>
                      返回首页
                    </Button>
                  }
                />
              }
            />
          </Routes>
        </Layout.Content>
      </Layout>
    </Layout>
  );
}
