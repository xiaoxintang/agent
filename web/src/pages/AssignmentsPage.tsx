import { useState } from "react";
import { Button, Card, Table } from "antd";
import { Result } from "../components/Result";
import { Status } from "../components/Status";
import { AssignmentDialog } from "./AssignmentDialog";
import { useDelete } from "../hooks/useDelete";
import { useWorkspace } from "../state/workspace";
export function AssignmentsPage() {
  const { users, assignments, agents, agent } = useWorkspace();
  const [open, setOpen] = useState(false);
  const confirmDelete = useDelete();
  return (
    <>
      <Card
        title="入站授权"
        extra={
          <Button
            type="primary"
            disabled={!users.some((u) => u.status === "active")}
            onClick={() => setOpen(true)}
          >
            分配用户
          </Button>
        }
      >
        <Table
          rowKey="id"
          dataSource={assignments}
          expandable={{
            expandedRowRender: (row) => <Result value={row.native_user} />,
          }}
          columns={[
            {
              title: "Xray 用户",
              render: (_, row) =>
                users.find((u) => u.id === row.user_id)?.name || row.user_id,
            },
            {
              title: "Agent",
              render: (_, row) =>
                agents.find((a) => a.id === row.agent_id)?.name || row.agent_id,
            },
            { title: "入站", dataIndex: "inbound_tag" },
            {
              title: "期望",
              dataIndex: "desired_present",
              render: (v) => (v ? "授权" : "撤销"),
            },
            {
              title: "状态",
              dataIndex: "status",
              render: (v) => <Status value={v} />,
            },
            {
              title: "操作",
              render: (_, row) => (
                <Button
                  danger
                  disabled={!row.desired_present}
                  onClick={() =>
                    confirmDelete("撤销此分配？", `/assignments/${row.id}`)
                  }
                >
                  撤销
                </Button>
              ),
            },
          ]}
        />
      </Card>
      {open && (
        <AssignmentDialog agent={agent} onClose={() => setOpen(false)} />
      )}
    </>
  );
}
