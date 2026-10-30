import { useCallback, useEffect, useState } from "react";
import { api } from "../api";
import type { Records } from "../types";
const empty: Records = { agents: [], users: [], assignments: [], tasks: [] };
async function fetchRecords(): Promise<Records> {
  const [agents, users, assignments, tasks] = await Promise.all([
    api("/agents"),
    api("/users"),
    api("/assignments"),
    api("/tasks"),
  ]);
  return { agents, users, assignments, tasks };
}
export function useWorkspaceData(onUnauthorized: () => void) {
  const [data, setData] = useState(empty);
  const [ready, setReady] = useState(false);
  const [agent, setAgent] = useState("local");
  const [revision, setRevision] = useState(0);
  const [inspection, setInspection] = useState<{
    key: string;
    value?: any;
    error?: string;
  }>();
  const key = `${agent}:${revision}`;
  const current = inspection?.key === key ? inspection : undefined;
  const refresh = useCallback(async () => {
    setData(await fetchRecords());
  }, []);
  const discover = useCallback(
    (id = agent) => {
      setAgent(id);
      setRevision((value) => value + 1);
    },
    [agent],
  );
  useEffect(() => {
    let active = true;
    fetchRecords()
      .then((value) => {
        if (active) {
          setData(value);
          setReady(true);
        }
      })
      .catch(() => {
        if (active) onUnauthorized();
      });
    const timer = setInterval(() => {
      refresh().catch((error) => {
        if (active && error.status === 401) onUnauthorized();
      });
    }, 3000);
    return () => {
      active = false;
      clearInterval(timer);
    };
  }, [refresh, onUnauthorized]);
  useEffect(() => {
    let active = true;
    api(`/agents/${encodeURIComponent(agent)}/discovery`)
      .then((value) => {
        if (active) setInspection({ key, value });
      })
      .catch((error) => {
        if (active) setInspection({ key, error: (error as Error).message });
      });
    return () => {
      active = false;
    };
  }, [agent, key]);
  return {
    ...data,
    ready,
    agent,
    setAgent,
    discovery: current?.value,
    connectionError: current?.error || "",
    loading: !current,
    refresh,
    discover,
  };
}
