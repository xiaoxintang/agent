import { Button, Space, Table, Tag, Typography } from "antd";
import { Result } from "./Result";
import { Status } from "./Status";
export function TaskTable({
  tasks,
  retry,
  compact = false,
}: {
  tasks: any[];
  retry: (id: string) => Promise<void>;
  compact?: boolean;
}) {
  return (
    <Table
      size="small"
      rowKey="id"
      dataSource={tasks}
      pagination={compact ? false : { pageSize: 15 }}
      expandable={
        compact
          ? undefined
          : { expandedRowRender: (row) => <Result value={row} /> }
      }
      columns={[
        {
          title: "操作",
          render: (_, row) => (
            <Space>
              <Tag>{row.request.method}</Tag>
              <Typography.Text
                code
                ellipsis
                style={{ maxWidth: compact ? 240 : 420 }}
              >
                {row.request.path}
              </Typography.Text>
            </Space>
          ),
        },
        {
          title: "状态",
          dataIndex: "status",
          render: (v) => <Status value={v} />,
        },
        ...(!compact
          ? [
              { title: "次数", dataIndex: "attempts" },
              { title: "错误", dataIndex: "error", ellipsis: true },
              {
                title: "操作",
                render: (_: unknown, row: any) =>
                  ["blocked", "retry"].includes(row.status) ? (
                    <Button onClick={() => void retry(row.id)}>
                      核对并重试
                    </Button>
                  ) : null,
              },
            ]
          : []),
      ]}
    />
  );
}
