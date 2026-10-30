import { App, Card, Typography } from "antd";
import { TaskTable } from "../components/TaskTable";
import { send, errorMessage } from "../api";
import { useWorkspace } from "../state/workspace";
export function TasksPage() {
  const { tasks, refresh } = useWorkspace();
  const { message } = App.useApp();
  const showError = (error: unknown) => message.error(errorMessage(error));
  return (
    <Card title="持久化操作记录">
      <Typography.Paragraph type="secondary">
        已提交不代表已生效。离线节点恢复后自动继续；需要人工处理的任务请查看原始响应。
      </Typography.Paragraph>
      <TaskTable
        tasks={tasks}
        retry={async (id) => {
          try {
            await send(`/tasks/${id}/retry`, {});
            await refresh();
          } catch (e) {
            showError(e);
          }
        }}
      />
    </Card>
  );
}
