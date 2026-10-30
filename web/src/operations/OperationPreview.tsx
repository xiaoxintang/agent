import { Alert, Typography } from "antd";
import { pretty } from "../api";
export function OperationPreview({ value }: { value: any }) {
  return (
    <>
      <Typography.Paragraph>
        {value.restarts_xray
          ? "此操作将替换完整配置并重启 Xray。"
          : "操作将持久化为任务，执行状态可在任务页面查看。"}
      </Typography.Paragraph>
      {value.effects.length > 0 && (
        <Alert
          type="warning"
          title={`将同步更新 ${value.effects.length} 项用户分配`}
        />
      )}
      <pre className="response preview">{pretty(value)}</pre>
    </>
  );
}
