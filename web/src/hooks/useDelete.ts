import { App } from "antd";
import { api, errorMessage } from "../api";
import { useWorkspace } from "../state/workspace";
export function useDelete() {
  const { modal, message } = App.useApp();
  const { refresh } = useWorkspace();
  return (title: string, path: string) =>
    modal.confirm({
      title,
      content: "远端撤销未完成时会保留待处理记录。",
      onOk: async () => {
        try {
          await api(path, { method: "DELETE" });
          await refresh();
        } catch (error) {
          message.error(errorMessage(error));
          throw error;
        }
      },
    });
}
