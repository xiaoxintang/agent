import { useEffect, useRef, useState } from "react";
import { App } from "antd";
import { send, errorMessage } from "../api";
import { initial, parameters, mutating } from "../model.mjs";
import operations from "../operations.json";
import { useWorkspace } from "../state/workspace";
import { OperationPreview } from "./OperationPreview";
import { readEvents } from "./events";

export function useOperation(operationKey: string) {
  const { agent, agents, discovery, refresh } = useWorkspace();
  const { message, modal } = App.useApp();
  const operationEntry =
    operations.find(
      (entry) => `${entry.method} ${entry.path}` === operationKey,
    ) || operations[0];
  const operation =
    discovery?.openapi?.paths?.[operationEntry.path]?.[operationEntry.method];
  const schemas = discovery?.openapi?.components?.schemas || {};
  const requestSchema =
    operation?.requestBody?.content?.["application/json"]?.schema;
  const [parameterValues, setParameterValues] = useState<Record<string, any>>(
    {},
  );
  const [body, setBody] = useState<any>(() =>
    requestSchema ? initial(requestSchema, schemas) : undefined,
  );
  const [result, setResult] = useState<any>();
  const [sending, setSending] = useState(false);
  const [events, setEvents] = useState<any[]>([]);
  const controller = useRef<AbortController | null>(null);
  const active = useRef(true);
  const stop = () => controller.current?.abort();
  useEffect(() => {
    active.current = true;
    return () => {
      active.current = false;
      controller.current?.abort();
    };
  }, []);
  async function runOperation() {
    const receive = (value: any) => {
      if (active.current) setResult(value);
    };
    if (!operation) return;
    const missing = (operation.parameters || []).find(
      (param: any) =>
        param.required && parameterValues[param.name] === undefined,
    );
    if (missing) {
      message.warning(`请填写 ${missing.name}`);
      return;
    }
    const params = parameters(operation, parameterValues);
    let path = operationEntry.path;
    for (const [key, value] of Object.entries(params.path))
      path = path.replace(`{${key}}`, value);
    const request = {
      method: operationEntry.method.toUpperCase(),
      path,
      query: params.query,
      ...(requestSchema ? { body } : {}),
    };
    if (path === "/api/v1/xray/routing/events") {
      stop();
      setEvents([]);
      receive(undefined);
      const stream = new AbortController();
      controller.current = stream;
      void readEvents(
        agent,
        params.query,
        stream,
        (value) => {
          if (active.current) setEvents((old) => [...old.slice(-99), value]);
        },
        receive,
      );
      return;
    }
    setSending(true);
    try {
      if (mutating(operationEntry, body)) {
        const preview = await send(
          `/agents/${encodeURIComponent(agent)}/preview`,
          request,
        );
        modal.confirm({
          title: `确认执行操作 · ${agents.find((item) => item.id === agent)?.name || agent}`,
          width: 760,
          content: <OperationPreview value={preview} />,
          onOk: async () => {
            try {
              const value = await send(
                `/agents/${encodeURIComponent(agent)}/invoke`,
                { ...request, confirmation: preview.confirmation },
              );
              receive(value);
              await refresh();
              message.success("任务已提交");
            } catch (error) {
              message.error(errorMessage(error));
              throw error;
            }
          },
        });
      } else
        receive(
          await send(`/agents/${encodeURIComponent(agent)}/invoke`, request),
        );
    } catch (error) {
      receive({
        status: (error as any).status,
        ...((error as any).body || { error: errorMessage(error) }),
      });
      message.error(errorMessage(error));
    } finally {
      setSending(false);
    }
  }
  return {
    operationEntry,
    operation,
    operationKey,
    parameterValues,
    setParameterValues,
    body,
    setBody,
    result,
    sending,
    events,
    stop,
    runOperation,
  };
}
