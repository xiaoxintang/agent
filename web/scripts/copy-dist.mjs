import { access, cp, rm } from "node:fs/promises";

const source = new URL("../dist/", import.meta.url);
const destination = new URL("../../crates/server/web-dist/", import.meta.url);

await access(new URL("index.html", source));
await rm(destination, { recursive: true, force: true });
await cp(source, destination, { recursive: true });
console.log("Copied Vite assets to crates/server/web-dist");
