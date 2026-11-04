import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Alert, Button, Modal, Space, Table, Typography } from "antd";
import { ApiError, getTraffic } from "../api/panel";

const mib = (bytes: string) => `${(Number(bytes) / 1048576).toFixed(2)} MiB`;
export default function TrafficPanel({ nodeId }: { nodeId: string }) {
  const [open, setOpen] = useState(false);
  const cache = useQueryClient();
  const traffic = useQuery({
    queryKey: ["traffic", nodeId],
    queryFn: () => getTraffic(nodeId),
    enabled: open,
    retry: false,
    refetchInterval: open ? 15000 : false,
  });
  useEffect(() => {
    if (traffic.error instanceof ApiError && traffic.error.status === 401)
      cache.setQueryData(["session"], null);
  }, [cache, traffic.error]);
  const buckets = traffic.data?.buckets ?? [];
  const max = Math.max(
    1,
    ...buckets.map(
      (bucket) => Number(bucket.uplink_bytes) + Number(bucket.downlink_bytes),
    ),
  );
  return (
    <>
      <Button size="small" onClick={() => setOpen(true)}>
        流量统计
      </Button>
      {open && (
        <Modal
          title="节点流量"
          open
          width={760}
          onCancel={() => setOpen(false)}
          footer={<Button onClick={() => setOpen(false)}>关闭流量</Button>}
        >
          <Space orientation="vertical" style={{ width: "100%" }}>
            {traffic.error && (
              <Alert type="error" title={traffic.error.message} />
            )}
            {traffic.data &&
              (!traffic.data.available || traffic.data.stale) && (
                <Alert
                  type="warning"
                  title="统计不可用或已过期；历史累计值保留，当前数值不代表实时状态。"
                />
              )}
            <Typography.Paragraph>
              最近 7 天每小时上行/下行。首次观察和 Xray
              重启后的首轮只建立基线；用量可能少计，配额是最终一致限制。
            </Typography.Paragraph>
            {buckets.length > 0 ? (
              <svg
                role="img"
                aria-label="最近 7 天小时流量图"
                viewBox="0 0 640 180"
                width="100%"
              >
                {buckets.map((bucket, index) => {
                  const width = 640 / buckets.length;
                  const up = (Number(bucket.uplink_bytes) / max) * 160;
                  const down = (Number(bucket.downlink_bytes) / max) * 160;
                  return (
                    <g key={bucket.bucket_start}>
                      <title>
                        {new Date(bucket.bucket_start * 1000).toLocaleString()}{" "}
                        上行 {mib(bucket.uplink_bytes)} 下行{" "}
                        {mib(bucket.downlink_bytes)}
                      </title>
                      <rect
                        x={index * width}
                        y={170 - up}
                        width={Math.max(1, width - 1)}
                        height={up}
                        fill="#1677ff"
                      />
                      <rect
                        x={index * width}
                        y={170 - up - down}
                        width={Math.max(1, width - 1)}
                        height={down}
                        fill="#52c41a"
                      />
                    </g>
                  );
                })}
              </svg>
            ) : (
              <Typography.Text>尚无已记账流量</Typography.Text>
            )}
            <Typography.Text>
              蓝色：上行；绿色：下行。客户端累计值不受图表的 7 天保留期影响。
            </Typography.Text>
            <Table
              size="small"
              rowKey="client_id"
              pagination={{ pageSize: 10 }}
              dataSource={traffic.data?.clients}
              columns={[
                { title: "客户端", dataIndex: "display_name" },
                { title: "累计上行", dataIndex: "uplink_bytes", render: mib },
                { title: "累计下行", dataIndex: "downlink_bytes", render: mib },
              ]}
            />
          </Space>
        </Modal>
      )}
    </>
  );
}
