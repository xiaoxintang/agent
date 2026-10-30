import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { readFileSync } from "node:fs";

const version = readFileSync(
  new URL("../Cargo.toml", import.meta.url),
  "utf8",
).match(/^version\s*=\s*"([^"]+)"/m)![1];
export default defineConfig({
  plugins: [
    react(),
    {
      name: "release-version",
      generateBundle() {
        this.emitFile({
          type: "asset",
          fileName: "version.json",
          source: JSON.stringify({ version }),
        });
      },
    },
  ],
  define: { __WEB_VERSION__: JSON.stringify(version) },
  server: { proxy: { "/api": "http://127.0.0.1:3000" } },
  build: { sourcemap: false },
});
