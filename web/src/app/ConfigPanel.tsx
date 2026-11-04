import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Alert,
  Button,
  Checkbox,
  Form,
  Input,
  InputNumber,
  Modal,
  Popconfirm,
  Select,
  Space,
  Typography,
} from "antd";
import {
  ApiError,
  getInbounds,
  saveInbound,
  removeInbound,
  getClients,
  createClient,
  updateClient,
  removeClient,
  assignClient,
  getRevisions,
  applyConfig,
} from "../api/panel";
import type { InboundView } from "../types/generated";

type Fields = {
  name: string;
  listen_address: string;
  listen_port: number;
  target: string;
  server_names: string;
  short_ids: string;
  enabled: boolean;
};
export default function ConfigPanel({
  nodeId,
  csrf,
}: {
  nodeId: string;
  csrf: string;
}) {
  const cache = useQueryClient();
  const [open, setOpen] = useState(false);
  const [editing, setEditing] = useState<string | null>(null);
  const [error, setError] = useState<string>();
  const [editingClient, setEditingClient] = useState<string | null>(null);
  const [secret, setSecret] = useState<string>();
  const [busy, setBusy] = useState(false);
  const [form] = Form.useForm<Fields>();
  const [clientForm] = Form.useForm<{
    display_name: string;
    quota_mib?: number;
    expiry?: string;
  }>();
  const inbounds = useQuery({
    queryKey: ["inbounds", nodeId],
    queryFn: () => getInbounds(nodeId),
    enabled: open,
    retry: false,
  });
  const clients = useQuery({
    queryKey: ["clients"],
    queryFn: getClients,
    enabled: open,
    retry: false,
  });
  const revisions = useQuery({
    queryKey: ["revisions", nodeId],
    queryFn: () => getRevisions(nodeId),
    enabled: open,
    retry: false,
    refetchInterval: open ? 2000 : false,
  });
  useEffect(() => {
    if (
      [inbounds.error, clients.error, revisions.error].some(
        (error) => error instanceof ApiError && error.status === 401,
      )
    )
      cache.setQueryData(["session"], null);
  }, [cache, inbounds.error, clients.error, revisions.error]);
  async function run(action: () => Promise<unknown>) {
    setBusy(true);
    setError(undefined);
    try {
      await action();
      await Promise.all([
        inbounds.refetch(),
        clients.refetch(),
        revisions.refetch(),
      ]);
    } catch (error) {
      if (error instanceof ApiError && error.status === 401)
        cache.setQueryData(["session"], null);
      setError(error instanceof Error ? error.message : "操作失败");
    } finally {
      setBusy(false);
    }
  }
  function edit(inbound: InboundView) {
    setEditing(inbound.id);
    form.setFieldsValue({
      ...inbound,
      server_names: inbound.server_names.join(","),
      short_ids: inbound.short_ids.join(","),
    });
  }
  const close = () => {
    setOpen(false);
    setSecret(undefined);
    setError(undefined);
    setEditing(null);
    setEditingClient(null);
    form.resetFields();
    clientForm.resetFields();
  };
  return (
    <>
      <Button size="small" onClick={() => setOpen(true)}>
        入站与客户端
      </Button>
      {open && (
        <Modal
          title="VLESS + REALITY 配置"
          open
          width={800}
          onCancel={close}
          footer={<Button onClick={close}>关闭配置</Button>}
        >
          <Space orientation="vertical" style={{ width: "100%" }}>
            <Alert
              type="info"
              title="保存后需应用修订才会更新节点；应用会重启 Xray。请使用你有权使用、支持 TLS 1.3 的 REALITY 目标。"
            />
            {(error || inbounds.error || clients.error || revisions.error) && (
              <Alert
                type="error"
                title={
                  error ??
                  inbounds.error?.message ??
                  clients.error?.message ??
                  revisions.error?.message
                }
              />
            )}
            <Typography.Title level={5}>
              {editing ? "编辑入站" : "创建入站"}
            </Typography.Title>
            <Form
              form={form}
              layout="vertical"
              initialValues={{
                listen_address: "0.0.0.0",
                listen_port: 443,
                enabled: true,
              }}
              onFinish={(values: Fields) =>
                void run(async () => {
                  await saveInbound(
                    nodeId,
                    editing,
                    {
                      ...values,
                      server_names: values.server_names
                        .split(",")
                        .map((v) => v.trim()),
                      short_ids: values.short_ids
                        .split(",")
                        .map((v) => v.trim()),
                    },
                    csrf,
                  );
                  setEditing(null);
                  form.resetFields();
                })
              }
            >
              <Form.Item
                name="name"
                label="入站名称"
                rules={[{ required: true }]}
              >
                <Input maxLength={80} />
              </Form.Item>
              <Form.Item
                name="listen_address"
                label="监听 IP"
                rules={[{ required: true }]}
              >
                <Input />
              </Form.Item>
              <Form.Item
                name="listen_port"
                label="监听端口"
                rules={[{ required: true }]}
              >
                <InputNumber min={1} max={65535} />
              </Form.Item>
              <Form.Item
                name="target"
                label="REALITY 目标（域名:端口）"
                rules={[{ required: true }]}
              >
                <Input placeholder="www.example.com:443" />
              </Form.Item>
              <Form.Item
                name="server_names"
                label="SNI 域名（逗号分隔）"
                rules={[{ required: true }]}
              >
                <Input />
              </Form.Item>
              <Form.Item
                name="short_ids"
                label="Short ID（偶数位十六进制，逗号分隔）"
                rules={[{ required: true }]}
              >
                <Input />
              </Form.Item>
              <Form.Item name="enabled" valuePropName="checked">
                <Checkbox>启用入站</Checkbox>
              </Form.Item>
              <Space>
                <Button htmlType="submit" loading={busy}>
                  保存入站
                </Button>
                {editing && (
                  <Button
                    onClick={() => {
                      setEditing(null);
                      form.resetFields();
                    }}
                  >
                    取消编辑
                  </Button>
                )}
              </Space>
            </Form>
            {inbounds.data?.map((inbound) => (
              <Space
                orientation="vertical"
                key={inbound.id}
                style={{ width: "100%" }}
              >
                <Typography.Text>
                  {inbound.name} · {inbound.listen_address}:
                  {inbound.listen_port} · {inbound.enabled ? "启用" : "停用"}
                </Typography.Text>
                <Typography.Text copyable>
                  公钥：{inbound.public_key}
                </Typography.Text>
                <Space>
                  <Button onClick={() => edit(inbound)}>
                    编辑 {inbound.name}
                  </Button>
                  <Popconfirm
                    title="删除此入站？应用后才会从节点移除。"
                    onConfirm={() => run(() => removeInbound(inbound.id, csrf))}
                  >
                    <Button danger disabled={busy}>
                      删除入站
                    </Button>
                  </Popconfirm>
                </Space>
                <Select
                  aria-label={`${inbound.name} 的客户端`}
                  mode="multiple"
                  style={{ width: "100%" }}
                  value={inbound.client_ids}
                  disabled={busy}
                  options={clients.data?.map((client) => ({
                    value: client.id,
                    label: client.display_name,
                  }))}
                  onChange={(selected) =>
                    void run(async () => {
                      for (const id of inbound.client_ids.filter(
                        (id) => !selected.includes(id),
                      ))
                        await assignClient(inbound.id, id, false, csrf);
                      for (const id of selected.filter(
                        (id) => !inbound.client_ids.includes(id),
                      ))
                        await assignClient(inbound.id, id, true, csrf);
                    })
                  }
                />
              </Space>
            ))}
            <Typography.Title level={5}>全局客户端</Typography.Title>
            <Form
              form={clientForm}
              layout="inline"
              onFinish={({
                display_name,
                quota_mib,
                expiry,
              }: {
                display_name: string;
                quota_mib?: number;
                expiry?: string;
              }) =>
                void run(async () => {
                  setSecret(undefined);
                  const policy = {
                    display_name,
                    enabled:
                      clients.data?.find(
                        (client) => client.id === editingClient,
                      )?.enabled ?? true,
                    quota_bytes: quota_mib
                      ? String(Math.round(quota_mib * 1048576))
                      : null,
                    expiry_at: expiry
                      ? Math.floor(new Date(expiry).getTime() / 1000)
                      : null,
                  };
                  if (editingClient)
                    await updateClient(editingClient, policy, csrf);
                  else {
                    const created = await createClient(policy, csrf);
                    setSecret(created.protocol_id);
                  }
                  setEditingClient(null);
                  clientForm.resetFields();
                })
              }
            >
              <Form.Item
                name="display_name"
                label="客户端名称"
                rules={[{ required: true }]}
              >
                <Input maxLength={64} />
              </Form.Item>
              <Form.Item name="quota_mib" label="总配额 MiB（空为不限）">
                <InputNumber min={1} max={8589934591} precision={0} />
              </Form.Item>
              <Form.Item name="expiry" label="到期时间（本地时区）">
                <Input type="datetime-local" />
              </Form.Item>
              <Button htmlType="submit" disabled={busy}>
                {editingClient ? "保存客户端策略" : "创建客户端"}
              </Button>
              {editingClient && (
                <Button
                  onClick={() => {
                    setEditingClient(null);
                    clientForm.resetFields();
                  }}
                >
                  取消策略编辑
                </Button>
              )}
            </Form>
            <Alert
              type="info"
              title="用量为全部节点累计值。达到配额或到期会自动提交移除修订；提高配额或延长到期后，需手动应用配置恢复客户端。离线节点恢复连接后才会执行。"
            />
            {secret && (
              <Alert
                type="warning"
                title="客户端 UUID 仅此时显示，请安全保存"
                description={
                  <Typography.Text copyable>{secret}</Typography.Text>
                }
                closable
                onClose={() => setSecret(undefined)}
              />
            )}
            {clients.data?.map((client) => (
              <Space key={client.id}>
                <Checkbox
                  checked={client.enabled}
                  disabled={busy}
                  onChange={(event) =>
                    void run(() =>
                      updateClient(
                        client.id,
                        {
                          display_name: client.display_name,
                          enabled: event.target.checked,
                          expiry_at: client.expiry_at,
                          quota_bytes: client.quota_bytes,
                        },
                        csrf,
                      ),
                    )
                  }
                >
                  {client.display_name}
                </Checkbox>
                <Typography.Text>
                  已用 {(Number(client.used_bytes ?? 0) / 1048576).toFixed(2)}{" "}
                  MiB · {client.policy_blocked ? "配额/到期已限制" : "策略允许"}
                </Typography.Text>
                <Button
                  size="small"
                  onClick={() => {
                    setEditingClient(client.id);
                    setSecret(undefined);
                    clientForm.setFieldsValue({
                      display_name: client.display_name,
                      quota_mib: client.quota_bytes
                        ? Number(client.quota_bytes) / 1048576
                        : undefined,
                      expiry: client.expiry_at
                        ? new Date(
                            client.expiry_at * 1000 -
                              new Date().getTimezoneOffset() * 60000,
                          )
                            .toISOString()
                            .slice(0, 16)
                        : undefined,
                    });
                  }}
                >
                  编辑配额/到期
                </Button>
                <Popconfirm
                  title="从所有入站删除此客户端？应用后生效。"
                  onConfirm={() => run(() => removeClient(client.id, csrf))}
                >
                  <Button danger size="small" disabled={busy}>
                    删除客户端
                  </Button>
                </Popconfirm>
              </Space>
            ))}
            <Popconfirm
              title="应用当前配置并重启 Xray？连接可能短暂中断。"
              onConfirm={() => run(() => applyConfig(nodeId, csrf))}
            >
              <Button type="primary" disabled={busy}>
                应用配置修订
              </Button>
            </Popconfirm>
            {revisions.data?.map((revision) => (
              <Typography.Text key={revision.id}>
                修订 {revision.revision} · {revision.status} ·{" "}
                {revision.content_hash.slice(0, 12)}
              </Typography.Text>
            ))}
          </Space>
        </Modal>
      )}
    </>
  );
}
