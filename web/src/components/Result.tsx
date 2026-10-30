import { Table, Tabs, Typography } from "antd";
import { pretty } from "../api";
export function Result({ value }: { value: any }) {
  const rows = Array.isArray(value)
    ? value
    : Object.values(value || {}).find(Array.isArray);
  const keys =
    rows?.length && typeof rows[0] === "object"
      ? Array.from(
          new Set<string>(rows.flatMap((row: any) => Object.keys(row || {}))),
        )
      : [];
  return (
    <Tabs
      items={[
        ...(keys.length
          ? [
              {
                key: "table",
                label: "表格",
                children: (
                  <Table
                    size="small"
                    rowKey={(_, i) => String(i)}
                    dataSource={rows}
                    columns={keys.map((key) => ({
                      key,
                      dataIndex: key,
                      title: key,
                      render: (v: any) =>
                        typeof v === "object" ? (
                          <Typography.Text code>
                            {JSON.stringify(v)}
                          </Typography.Text>
                        ) : (
                          String(v ?? "")
                        ),
                    }))}
                    scroll={{ x: true }}
                  />
                ),
              },
            ]
          : []),
        {
          key: "raw",
          label: "响应内容",
          children: <pre className="response">{pretty(value)}</pre>,
        },
      ]}
    />
  );
}
