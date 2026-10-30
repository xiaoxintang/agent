import { Input, InputNumber, Select, Switch, Tag } from "antd";
import { branch, inferred, initial, resolve } from "../model.mjs";
import { ArrayField } from "./ArrayField";
import { ObjectField } from "./ObjectField";
import { valueTypes as types, type Schema, type FieldProps } from "./types";
export type { Schema } from "./types";
export function SchemaField({
  schema: input = {},
  schemas,
  value,
  onChange,
  depth = 0,
}: FieldProps) {
  const schema = resolve(input, schemas);
  const options: Schema[] = schema.oneOf || schema.anyOf || [];
  if (options.length) {
    const selected = branch(schema, value, schemas);
    return (
      <div className="schema-branch">
        <Select
          aria-label="配置分支"
          value={selected}
          style={{ minWidth: 200 }}
          options={options.map((item, index) => {
            const target = resolve(item, schemas);
            const discriminator = schema.discriminator?.propertyName;
            return {
              value: index,
              label: discriminator
                ? (target.properties?.[discriminator]?.enum?.[0] ??
                  target.title ??
                  `分支 ${index + 1}`)
                : (target.title ??
                  target.type ??
                  item.$ref?.split(".").pop() ??
                  `分支 ${index + 1}`),
            };
          })}
          onChange={(index) => onChange(initial(options[index], schemas))}
        />
        <SchemaField
          schema={options[selected]}
          schemas={schemas}
          value={value}
          onChange={onChange}
          depth={depth}
        />
      </div>
    );
  }
  const unconstrained = !schema.type && !schema.properties && !schema.enum;
  const type = Array.isArray(schema.type)
    ? schema.type.includes(inferred(value))
      ? inferred(value)
      : schema.type[0]
    : schema.type ||
      (schema.properties
        ? "object"
        : inferred(value === undefined ? "" : value));
  let editor;
  if (schema.enum)
    editor = (
      <Select
        aria-label="枚举值"
        allowClear
        value={value}
        options={schema.enum.map((v: any) => ({ value: v, label: String(v) }))}
        onChange={onChange}
        style={{ minWidth: 180 }}
      />
    );
  else if (type === "object")
    editor = (
      <ObjectField
        schema={schema}
        schemas={schemas}
        value={value}
        onChange={onChange}
        depth={depth}
      />
    );
  else if (type === "array")
    editor = (
      <ArrayField
        schema={schema}
        schemas={schemas}
        value={value}
        onChange={onChange}
        depth={depth}
      />
    );
  else if (type === "boolean")
    editor = (
      <Switch
        aria-label="布尔值"
        checked={value === true}
        onChange={onChange}
      />
    );
  else if (type === "number" || type === "integer")
    editor = (
      <InputNumber
        aria-label="数值"
        value={value}
        min={schema.minimum}
        max={schema.maximum}
        precision={type === "integer" ? 0 : undefined}
        onChange={(v) => onChange(v)}
        style={{ width: 220 }}
      />
    );
  else if (type === "null") editor = <Tag>null</Tag>;
  else
    editor =
      schema.format === "password" ? (
        <Input.Password
          aria-label="凭据"
          value={value ?? ""}
          onChange={(e) => onChange(e.target.value)}
          style={{ maxWidth: 620 }}
        />
      ) : (
        <Input.TextArea
          autoSize={{ minRows: 1, maxRows: 10 }}
          aria-label="字段值"
          value={value ?? ""}
          placeholder={schema.description}
          onChange={(e) => onChange(e.target.value)}
          style={{ maxWidth: 620 }}
        />
      );
  return (
    <div className="schema-editor">
      {(unconstrained || Array.isArray(schema.type)) && (
        <Select
          aria-label="字段类型"
          size="small"
          value={type}
          options={(Array.isArray(schema.type) ? schema.type : types).map(
            (t: string) => ({ value: t, label: t }),
          )}
          onChange={(t) => onChange(initial({ type: t }, schemas))}
          style={{ width: 110, marginBottom: 8 }}
        />
      )}{" "}
      {editor}
    </div>
  );
}
