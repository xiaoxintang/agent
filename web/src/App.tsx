import { useCallback, useEffect, useState } from "react";
import { Spin } from "antd";
import { api, send } from "./api";
import type { Session } from "./types";
import { LoginPage } from "./pages/LoginPage";
import { Shell } from "./components/Shell";
import { WorkspaceProvider } from "./state/WorkspaceProvider";

export default function App() {
  const [session, setSession] = useState<Session | null>(null);
  const [ready, setReady] = useState(false);
  const clearSession = useCallback(() => setSession(null), []);
  async function authenticate() {
    try {
      setSession(await api("/session"));
    } catch {
      clearSession();
    } finally {
      setReady(true);
    }
  }
  useEffect(() => {
    let active = true;
    api("/session")
      .then((value) => {
        if (active) setSession(value);
      })
      .catch(() => {
        if (active) clearSession();
      })
      .finally(() => {
        if (active) setReady(true);
      });
    return () => {
      active = false;
    };
  }, [clearSession]);
  if (!ready)
    return (
      <div className="boot">
        <Spin size="large" />
      </div>
    );
  if (!session) return <LoginPage onLogin={() => void authenticate()} />;
  return (
    <WorkspaceProvider onUnauthorized={clearSession}>
      <Shell
        session={session}
        logout={async () => {
          await send("/logout", {});
          clearSession();
        }}
      />
    </WorkspaceProvider>
  );
}
