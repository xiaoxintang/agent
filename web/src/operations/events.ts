import { eventParser } from "../model.mjs";
export async function readEvents(
  agent: string,
  query: Record<string, string>,
  controller: AbortController,
  onRoute: (value: any) => void,
  onError: (value: any) => void,
) {
  try {
    const response = await fetch(
      `/api/agents/${encodeURIComponent(agent)}/events?${new URLSearchParams(query)}`,
      {
        signal: controller.signal,
        headers: { "X-Web-Version": __WEB_VERSION__ },
      },
    );
    if (!response.ok) {
      const text = await response.text();
      let error;
      try {
        error = JSON.parse(text);
      } catch {
        error = { error: text };
      }
      onError({ status: response.status, ...error });
      return;
    }
    if (!response.body) throw new Error("事件响应没有数据流");
    const feed = eventParser((frame) => {
      let value;
      try {
        value = JSON.parse(frame.data);
      } catch {
        value = { data: frame.data };
      }
      if (frame.event === "route") onRoute(value);
      else if (frame.event === "error") {
        onError(value);
        controller.abort();
      }
    });
    const reader = response.body.getReader();
    try {
      while (!controller.signal.aborted) {
        const chunk = await reader.read();
        if (chunk.done) break;
        feed(chunk.value);
      }
    } finally {
      await reader.cancel().catch(() => {});
    }
  } catch (error) {
    if (!controller.signal.aborted)
      onError({ error: (error as Error).message });
  }
}
