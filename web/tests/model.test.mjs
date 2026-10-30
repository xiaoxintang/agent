import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  branch,
  initial,
  mutating,
  eventParser,
  parameters,
  resolve,
  setField,
} from "../src/model.mjs";
const schemas = JSON.parse(
  readFileSync(new URL("../../schemas/xray-native.json", import.meta.url)),
);
test("editing one field retains unknown fields and preserves false, zero, empty and null", () => {
  const original = {
    unknown: { nested: [false, 0, "", null] },
    enabled: false,
  };
  const edited = setField(original, "port", 0);
  assert.deepEqual(edited.unknown, original.unknown);
  assert.equal(edited.port, 0);
  assert.equal(setField(edited, "enabled", undefined).enabled, undefined);
  assert.equal(original.enabled, false);
});
test("protocol discriminator selects and initializes the corresponding native form", () => {
  const schema = schemas.InboundConfig;
  const index = branch(schema, { protocol: "vless" }, schemas);
  assert.equal(schema.oneOf[index].properties.protocol.enum[0], "vless");
  assert.equal(initial(schema.oneOf[index], schemas).protocol, "vless");
  assert.ok(
    resolve(schema.oneOf[index].properties.settings, schemas).properties
      .clients,
  );
});
test("int64 strings never pass through JavaScript numbers", () => {
  const value = "18446744073709551615";
  assert.equal(setField({}, "bytes", value).bytes, value);
});
test("parameters encode tag and email independently and omit only unset values", () => {
  const operation = {
    parameters: [
      { name: "tag", in: "path" },
      { name: "email", in: "path" },
      { name: "reset", in: "query" },
      { name: "count", in: "query" },
      { name: "absent", in: "query" },
    ],
  };
  assert.deepEqual(
    parameters(operation, {
      tag: "入口/a",
      email: "a+b@example.com",
      reset: false,
      count: 0,
    }),
    {
      path: { tag: "%E5%85%A5%E5%8F%A3%2Fa", email: "a%2Bb%40example.com" },
      query: { reset: "false", count: "0" },
    },
  );
});
test("nested optional objects are not invented during initialization", () => {
  assert.deepEqual(
    initial({
      type: "object",
      required: ["enabled"],
      properties: {
        enabled: { type: "boolean" },
        optional: { type: "object" },
      },
    }),
    { enabled: false },
  );
});

test("every mask and balancing strategy can be selected without falling back to the first branch", () => {
  for (const name of [
    "native.Mask.tcp",
    "native.Mask.udp",
    "native.StrategyConfig",
  ]) {
    const schema = schemas[name];
    schema.oneOf.forEach((choice, index) => {
      assert.equal(
        branch(schema, initial(choice, schemas), schemas),
        index,
        `${name} branch ${index}`,
      );
    });
  }
});

test("counter reads stay read-only while explicit resets remain durable writes", () => {
  const operation = { path: "/api/v1/xray/stats/query", mutating: true };
  assert.equal(mutating(operation, {}), false);
  assert.equal(mutating(operation, { reset: false }), false);
  assert.equal(mutating(operation, { reset: true }), true);
  assert.equal(
    mutating({ path: "/api/v1/xray/restart", mutating: true }),
    true,
  );
});

test("SSE parser preserves fragmented UTF-8, multiline data and upstream error frames", () => {
  const events = [];
  const feed = eventParser((event) => events.push(event));
  const bytes = new TextEncoder().encode(
    ': keepalive\r\nevent: route\r\ndata: {"tag":"入口"}\r\n\r\nevent: error\ndata: first\ndata: second\n\n',
  );
  for (const byte of bytes) feed(new Uint8Array([byte]));
  assert.deepEqual(events, [
    { event: "route", data: '{"tag":"入口"}' },
    { event: "error", data: "first\nsecond" },
  ]);
});
