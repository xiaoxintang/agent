import { createContext, useContext } from "react";
import type { useWorkspaceData } from "./useWorkspaceData";
export const WorkspaceContext = createContext<ReturnType<
  typeof useWorkspaceData
> | null>(null);
export function useWorkspace() {
  const value = useContext(WorkspaceContext);
  if (!value) throw new Error("WorkspaceProvider is missing");
  return value;
}
