import type { ReactNode } from "react";
import { WorkspaceContext } from "./workspace";
import { useWorkspaceData } from "./useWorkspaceData";
export function WorkspaceProvider({
  children,
  onUnauthorized,
}: {
  children: ReactNode;
  onUnauthorized: () => void;
}) {
  return (
    <WorkspaceContext value={useWorkspaceData(onUnauthorized)}>
      {children}
    </WorkspaceContext>
  );
}
