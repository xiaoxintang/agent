export function resolve(schema?: any, schemas?: Record<string, any>): any;
export function inferred(value: any): string;
export function initial(schema?: any, schemas?: Record<string, any>): any;
export function branch(
  schema: any,
  value: any,
  schemas?: Record<string, any>,
): number;
export function setField(object: any, key: string, value: any): any;
export function parameters(
  operation: any,
  values: Record<string, any>,
): { path: Record<string, string>; query: Record<string, string> };

export function mutating(operation: any, body?: any): boolean;
export function eventParser(
  onEvent: (event: { event: string; data: string }) => void,
): (chunk: string | Uint8Array) => void;
