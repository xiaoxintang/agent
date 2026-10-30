import { Select, Space, Typography } from "antd";
import { SchemaField } from "../schema/SchemaField";
import { initial } from "../model.mjs";
export function RequestParameters({
  operation,
  parameterValues,
  setParameterValues,
  schemas,
}: {
  operation: any;
  parameterValues: Record<string, any>;
  setParameterValues: React.Dispatch<React.SetStateAction<Record<string, any>>>;
  schemas: Record<string, any>;
}) {
  return (
    <>
      {(operation.parameters || [])
        .filter((p: any) => ["path", "query"].includes(p.in))
        .map((p: any) => (
          <div className="parameter" key={p.name}>
            <Typography.Text strong>
              {p.name} {p.required ? "*" : ""}
            </Typography.Text>
            <Space>
              <Select
                aria-label={`设置参数 ${p.name}`}
                size="small"
                value={parameterValues[p.name] === undefined ? "omit" : "set"}
                options={[
                  { value: "omit", label: "不传" },
                  { value: "set", label: "设置" },
                ]}
                onChange={(v) =>
                  setParameterValues((old) => {
                    const next = { ...old };
                    if (v === "omit") delete next[p.name];
                    else next[p.name] = initial(p.schema, schemas);
                    return next;
                  })
                }
              />
              {parameterValues[p.name] !== undefined && (
                <SchemaField
                  schema={p.schema}
                  schemas={schemas}
                  value={parameterValues[p.name]}
                  onChange={(v) =>
                    setParameterValues((old) => ({
                      ...old,
                      [p.name]: v,
                    }))
                  }
                />
              )}
            </Space>
          </div>
        ))}
    </>
  );
}
