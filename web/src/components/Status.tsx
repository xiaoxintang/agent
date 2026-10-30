import { Tag } from "antd";
export function Status({ value }: { value: string }) {
  return (
    <Tag
      color={
        ["complete", "applied", "active"].includes(value)
          ? "green"
          : ["blocked", "deleting"].includes(value)
            ? "orange"
            : ["retry", "pending", "queued", "running"].includes(value)
              ? "blue"
              : "default"
      }
    >
      {(
        {
          complete: "已完成",
          accepted: "上游已接收",
          applied: "已生效",
          active: "正常",
          deleting: "撤销中",
          deleted: "已删除",
          pending: "待同步",
          revoked: "已撤销",
          queued: "排队中",
          running: "执行中",
          retry: "等待重试",
          blocked: "需处理",
          canceled: "已被新操作替代",
        } as Record<string, string>
      )[value] || value}
    </Tag>
  );
}
