import { RequestParameters } from "./RequestParameters";
import { useState } from "react";
import {
  Alert,
  Button,
  Card,
  Empty,
  Select,
  Space,
  Tag,
  Typography,
} from "antd";
import { SchemaField } from "../schema/SchemaField";
import { Result } from "../components/Result";
import { Status } from "../components/Status";
import { mutating } from "../model.mjs";
import { send, errorMessage } from "../api";
import operations from "../operations.json";
import { useOperation } from "./useOperation";
import { useWorkspace } from "../state/workspace";
import { App } from "antd";
export function OperationPage({ group }: { group: string }) {
  const { agent, discovery } = useWorkspace();
  const first = operations.find((entry) => entry.group === group)!;
  const [operationKey, setOperationKey] = useState(
    `${first.method} ${first.path}`,
  );
  return (
    <OperationEditor
      key={`${agent}:${operationKey}:${!!discovery}`}
      group={group}
      operationKey={operationKey}
      setOperationKey={setOperationKey}
    />
  );
}
function OperationEditor({
  group,
  operationKey,
  setOperationKey,
}: {
  group: string;
  operationKey: string;
  setOperationKey: (value: string) => void;
}) {
  const { agent, discovery, loading, tasks } = useWorkspace();
  const { message } = App.useApp();
  const showError = (error: unknown) => message.error(errorMessage(error));
  const {
    operationEntry,
    operation,
    parameterValues,
    setParameterValues,
    body,
    setBody,
    result,
    sending,
    events,
    stop,
    runOperation,
  } = useOperation(operationKey);
  const schemas = discovery?.openapi?.components?.schemas || {};
  const requestSchema =
    operation?.requestBody?.content?.["application/json"]?.schema;
  const resultTask = result?.task_id
    ? tasks.find((task) => task.id === result.task_id)
    : undefined;
  const operationResult = resultTask?.result?.body ?? result;
  const category = group;
  return (
    <>
      <Card className="operation-card">
        <Space wrap>
          <Select
            aria-label="管理接口"
            value={operationKey}
            style={{ minWidth: 440, maxWidth: "100%" }}
            options={operations
              .filter((e) => e.group === category)
              .map((e) => ({
                value: `${e.method} ${e.path}`,
                label: `${e.method.toUpperCase()} · ${e.label}`,
              }))}
            onChange={setOperationKey}
          />
          <Tag color={mutating(operationEntry, body) ? "orange" : "blue"}>
            {operationEntry.method.toUpperCase()}
          </Tag>
          <Typography.Text code>{operationEntry.path}</Typography.Text>
        </Space>
      </Card>
      {!operation ? (
        <Alert
          type="warning"
          title={
            loading ? "正在加载接口定义" : "当前 Agent 未提供此接口，无法调用"
          }
        />
      ) : (
        <div className="operation-grid">
          <Card
            title={operationEntry.label}
            extra={
              operationEntry.path === "/api/v1/xray/config" &&
              operationEntry.method === "put" ? (
                <Button
                  onClick={async () => {
                    try {
                      const v = await send(`/agents/${agent}/invoke`, {
                        method: "GET",
                        path: "/api/v1/xray/config",
                      });
                      setBody(v.config);
                    } catch (e) {
                      showError(e);
                    }
                  }}
                >
                  载入当前配置
                </Button>
              ) : undefined
            }
          >
            <RequestParameters
              operation={operation}
              parameterValues={parameterValues}
              setParameterValues={setParameterValues}
              schemas={schemas}
            />
            {requestSchema && (
              <SchemaField
                schema={requestSchema}
                schemas={schemas}
                value={body}
                onChange={setBody}
              />
            )}
            <Space className="submit-row">
              <Button
                type="primary"
                loading={sending}
                onClick={() => void runOperation()}
              >
                {operationEntry.path.endsWith("/events")
                  ? "开始订阅"
                  : mutating(operationEntry, body)
                    ? "预览并执行"
                    : "查询"}
              </Button>
              {operationEntry.path.endsWith("/events") && (
                <Button onClick={() => stop()}>停止订阅</Button>
              )}
            </Space>
          </Card>
          <Card title="执行结果">
            {resultTask && (
              <Space wrap style={{ marginBottom: 16 }}>
                <Status value={resultTask.status} />
                {resultTask.result?.status && (
                  <Tag>HTTP {resultTask.result.status}</Tag>
                )}
                <Typography.Text type="secondary">
                  {resultTask.error || resultTask.id}
                </Typography.Text>
              </Space>
            )}
            {result?.error && events.length > 0 && (
              <Alert
                type="error"
                title={result.error}
                description={
                  result.status ? `HTTP ${result.status}` : undefined
                }
              />
            )}
            {events.length > 0 ? (
              <Result value={events} />
            ) : result === undefined ? (
              <Empty description="执行后显示响应" />
            ) : (
              <Result value={operationResult} />
            )}
          </Card>
        </div>
      )}
    </>
  );
}
