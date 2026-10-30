import { useState } from "react";
import { Alert, Button, Card, Form, Input, Space, Tag, Typography } from "antd";
import {
  DeploymentUnitOutlined,
  SafetyCertificateOutlined,
} from "@ant-design/icons";
import { send } from "../api";
export function LoginPage({ onLogin }: { onLogin: () => void }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  return (
    <div className="login">
      <div className="login-copy">
        <div className="brand-mark">
          <DeploymentUnitOutlined />
        </div>
        <Typography.Title>Xray 管理平台</Typography.Title>
        <p>在一个工作台，管理每个节点与连接。</p>
        <Space>
          <Tag>多 Agent</Tag>
          <Tag>独立凭据</Tag>
          <Tag>操作可追踪</Tag>
        </Space>
      </div>
      <Card className="login-card">
        <SafetyCertificateOutlined className="login-icon" />
        <Typography.Title level={3}>管理员登录</Typography.Title>
        <Typography.Paragraph type="secondary">
          使用安装时生成的 Web 账号。
        </Typography.Paragraph>
        {error && (
          <Alert
            type="error"
            title={error}
            showIcon
            style={{ marginBottom: 20 }}
          />
        )}
        <Form
          layout="vertical"
          onFinish={async (values) => {
            setBusy(true);
            setError("");
            try {
              await send("/login", values);
              onLogin();
            } catch (e) {
              setError((e as Error).message);
            } finally {
              setBusy(false);
            }
          }}
        >
          <Form.Item
            name="username"
            label="用户名"
            rules={[{ required: true }]}
          >
            <Input autoComplete="username" size="large" />
          </Form.Item>
          <Form.Item name="password" label="密码" rules={[{ required: true }]}>
            <Input.Password autoComplete="current-password" size="large" />
          </Form.Item>
          <Button
            htmlType="submit"
            type="primary"
            size="large"
            block
            loading={busy}
          >
            登录
          </Button>
        </Form>
        <Typography.Paragraph type="secondary" style={{ marginTop: 24 }}>
          忘记密码时，在服务器执行{" "}
          <Typography.Text code>agent web reset-password</Typography.Text>。
        </Typography.Paragraph>
      </Card>
    </div>
  );
}
