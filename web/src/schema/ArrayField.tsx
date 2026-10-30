import { Button, Tag } from "antd";
import { DeleteOutlined, PlusOutlined } from "@ant-design/icons";
import { initial } from "../model.mjs";
import { SchemaField } from "./SchemaField";
import type { FieldProps } from "./types";
export function ArrayField({
  schema = {},
  schemas,
  value,
  onChange,
  depth = 0,
}: FieldProps) {
  const array: any[] = Array.isArray(value) ? value : [];
  return (
    <div className="schema-array">
      {array.map((item, index) => (
        <div className="schema-array-item" key={index}>
          <Tag>{index + 1}</Tag>
          <div className="schema-value">
            <SchemaField
              schema={schema.items || {}}
              schemas={schemas}
              value={item}
              onChange={(v) =>
                onChange(array.map((old, i) => (i === index ? v : old)))
              }
              depth={depth + 1}
            />
          </div>
          <Button
            aria-label={`删除第 ${index + 1} 项`}
            icon={<DeleteOutlined />}
            onClick={() => onChange(array.filter((_, i) => i !== index))}
          />
        </div>
      ))}
      <Button
        icon={<PlusOutlined />}
        onClick={() =>
          onChange([...array, initial(schema.items || {}, schemas)])
        }
      >
        添加一项
      </Button>
    </div>
  );
}
