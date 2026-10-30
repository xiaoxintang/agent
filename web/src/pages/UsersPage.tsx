import { useState } from "react";
import { App, Button, Card, Form, Input, Modal, Table, Typography } from "antd";
import { Status } from "../components/Status";
import { useDelete } from "../hooks/useDelete";
import { send, errorMessage } from "../api";
import { useWorkspace } from "../state/workspace";
export function UsersPage() {
  const { users, assignments, refresh } = useWorkspace();
  const [userModal, setUserModal] = useState(false);
  const [userForm] = Form.useForm();
  const confirmDelete = useDelete();
  const { message } = App.useApp();
  const showError = (error: unknown) => message.error(errorMessage(error));
  return (
    <>
      <Card
        title="全局 Xray 用户"
        extra={
          <Button type="primary" onClick={() => setUserModal(true)}>
            创建用户
          </Button>
        }
      >
        <Typography.Paragraph type="secondary">
          这些用户用于 Xray 访问授权。Web 管理员账号由服务器命令管理。
        </Typography.Paragraph>
        <Table
          rowKey="id"
          dataSource={users}
          columns={[
            { title: "名称", dataIndex: "name" },
            { title: "用户 ID", dataIndex: "id" },
            {
              title: "状态",
              dataIndex: "status",
              render: (v) => <Status value={v} />,
            },
            {
              title: "分配数量",
              render: (_, row) =>
                assignments.filter(
                  (a) => a.user_id === row.id && a.status !== "revoked",
                ).length,
            },
            {
              title: "操作",
              render: (_, row) => (
                <Button
                  danger
                  disabled={row.status !== "active"}
                  onClick={() =>
                    confirmDelete(
                      "删除用户并撤销其全部分配？",
                      `/users/${row.id}`,
                    )
                  }
                >
                  删除
                </Button>
              ),
            },
          ]}
        />
      </Card>
      <Modal
        title="创建 Xray 用户"
        open={userModal}
        onCancel={() => setUserModal(false)}
        onOk={() => userForm.submit()}
      >
        <Form
          form={userForm}
          layout="vertical"
          onFinish={async (values) => {
            try {
              await send("/users", values);
              setUserModal(false);
              userForm.resetFields();
              await refresh();
            } catch (e) {
              showError(e);
            }
          }}
        >
          <Form.Item name="name" label="用户名称" rules={[{ required: true }]}>
            <Input />
          </Form.Item>
        </Form>
      </Modal>
    </>
  );
}
