import { useState } from "react";
import { App, Button, Card, Form, Input, Modal, Space, Table, Tag } from "antd";
import { CloudServerOutlined } from "@ant-design/icons";
import { useDelete } from "../hooks/useDelete";
import { send, errorMessage } from "../api";
import type { Agent } from "../types";
import { useWorkspace } from "../state/workspace";
export function AgentsPage() {
  const { agents, setAgent, discover, refresh } = useWorkspace();
  const [agentModal, setAgentModal] = useState<Agent | boolean>(false);
  const [agentForm] = Form.useForm();
  const confirmDelete = useDelete();
  const { message } = App.useApp();
  const showError = (error: unknown) => message.error(errorMessage(error));
  return (
    <>
      <Card
        title="连接与凭据"
        extra={
          <Button
            type="primary"
            onClick={() => {
              agentForm.resetFields();
              setAgentModal(true);
            }}
          >
            添加 Agent
          </Button>
        }
      >
        <Table
          rowKey="id"
          dataSource={agents}
          columns={[
            {
              title: "名称",
              dataIndex: "name",
              render: (v, row) => (
                <Space>
                  <CloudServerOutlined />
                  {v}
                  {row.id === "local" && <Tag>本机</Tag>}
                </Space>
              ),
            },
            { title: "地址", dataIndex: "url" },
            {
              title: "操作",
              render: (_, row) => (
                <Space>
                  <Button
                    onClick={() => {
                      setAgent(row.id);
                      void discover(row.id);
                    }}
                  >
                    连接检查
                  </Button>
                  {row.id !== "local" && (
                    <>
                      <Button
                        onClick={() => {
                          agentForm.setFieldsValue({ ...row, token: "" });
                          setAgentModal(row);
                        }}
                      >
                        编辑
                      </Button>
                      <Button
                        danger
                        onClick={() =>
                          confirmDelete("移除 Agent？", `/agents/${row.id}`)
                        }
                      >
                        移除
                      </Button>
                    </>
                  )}
                </Space>
              ),
            },
          ]}
        />
      </Card>
      <Modal
        title={typeof agentModal === "object" ? "编辑 Agent" : "添加 Agent"}
        open={!!agentModal}
        onCancel={() => setAgentModal(false)}
        onOk={() => agentForm.submit()}
      >
        <Form
          form={agentForm}
          layout="vertical"
          onFinish={async (values) => {
            try {
              await send("/agents", {
                ...values,
                ...(typeof agentModal === "object"
                  ? { id: agentModal.id }
                  : {}),
              });
              setAgentModal(false);
              await refresh();
            } catch (e) {
              showError(e);
            }
          }}
        >
          <Form.Item name="name" label="名称" rules={[{ required: true }]}>
            <Input />
          </Form.Item>
          <Form.Item
            name="url"
            label="Agent 地址"
            rules={[{ required: true }, { type: "url" }]}
          >
            <Input placeholder="https://agent.example.com:8080" />
          </Form.Item>
          <Form.Item
            name="token"
            label="Bearer Token"
            rules={[{ required: typeof agentModal !== "object" }]}
          >
            <Input.Password
              placeholder={
                typeof agentModal === "object" ? "留空保留原 Token" : ""
              }
            />
          </Form.Item>
        </Form>
      </Modal>
    </>
  );
}
