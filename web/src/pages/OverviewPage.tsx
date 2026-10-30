import { Card, Descriptions, Empty, Statistic, Tag } from "antd";
import { TaskTable } from "../components/TaskTable";
import { send } from "../api";
import { useWorkspace } from "../state/workspace";
export function OverviewPage() {
  const { agents, users, assignments, tasks, discovery, refresh } =
    useWorkspace();
  return (
    <>
      <div className="metrics">
        {[
          ["Agent 节点", agents.length],
          ["Xray 用户", users.filter((u) => u.status === "active").length],
          [
            "生效分配",
            assignments.filter((a) => a.status === "applied").length,
          ],
          [
            "待处理任务",
            tasks.filter((t) =>
              ["queued", "retry", "running", "blocked"].includes(t.status),
            ).length,
          ],
        ].map(([title, value]) => (
          <Card key={title}>
            <Statistic title={title} value={value} />
          </Card>
        ))}
      </div>
      <div className="overview-grid">
        <Card title="当前节点">
          {discovery ? (
            <Descriptions
              column={1}
              items={[
                {
                  key: "version",
                  label: "Agent 版本",
                  children: discovery.openapi?.info?.version,
                },
                {
                  key: "xray",
                  label: "Xray 版本",
                  children: discovery.status.active_version || "未安装",
                },
                {
                  key: "process",
                  label: "进程状态",
                  children: discovery.status.system?.active_state || "未知",
                },
                {
                  key: "revision",
                  label: "配置修订",
                  children: discovery.status.revision,
                },
                {
                  key: "sync",
                  label: "同步状态",
                  children: (
                    <Tag
                      color={
                        discovery.status.sync_state === "in_sync"
                          ? "green"
                          : "orange"
                      }
                    >
                      {discovery.status.sync_state}
                    </Tag>
                  ),
                },
              ]}
            />
          ) : (
            <Empty description="选择节点后加载状态" />
          )}
        </Card>
        <Card title="近期任务">
          <TaskTable
            tasks={tasks.slice(0, 6)}
            retry={async (id) => {
              await send(`/tasks/${id}/retry`, {});
              await refresh();
            }}
            compact
          />
        </Card>
      </div>
    </>
  );
}
