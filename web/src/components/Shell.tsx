import { useState } from "react";
import {
  Alert,
  App,
  Badge,
  Button,
  Layout,
  Menu,
  Select,
  Space,
  Spin,
  Typography,
} from "antd";
import {
  ApiOutlined,
  CloudServerOutlined,
  DashboardOutlined,
  DeploymentUnitOutlined,
  LogoutOutlined,
  ReloadOutlined,
  TeamOutlined,
  UnorderedListOutlined,
} from "@ant-design/icons";
import type { Session } from "../types";
import operations from "../operations.json";
import { useWorkspace } from "../state/workspace";
import { errorMessage } from "../api";
import { AgentsPage } from "../pages/AgentsPage";
import { AssignmentsPage } from "../pages/AssignmentsPage";
import { OverviewPage } from "../pages/OverviewPage";
import { TasksPage } from "../pages/TasksPage";
import { UsersPage } from "../pages/UsersPage";
import { OperationPage } from "../operations/OperationPage";
const titles: Record<string, string> = {
  overview: "工作台",
  agents: "Agent 节点",
  users: "Xray 用户",
  assignments: "用户分配",
  tasks: "执行任务",
};
const groups = Array.from(new Set(operations.map((entry) => entry.group)));
const menu = [
  { key: "overview", icon: <DashboardOutlined />, label: titles.overview },
  { key: "agents", icon: <CloudServerOutlined />, label: titles.agents },
  { key: "users", icon: <TeamOutlined />, label: titles.users },
  {
    key: "assignments",
    icon: <DeploymentUnitOutlined />,
    label: titles.assignments,
  },
  { key: "tasks", icon: <UnorderedListOutlined />, label: titles.tasks },
  {
    type: "group" as const,
    label: "节点管理接口",
    children: groups.map((group) => ({
      key: `api:${group}`,
      icon: <ApiOutlined />,
      label: group,
    })),
  },
];
export function Shell({
  session,
  logout,
}: {
  session: Session;
  logout: () => Promise<void>;
}) {
  const [page, setPage] = useState("overview");
  const { message } = App.useApp();
  const {
    ready,
    agent,
    setAgent,
    agents,
    discovery,
    connectionError,
    loading,
    discover,
    refresh,
  } = useWorkspace();
  if (!ready)
    return (
      <div className="boot">
        <Spin size="large" />
      </div>
    );
  const selected = agents.find((item) => item.id === agent);
  const pages: Record<string, React.ReactNode> = {
    overview: <OverviewPage />,
    agents: <AgentsPage />,
    users: <UsersPage />,
    assignments: <AssignmentsPage />,
    tasks: <TasksPage />,
  };
  return (
    <Layout className="shell">
      <Layout.Sider
        width={238}
        breakpoint="lg"
        collapsedWidth={0}
        className="sidebar"
      >
        <div className="brand">
          <DeploymentUnitOutlined />
          <div>
            XRAY <span>CONTROL</span>
            <small>多节点管理平台</small>
          </div>
        </div>
        <Menu
          theme="dark"
          mode="inline"
          selectedKeys={[page]}
          items={menu}
          onClick={({ key }) => setPage(key)}
        />
        <div className="sidebar-footer">
          v{__WEB_VERSION__}
          <br />
          Agent · Web · Xray
        </div>
      </Layout.Sider>
      <Layout>
        <Layout.Header className="topbar">
          <Space>
            <Badge
              status={
                connectionError ? "error" : discovery ? "success" : "default"
              }
            />
            <Select
              aria-label="当前 Agent"
              value={agent}
              style={{ width: 210 }}
              options={agents.map((item) => ({
                value: item.id,
                label: item.name,
              }))}
              onChange={setAgent}
            />
            <Typography.Text type="secondary">{selected?.url}</Typography.Text>
          </Space>
          <Space>
            <Typography.Text>{session.username}</Typography.Text>
            <Button icon={<LogoutOutlined />} onClick={() => void logout()}>
              退出
            </Button>
          </Space>
        </Layout.Header>
        <Layout.Content className="content">
          <div className="page-heading">
            <div>
              <Typography.Text type="secondary">
                管理平台 / {selected?.name || "节点"}
              </Typography.Text>
              <Typography.Title level={2}>
                {titles[page] || page.slice(4)}
              </Typography.Title>
            </div>
            <Button
              icon={<ReloadOutlined />}
              loading={loading}
              onClick={() => {
                void discover();
                void refresh().catch((error) =>
                  message.error(errorMessage(error)),
                );
              }}
            >
              刷新
            </Button>
          </div>
          {connectionError && (
            <Alert
              type="error"
              showIcon
              title="当前节点连接失败"
              description={connectionError}
              style={{ marginBottom: 20 }}
            />
          )}
          {discovery?.status?.sync_state &&
            discovery.status.sync_state !== "in_sync" && (
              <Alert
                type="warning"
                showIcon
                title="Agent 需要一致性恢复，后台配置写入已暂停"
                description={discovery.status.sync_state}
                style={{ marginBottom: 20 }}
              />
            )}
          {pages[page] || <OperationPage key={page} group={page.slice(4)} />}
        </Layout.Content>
      </Layout>
    </Layout>
  );
}
