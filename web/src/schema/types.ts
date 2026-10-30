export type Schema = Record<string, any>;
export type FieldProps = {
  schema?: Schema;
  schemas: Record<string, Schema>;
  value: any;
  onChange: (value: any) => void;
  depth?: number;
};
export const valueTypes = [
  "string",
  "number",
  "boolean",
  "object",
  "array",
  "null",
];
