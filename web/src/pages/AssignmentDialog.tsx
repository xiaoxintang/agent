import { useEffect, useState } from "react";
import { App, Form, Input, Modal, Select, Typography } from "antd";
import { SchemaField } from "../schema/SchemaField";
import { send, errorMessage } from "../api";
import { useWorkspace } from "../state/workspace";
export function AssignmentDialog({
  agent,
  onClose,
}: {
  agent: string;
  onClose: () => void;
}) {
  const { agents, users, discovery, refresh } = useWorkspace();
  const { message } = App.useApp();
  const [assignmentForm] = Form.useForm();
  const [assignmentAgent, setAssignmentAgent] = useState(agent);
  const [inboundResult, setInboundResult] = useState<{
    agent: string;
    items: any[];
  }>();
  const inbounds =
    inboundResult?.agent === assignmentAgent ? inboundResult.items : [];
  const [extraUser, setExtraUser] = useState<any>({});
  const [adopt, setAdopt] = useState(false);
  const schemas = discovery?.openapi?.components?.schemas || {};
  const showError = (error: unknown) => message.error(errorMessage(error));
  useEffect(() => {
    let active = true;
    send(`/agents/${encodeURIComponent(assignmentAgent)}/invoke`, {
      method: "GET",
      path: "/api/v1/xray/config",
    })
      .then((value) => {
        if (active)
          setInboundResult({
            agent: assignmentAgent,
            items: value.config?.inbounds || [],
          });
      })
      .catch((error) => {
        if (active) message.error(errorMessage(error));
      });
    return () => {
      active = false;
    };
  }, [assignmentAgent, message]);
  return (
    <Modal
      title="分配 Xray 用户"
      open
      width={760}
      onCancel={() => onClose()}
      onOk={() => assignmentForm.submit()}
    >
      <Form
        form={assignmentForm}
        layout="vertical"
        initialValues={{ agent_id: agent }}
        onFinish={async (values) => {
          try {
            await send("/assignments", {
              ...values,
              ...(adopt ? {} : { user: extraUser }),
            });
            onClose();
            await refresh();
            message.success(adopt ? "已接管现有用户" : "已提交分配任务");
          } catch (e) {
            showError(e);
          }
        }}
      >
        <Form.Item
          name="user_id"
          label="全局 Xray 用户"
          rules={[{ required: true }]}
        >
          <Select
            options={users
              .filter((u) => u.status === "active")
              .map((u) => ({ value: u.id, label: u.name }))}
          />
        </Form.Item>
        <Form.Item name="agent_id" label="Agent" rules={[{ required: true }]}>
          <Select
            options={agents.map((a) => ({ value: a.id, label: a.name }))}
            onChange={(value) => {
              setAssignmentAgent(value);
              assignmentForm.setFieldValue("inbound_tag", undefined);
            }}
          />
        </Form.Item>
        <Form.Item name="inbound_tag" label="入站" rules={[{ required: true }]}>
          <Select
            options={inbounds.map((i) => ({
              value: i.tag,
              label: `${i.tag} · ${i.protocol}`,
            }))}
          />
        </Form.Item>
        <Form.Item label="分配方式">
          <Select
            value={adopt ? "adopt" : "create"}
            options={[
              { value: "create", label: "生成独立凭据" },
              { value: "adopt", label: "接管已有用户，保留凭据" },
            ]}
            onChange={(v) => {
              setAdopt(v === "adopt");
              assignmentForm.setFieldValue("adopt_email", undefined);
            }}
          />
        </Form.Item>
        {adopt ? (
          <Form.Item
            name="adopt_email"
            label="现有 Xray 用户 email 标识"
            rules={[{ required: true }]}
          >
            <Input />
          </Form.Item>
        ) : (
          <>
            <Typography.Paragraph type="secondary">
              未指定的访问凭据将自动生成；email 使用稳定分配标识。
            </Typography.Paragraph>
            <SchemaField
              schema={{
                ...(schemas.NativeUser || { type: "object" }),
                required: [],
              }}
              schemas={schemas}
              value={extraUser}
              onChange={setExtraUser}
            />
          </>
        )}
      </Form>
    </Modal>
  );
}
