// Pure schema operations, shared with the no-build Node tests.
export function resolve(schema = {}, schemas = {}) {
  if (!schema.$ref) return schema;
  const name = schema.$ref.split("/").pop();
  return {
    ...(schemas[name] || {}),
    ...Object.fromEntries(
      Object.entries(schema).filter(([key]) => key !== "$ref"),
    ),
  };
}
export function inferred(value) {
  return value === null
    ? "null"
    : Array.isArray(value)
      ? "array"
      : typeof value === "object"
        ? "object"
        : typeof value;
}
export function initial(schema = {}, schemas = {}) {
  schema = resolve(schema, schemas);
  if (schema.const !== undefined) return schema.const;
  if (schema.enum?.length) return schema.enum[0];
  if (schema.oneOf?.length || schema.anyOf?.length)
    return initial((schema.oneOf || schema.anyOf)[0], schemas);
  switch (schema.type) {
    case "object":
      return Object.fromEntries(
        (schema.required || [])
          .filter((key) => schema.properties?.[key])
          .map((key) => [key, initial(schema.properties[key], schemas)]),
      );
    case "array":
      return [];
    case "boolean":
      return false;
    case "integer":
    case "number":
      return schema.minimum ?? 0;
    case "null":
      return null;
    default:
      return "";
  }
}
export function branch(schema, value, schemas = {}) {
  const options = schema.oneOf || schema.anyOf || [];
  const key = schema.discriminator?.propertyName;
  if (key && value && typeof value === "object") {
    const found = options.findIndex((option) =>
      resolve(option, schemas).properties?.[key]?.enum?.includes(value[key]),
    );
    if (found >= 0) return found;
  }
  const found = options.findIndex((option) => {
    const candidate = resolve(option, schemas);
    return (
      candidate.type === inferred(value) ||
      (candidate.type === "integer" && typeof value === "number")
    );
  });
  return Math.max(0, found);
}
export function setField(object, key, value) {
  const next = { ...object };
  if (value === undefined) delete next[key];
  else next[key] = value;
  return next;
}
export function parameters(operation, values) {
  const path = {},
    query = {};
  for (const item of operation.parameters || []) {
    const value = values[item.name];
    if (value === undefined) continue;
    if (item.in === "path") path[item.name] = encodeURIComponent(String(value));
    if (item.in === "query")
      query[item.name] = Array.isArray(value) ? value.join(",") : String(value);
  }
  return { path, query };
}

export function mutating(operation, body = {}) {
  if (
    [
      "/api/v1/xray/stats/read",
      "/api/v1/xray/stats/query",
      "/api/v1/xray/stats/users/query",
    ].includes(operation.path)
  )
    return body?.reset === true;
  if (operation.path === "/api/v1/xray/routing/test")
    return (
      body?.PublishResult === true ||
      body?.publishResult === true ||
      body?.publish_result === true
    );
  return operation.mutating;
}

// Feed bytes or strings; UTF-8 code points and SSE lines may span transport chunks.
export function eventParser(onEvent) {
  const decoder = new TextDecoder();
  let buffer = "",
    event = "message",
    data = [];
  return (chunk) => {
    buffer +=
      typeof chunk === "string"
        ? chunk
        : decoder.decode(chunk, { stream: true });
    let end;
    while ((end = buffer.indexOf("\n")) >= 0) {
      const line = buffer.slice(0, end).replace(/\r$/, "");
      buffer = buffer.slice(end + 1);
      if (line === "") {
        if (data.length) onEvent({ event, data: data.join("\n") });
        event = "message";
        data = [];
      } else if (!line.startsWith(":")) {
        const colon = line.indexOf(":");
        const field = colon < 0 ? line : line.slice(0, colon);
        const value = colon < 0 ? "" : line.slice(colon + 1).replace(/^ /, "");
        if (field === "event") event = value;
        if (field === "data") data.push(value);
      }
    }
  };
}
