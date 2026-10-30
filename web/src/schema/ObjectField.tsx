import { useState } from "react";
import {
  Button,
  Collapse,
  Input,
  Select,
  Space,
  Switch,
  Tag,
  Typography,
} from "antd";
import { PlusOutlined } from "@ant-design/icons";
import { initial, setField } from "../model.mjs";
import { SchemaField } from "./SchemaField";
import { valueTypes as types, type FieldProps, type Schema } from "./types";
export function ObjectField({
  schema = {},
  schemas,
  value,
  onChange,
  depth = 0,
}: FieldProps) {
  const [key, setKey] = useState("");
  const [newType, setNewType] = useState("string");
  const object =
    value && typeof value === "object" && !Array.isArray(value) ? value : {};
  const props: Record<string, Schema> = schema.properties || {};
  const keys = Array.from(
    new Set([...Object.keys(props), ...Object.keys(object)]),
  );
  const content = (
    <div className="schema-object">
      {keys.map((name) => {
        const present = Object.hasOwn(object, name);
        const required = schema.required?.includes(name);
        const field =
          props[name] ||
          (typeof schema.additionalProperties === "object"
            ? schema.additionalProperties
            : {});
        return (
          <div className="schema-row" key={name}>
            <div className="schema-label">
              <Switch
                size="small"
                aria-label={`启用 ${name}`}
                checked={present}
                disabled={required}
                onChange={(enabled) =>
                  onChange(
                    setField(
                      object,
                      name,
                      enabled ? initial(field, schemas) : undefined,
                    ),
                  )
                }
              />
              <Typography.Text code>{name}</Typography.Text>
              {required && <Tag color="blue">必填</Tag>}
            </div>
            {present && (
              <div className="schema-value">
                <SchemaField
                  schema={field}
                  schemas={schemas}
                  value={object[name]}
                  onChange={(v) => onChange(setField(object, name, v))}
                  depth={depth + 1}
                />
              </div>
            )}
          </div>
        );
      })}
      {schema.additionalProperties !== false && (
        <Space wrap className="schema-add">
          <Input
            aria-label="新增字段名"
            placeholder="新增字段名"
            value={key}
            onChange={(e) => setKey(e.target.value)}
            style={{ width: 170 }}
          />
          <Select
            aria-label="新增字段类型"
            value={newType}
            options={types.map((t) => ({ value: t, label: t }))}
            onChange={setNewType}
            style={{ width: 110 }}
          />
          <Button
            icon={<PlusOutlined />}
            disabled={
              !key ||
              Object.hasOwn(object, key) ||
              ["__proto__", "constructor", "prototype"].includes(key)
            }
            onClick={() => {
              onChange(
                setField(
                  object,
                  key,
                  initial(
                    typeof schema.additionalProperties === "object"
                      ? schema.additionalProperties
                      : { type: newType },
                    schemas,
                  ),
                ),
              );
              setKey("");
            }}
          >
            添加字段
          </Button>
        </Space>
      )}
    </div>
  );
  return depth > 0 ? (
    <Collapse
      size="small"
      defaultActiveKey={depth < 2 ? ["fields"] : []}
      items={[
        {
          key: "fields",
          label: `${Object.keys(object).length} 个已设置字段`,
          children: content,
        },
      ]}
    />
  ) : (
    content
  );
}
